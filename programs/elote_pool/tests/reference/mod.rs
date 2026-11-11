//! An independent reference for ordinary interest (decision 48), written without the program: a
//! line is a list of `(time, principal change)` events, and the interest accrued at `t` is
//! `rate_bps * Σ balance * Δt / (10_000 * 365 days)` over the segments before `min(t, end)`,
//! rounded down once. `end` is the last due date, or the acceleration if earlier. Payments split
//! here, independently of the program: late interest (given by the caller), then the ordinary
//! interest accrued and unpaid, then principal, never more than is owed.
#![allow(dead_code)]

const BPS: u128 = 10_000;
const YEAR: u128 = 365 * 86_400;

#[derive(Clone, Debug)]
pub struct RefLine {
    pub rate_bps: u16,
    /// Ordinary interest stops here.
    pub end: i64,
    /// `(time, principal change)`: a draw adds, a principal repayment subtracts.
    pub events: Vec<(i64, i128)>,
    pub drawn: u128,
    pub principal_repaid: u128,
    pub interest_paid: u128,
}

impl RefLine {
    pub fn new(rate_bps: u16, end: i64) -> Self {
        RefLine { rate_bps, end, events: Vec::new(), drawn: 0, principal_repaid: 0, interest_paid: 0 }
    }

    pub fn draw(&mut self, at: i64, amount: u64) {
        self.events.push((at, amount as i128));
        self.drawn += amount as u128;
    }

    pub fn balance(&self) -> u128 {
        self.drawn - self.principal_repaid
    }

    /// `Σ balance * seconds` over `[first event, min(at, end)]`, segment by segment.
    pub fn balance_seconds(&self, at: i64) -> u128 {
        let stop = at.min(self.end);
        let mut events = self.events.clone();
        events.sort_by_key(|e| e.0);
        let mut area: u128 = 0;
        let mut balance: i128 = 0;
        for (i, &(t, delta)) in events.iter().enumerate() {
            balance += delta;
            assert!(balance >= 0, "the reference never repays more than was drawn");
            let next = events.get(i + 1).map_or(i64::MAX, |e| e.0);
            let from = t.min(stop);
            let to = next.min(stop);
            if to > from {
                area += balance as u128 * (to - from) as u128;
            }
        }
        area
    }

    /// The ordinary interest accrued by `at`, rounded down once.
    pub fn interest_accrued(&self, at: i64) -> u128 {
        self.rate_bps as u128 * self.balance_seconds(at) / (BPS * YEAR)
    }

    pub fn interest_unpaid(&self, at: i64) -> u128 {
        self.interest_accrued(at) - self.interest_paid
    }

    /// What a payment of `amount` at `at` settles once `late` (the late interest due, which the
    /// caller states) is taken: `(late, interest, principal)`.
    pub fn pay(&mut self, at: i64, amount: u64, late: u64) -> (u64, u64, u64) {
        let amount = amount as u128;
        let late = late as u128;
        assert!(amount >= late, "a payment covers the late interest");
        let rest = amount - late;
        let interest = rest.min(self.interest_unpaid(at));
        let principal = (rest - interest).min(self.balance());
        self.interest_paid += interest;
        if principal > 0 {
            self.events.push((at, -(principal as i128)));
            self.principal_repaid += principal;
        }
        (late as u64, interest as u64, principal as u64)
    }

    /// What a payment would have to bring at `at` to owe nothing more, late interest aside.
    pub fn payoff(&self, at: i64) -> u64 {
        (self.balance() + self.interest_unpaid(at)) as u64
    }
}

#[test]
fn the_reference_on_hand_numbers() {
    // 60,000.00 at 12% for a twelfth of a year: 600.00.
    let month = (YEAR / 12) as i64;
    let mut r = RefLine::new(1_200, 6 * month);
    r.draw(0, 6_000_000);
    assert_eq!(r.interest_accrued(month), 60_000);
    assert_eq!(r.interest_accrued(6 * month), 360_000);
    assert_eq!(r.interest_accrued(10 * month), 360_000, "nothing after the end");
    assert_eq!(r.pay(month, 1_060_000, 0), (0, 60_000, 1_000_000));
    assert_eq!(r.interest_accrued(2 * month), 60_000 + 50_000);
    // One centavo a second on 262,800,000 at 12%: exactly one centavo of interest a second.
    let mut r = RefLine::new(1_200, 10 * month);
    r.draw(0, 262_800_000);
    assert_eq!(r.interest_accrued(3_600), 3_600);
}

/// The pagaré's hashes, written from their specification with `sha2`, not from the program.
pub mod pagare {
    use sha2::{Digest, Sha256};

    fn sha256(parts: &[&[u8]]) -> [u8; 32] {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().into()
    }

    /// `sha256(pagare_template_hash || terms_hash || line || draw_index u16 LE || amount_mxn u64 LE
    /// || drawn_at i64 LE)`: what `draw` derives on chain, unchanged by decision 55.
    pub fn pagare_hash(template: &[u8; 32], terms: &[u8; 32], line: &[u8; 32], draw_index: u16, amount_mxn: u64, drawn_at: i64) -> [u8; 32] {
        sha256(&[template, terms, line, &draw_index.to_le_bytes(), &amount_mxn.to_le_bytes(), &drawn_at.to_le_bytes()])
    }

    /// Decision 55: the issuance entry states `sha256(pagare_hash || signed_pagare_hash)`.
    pub fn issuance_hash(pagare_hash: &[u8; 32], signed_pagare_hash: &[u8; 32]) -> [u8; 32] {
        sha256(&[pagare_hash, signed_pagare_hash])
    }

    /// `sha256(asset || kind || from || to || hash || stated_at i64 LE || entry_count u32 LE)`.
    pub fn entry_hash(asset: &[u8; 32], kind: u8, from: &[u8; 32], to: &[u8; 32], hash: &[u8; 32], stated_at: i64, count: u32) -> [u8; 32] {
        sha256(&[asset, &[kind], from, to, hash, &stated_at.to_le_bytes(), &count.to_le_bytes()])
    }

    /// The record's root after its first entry: `sha256(zero || entry)`.
    pub fn genesis_root(entry: &[u8; 32]) -> [u8; 32] {
        sha256(&[&[0u8; 32], entry])
    }

    #[test]
    fn the_pagare_hashes_on_pinned_vectors() {
        let hex = |h: [u8; 32]| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let line: [u8; 32] = core::array::from_fn(|i| i as u8 + 1);
        // Computed outside Rust (Python hashlib) before decision 55.
        assert_eq!(
            hex(pagare_hash(&[0x7e; 32], &[0x5a; 32], &line, 3, 123_456_789, 1_793_736_000)),
            "d89deb6ed82459435103e1ba364dbaed47a91f18a0617a57834f608643c96ade"
        );
        assert_eq!(
            hex(issuance_hash(&[0x11; 32], &[0x22; 32])),
            "5189c77d29fe5d546a045ec46986852785fea5c13ac7da9c115ff5fb6edf817c",
            "the vector clients/js/test/signed-pagare.test.ts pins"
        );
    }
}

/// Where decisions 56 and 57 put their fields, summed from the field sizes the specification lists,
/// discriminator first: not from the program's types.
pub mod layout {
    const fn sum(sizes: &[usize]) -> usize {
        let mut i = 0;
        let mut total = 0;
        while i < sizes.len() {
            total += sizes[i];
            i += 1;
        }
        total
    }

    /// Discriminator, council .. collection (6 keys), four u64, collection period u32, line cap
    /// u16, four u8, creator, council kind u8, multisig, vault bump u8, pending council and kind,
    /// outstanding cap u64, notice u32, impaired u64, provision u16, feeder, FIX u64, date u32,
    /// posted i64, realized i64, bounds 2 * u64, recoverable u64.
    pub const POOL_COTEJO_KEYS: usize =
        sum(&[8, 32 * 6, 8 * 4, 4, 2, 1, 1, 1, 1, 32, 1, 32, 1, 32, 1, 8, 4, 8, 2, 32, 8, 4, 8, 8, 8, 8, 8]);
    pub const POOL_LEN: usize = POOL_COTEJO_KEYS + 3 * 32 + 48;

    /// Discriminator, line, draw index u16, asset, pagaré hash, amount, issued, due, drawn through,
    /// certificate, method, status, paid at, receipt, procurador, flags, log root, count u32, last
    /// kind, bump, FIX u64, date u32, USDC u64, delivery hash, signed hash, signature (64), day u32.
    pub const PAGARE_COTEJO: usize =
        sum(&[8, 32, 2, 32, 32, 8, 8, 8, 8, 32, 1, 1, 8, 32, 32, 1, 32, 4, 1, 1, 8, 4, 8, 32, 32, 64, 4]);
    pub const PAGARE_SEALED_HASH: usize = PAGARE_COTEJO + 32;
    pub const PAGARE_LEN: usize = PAGARE_SEALED_HASH + 32 + 28;
}

/// Decision 56 written from its statement: who co-signs a draw. Keys are raw bytes; the zero key
/// is an empty slot.
pub mod cotejo {
    pub const ZERO: [u8; 32] = [0; 32];

    /// The key the pagaré records, or `Err` (`CotejoRequired`). `cotejo` is the optional account
    /// as `(key, signed)`. With no key set, whatever is passed is ignored and the record names the
    /// zero key; with any set, a signer whose key is one of the non-zero keys.
    pub fn co_signer(keys: &[[u8; 32]; 3], cotejo: Option<([u8; 32], bool)>) -> Result<[u8; 32], ()> {
        let set: Vec<[u8; 32]> = keys.iter().copied().filter(|k| *k != ZERO).collect();
        if set.is_empty() {
            return Ok(ZERO);
        }
        match cotejo {
            Some((key, true)) if set.contains(&key) => Ok(key),
            _ => Err(()),
        }
    }

    /// Whether `set_cotejo_keys` takes `keys`: no non-zero key twice, none equal to the pool, the
    /// council or the program.
    pub fn keys_valid(keys: &[[u8; 32]; 3], pool: [u8; 32], council: [u8; 32], program: [u8; 32]) -> bool {
        let set: Vec<[u8; 32]> = keys.iter().copied().filter(|k| *k != ZERO).collect();
        let distinct = set.iter().enumerate().all(|(i, k)| !set[..i].contains(k));
        distinct && set.iter().all(|k| *k != pool && *k != council && *k != program)
    }
}
