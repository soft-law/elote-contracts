//! Account layouts.

use anchor_lang::prelude::*;

pub mod seeds {
    /// `[POOL, creator, asset_mint]`
    pub const POOL: &[u8] = b"pool";
    /// `[LINE, pool, line_id (u64 LE)]`
    pub const LINE: &[u8] = b"line";
    /// `[VAULT, pool]`: the pool's asset token account.
    pub const VAULT: &[u8] = b"vault";
    /// `[DEAD, pool]`: the pool-owned share token account holding the dead shares.
    pub const DEAD: &[u8] = b"dead";
    /// `[PAGARE, line, draw_index (u16 LE)]`: the program's record of one pagaré.
    pub const PAGARE: &[u8] = b"pagare";
    /// `[PAGARE_ASSET, line, draw_index (u16 LE)]`: the Metaplex Core asset that is the pagaré.
    pub const PAGARE_ASSET: &[u8] = b"pagare_asset";
    /// `[PROCURADOR, pagare, procurador]`: the program-signed authority of a pagaré's `BurnDelegate`
    /// during an endoso en procuración; nobody holds a key for it.
    pub const PROCURADOR: &[u8] = b"procurador";
}

/// Shares the first deposit locks forever in the pool-owned dead account, so the supply never
/// returns below this floor and a donation cannot round later deposits down to nothing.
pub const DEAD_SHARES: u64 = 1_000;
/// The first deposit is at least this many times `DEAD_SHARES`, whatever the asset's decimals,
/// so the inflation attack keeps needing a donation of about a thousand times the victim's deposit.
pub const MIN_FIRST_DEPOSIT_DEAD_RATIO: u64 = 1_000;

/// Smallest first deposit for an asset with `decimals`: one whole unit (`10^decimals`), and never
/// fewer base units than `MIN_FIRST_DEPOSIT_DEAD_RATIO * DEAD_SHARES`, so the opening price of a
/// share cannot be set by a dust deposit on any asset. `None` when `10^decimals` overflows.
pub fn min_first_deposit(decimals: u8) -> Option<u64> {
    let one_unit = 10u64.checked_pow(u32::from(decimals))?;
    Some(one_unit.max(MIN_FIRST_DEPOSIT_DEAD_RATIO.checked_mul(DEAD_SHARES)?))
}
pub const BPS: u64 = 10_000;
/// Upper bound on the tranches of one line.
pub const MAX_DRAWS: u16 = 64;
/// `CreditLine::sponsor_role`: a sponsor that refers, trains and shares data, with no payment
/// obligation.
pub const SPONSOR_PROMOTER: u8 = 0;
/// `CreditLine::sponsor_role`: a sponsor that takes full liability. Recorded only; the program
/// enforces nothing on it.
pub const SPONSOR_GUARANTOR: u8 = 1;
/// Upper bound on the installments of one line, so the arrears computation is bounded.
pub const MAX_INSTALLMENTS: u16 = 120;
/// The fewest missed installments a schedule may set before collection.
pub const MIN_MAX_MISSED: u8 = 3;
/// The shortest collection period a pool may set: 30 days.
pub const MIN_COLLECTION_PERIOD_SECONDS: u32 = 30 * 86_400;
/// The shortest cancellation delay a pool may set: seven days.
pub const MIN_CANCEL_DELAY_SECONDS: u32 = 7 * 86_400;
/// The year that annualizes `late_bps`: 365 days.
pub const SECONDS_PER_YEAR: u64 = 365 * 86_400;
/// `CreditLine::currency`: Mexican pesos (ISO 4217 484). A line's amounts are centavos.
pub const CURRENCY_MXN: u16 = 484;
/// The Squads v4 program whose multisig vaults may be a pool's council.
pub const SQUADS_PROGRAM_ID: Pubkey = pubkey!("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
/// `Pool::council_kind`: the vault (index 0) of an autonomous Squads v4 multisig with a majority
/// threshold, checked on every council instruction.
pub const COUNCIL_SQUADS: u8 = 0;
/// `Pool::council_kind`: any signer, for a future governance PDA; `accept_council` refuses it
/// until a verified kind exists.
pub const COUNCIL_GENERIC: u8 = 1;
/// The most members a council multisig may have, so the check's cost stays bounded.
pub const MAX_COUNCIL_MEMBERS: usize = 10;
/// How long a requested line waits for the council: `approve_line` refuses after
/// `requested_at + REQUEST_TTL_SECONDS`.
pub const REQUEST_TTL_SECONDS: i64 = 15 * 86_400;
/// The shortest notice a pool may set between `propose_acceleration` and `accelerate_line`.
pub const MIN_ACCELERATION_NOTICE_SECONDS: u32 = 7 * 86_400;
/// The shortest wait a pool may set from a pagaré's payment before its borrower may cancel it
/// alone.
pub const MIN_CANCEL_GRACE_SECONDS: u32 = 30 * 86_400;
#[cfg(all(feature = "devnet", feature = "localnet"))]
compile_error!("the cluster features `devnet` and `localnet` are exclusive");

/// The asset mints `init_pool` accepts (R2). Without a cluster feature, the mainnet list: a build
/// that forgets its feature stays on the strictest list.
#[cfg(not(any(feature = "devnet", feature = "localnet")))]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = Some(&[
    pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"), // USDC
    pubkey!("MEXeQE5UjuusUoyS2KxbUXp6o21YbbBJaNvPMMEfr4y"), // MEXe (Etherfuse)
]);
/// Devnet: the Etherfuse sandbox mints and Circle's devnet USDC.
#[cfg(feature = "devnet")]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = Some(&[
    pubkey!("MEXeDZ8UKQUzKK3yXcXFjHwCG2AAigdSTnaXPuxxchK"), // MEXe, Etherfuse sandbox
    pubkey!("4D4r1KuS9WxbPCzzAJnWtYUJSSzDEZzrF1CbwajWGVjg"), // USDC, Etherfuse sandbox
    pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"), // USDC, Circle
]);
/// Localnet (the tests and the local e2e): any mint the asset policy accepts.
#[cfg(feature = "localnet")]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = None;

/// Whether `mint` is on this build's `ASSET_ALLOWLIST`.
pub fn asset_listed(mint: &Pubkey) -> bool {
    ASSET_ALLOWLIST.is_none_or(|list| list.contains(mint))
}

/// `Pool::asset_controls` bit: the asset mint has a freeze authority.
pub const ASSET_CONTROL_FREEZE: u8 = 1 << 0;
/// `Pool::asset_controls` bit: the asset mint carries the Token-2022 `Pausable` extension.
pub const ASSET_CONTROL_PAUSABLE: u8 = 1 << 1;

#[account]
#[derive(InitSpace)]
pub struct Pool {
    /// The vault of the council multisig: signs every council instruction.
    pub council: Pubkey,
    /// The agreements engine's key: co-signs `draw`, `endorse_pagare` and
    /// `court_annotation`, optionally `cancel_pagare` (else the backstop), and may sign
    /// `annotate_payment` alone. Never the council; rotated with `set_attestor`.
    pub attestor: Pubkey,
    pub asset_mint: Pubkey,
    /// The share token mint; its mint authority is the pool PDA.
    pub share_mint: Pubkey,
    /// PDA token account `[VAULT, pool]` for `asset_mint`, owned by the pool PDA.
    pub vault: Pubkey,
    /// PDA share token account `[DEAD, pool]`, owned by the pool PDA; never burned.
    pub dead_shares: Pubkey,
    /// The series' Metaplex Core collection `draw` mints the pagaré assets into.
    pub collection: Pubkey,
    /// Principal drawn and neither repaid nor written off.
    pub outstanding_principal: u64,
    /// Ordinary interest repaid on every line; informative.
    pub interest_collected: u64,
    /// Principal written off on every line; informative.
    pub written_off: u64,
    pub line_count: u64,
    /// Seconds after a due date (or the term) before late interest accrues and collection or a
    /// write-off is allowed; zero means none. Fixed at `init_pool`.
    pub grace_period_seconds: u32,
    /// Seconds from the mark (`CreditLine::accelerated_at`) before a line in collection can be
    /// written off; at least `MIN_COLLECTION_PERIOD_SECONDS`. Set with `set_collection_period`.
    pub collection_period_seconds: u32,
    /// Seconds from a cancellation request before the owner may cancel without the attestor; at
    /// least `MIN_CANCEL_DELAY_SECONDS`. Set with `set_cancel_delay`; a request stores its own
    /// deadline, so a change never moves one.
    pub cancel_delay_seconds: u32,
    /// The largest line `approve_line` accepts, as a share of the NAV at the approval, in basis points
    /// (`1..=BPS`). Fixed at `init_pool`.
    pub max_line_bps: u16,
    /// `ASSET_CONTROL_*` bits: issuer controls of the asset mint that can stop the vault without
    /// the program. Recorded at `init_pool`, never refused.
    pub asset_controls: u8,
    /// The share mint is `DefaultAccountState = Frozen`: a holder's account must be thawed by the
    /// mint's freeze authority before it can receive or burn shares. Recorded at `init_pool`.
    pub permissioned: bool,
    pub paused: bool,
    pub bump: u8,
    /// Paid for and seeds the pool; no other power.
    pub creator: Pubkey,
    /// `COUNCIL_SQUADS` or `COUNCIL_GENERIC`.
    pub council_kind: u8,
    /// The Squads multisig whose vault is the council; the zero key under `COUNCIL_GENERIC`.
    pub multisig: Pubkey,
    /// The bump of the council's vault PDA `["multisig", multisig, "vault", 0]`.
    pub council_vault_bump: u8,
    /// The authority `propose_council` named, until it signs `accept_council`; the zero key when
    /// none.
    pub pending_council: Pubkey,
    pub pending_council_kind: u8,
    /// A key that may only pause the pool, beside every council member; the zero key when none.
    pub guardian: Pubkey,
    /// The most principal the pool lends at once, in asset units; checked at every draw.
    pub max_outstanding_principal: u64,
    /// Seconds from `propose_acceleration` before `accelerate_line`; at least
    /// `MIN_ACCELERATION_NOTICE_SECONDS`. A proposal stores its own deadline.
    pub acceleration_notice_seconds: u32,
    /// Seconds from a pagaré's payment before its borrower may cancel it alone; at least
    /// `MIN_CANCEL_GRACE_SECONDS`.
    pub cancel_grace_seconds: u32,
    /// Principal of the lines in collection, for a provision at the mark; zero, not yet used.
    pub impaired_principal: u64,
    /// The provision on `impaired_principal`, in basis points; zero, not yet used.
    pub provision_bps: u16,
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 64],
}

impl Pool {
    /// Bit 0 of `asset_controls`: the asset's issuer can freeze the vault.
    pub fn asset_freezable(&self) -> bool {
        self.asset_controls & ASSET_CONTROL_FREEZE != 0
    }

    /// Bit 1 of `asset_controls`: the asset's issuer can pause every transfer of the mint.
    pub fn asset_pausable(&self) -> bool {
        self.asset_controls & ASSET_CONTROL_PAUSABLE != 0
    }

    /// `nav * max_line_bps / BPS`, rounded down: the largest line the pool opens at this NAV.
    pub fn max_line_principal(&self, nav: u64) -> Option<u64> {
        (nav as u128)
            .checked_mul(self.max_line_bps as u128)?
            .checked_div(BPS as u128)
            .and_then(|v| u64::try_from(v).ok())
    }

    /// Invariants every money instruction re-checks on balances reloaded after its CPIs: the NAV
    /// fits a `u64`, and a given share supply is zero or at least `DEAD_SHARES` (the dead shares
    /// never move). `InvariantBroken` otherwise. Returns the NAV it checked.
    pub fn check(&self, vault_amount: u64, share_supply: Option<u64>) -> Result<u64> {
        let nav = vault_amount
            .checked_add(self.outstanding_principal)
            .ok_or(crate::errors::PoolError::InvariantBroken)?;
        if let Some(supply) = share_supply {
            require!(supply == 0 || supply >= DEAD_SHARES, crate::errors::PoolError::InvariantBroken);
        }
        Ok(nav)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum LineStatus {
    /// Requested by the borrower, waiting for the council's `approve_line` or `reject_line`.
    Requested,
    Approved,
    Active,
    Repaid,
    WrittenOff,
    /// A scheduled line with `max_missed` or more missed installments, in out-of-court
    /// collection. Repayments are still accepted; it leaves this state only when repaid in full
    /// or written off.
    InCollection,
    /// An approved commitment nobody drew, cleared by the council (`cancel_line`). Terminal; nothing
    /// was ever owed.
    Cancelled,
}

#[account]
#[derive(InitSpace)]
pub struct CreditLine {
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    /// Co-signed `request_line`; `None` when the line has no sponsor.
    pub sponsor: Option<Pubkey>,
    /// `SPONSOR_PROMOTER` or `SPONSOR_GUARANTOR`; meaningful only when `sponsor` is named.
    pub sponsor_role: u8,
    /// Hash of the signed credit agreement.
    pub agreement_hash: [u8; 32],
    /// Salted merkle root of the credit file in the agreements engine. A commitment: no document,
    /// name or folio is derivable from it.
    pub expediente_root: [u8; 32],
    pub status: LineStatus,
    /// The approved commitment; `available = principal - drawn`.
    pub principal: u64,
    /// Annual ordinary interest rate, in basis points, charged on each tranche from its draw to
    /// the last due date.
    pub rate_bps: u16,
    /// The maturity of a bullet line.
    pub term_end: i64,
    /// Last instant a draw is accepted (strict `<`): `first_due_at` on a scheduled line,
    /// `term_end` on a bullet line; `suspend_draws` brings it to now.
    pub availability_end: i64,
    /// Principal transferred to the borrower so far; `drawn <= principal`, never decreases.
    pub drawn: u64,
    /// Tranches drawn; at most `MAX_DRAWS`.
    pub draws: u16,
    /// Ordinary interest owed: each draw adds `amount * rate_bps * (last_due - now) / (BPS *
    /// SECONDS_PER_YEAR)`; `mark_in_collection` cuts it to the share of the installments then due.
    pub interest_due: u64,
    pub principal_repaid: u64,
    pub interest_repaid: u64,
    /// The index of the oldest pagaré no repayment has marked paid. `repay` requires the records
    /// `[pagares_paid ..= last_touched]` and moves it.
    pub pagares_paid: u16,
    /// The first due date; installment `k` is due at `first_due_at + (k - 1) * interval_seconds`.
    /// Zero on a bullet line.
    pub first_due_at: i64,
    pub interval_seconds: i64,
    /// Installments of the schedule, `1..=MAX_INSTALLMENTS`; zero on a bullet line.
    pub installments: u16,
    /// Annual late interest rate on the overdue principal, in basis points.
    pub late_bps: u16,
    /// Missed installments that allow `mark_in_collection`.
    pub max_missed: u8,
    pub late_collected: u64,
    /// Late interest is settled up to this instant; zero until a repayment collects some.
    pub late_paid_until: i64,
    /// Time of the first draw; informational (the calendar is `first_due_at`).
    pub drawn_at: i64,
    pub closed_at: i64,
    /// When `mark_in_collection` accelerated the balance; zero before. From then on every
    /// installment is due, and `write_off` waits `Pool::collection_period_seconds` from it.
    pub accelerated_at: i64,
    pub bump: u8,
    /// ISO 4217 code of every amount of the line: `CURRENCY_MXN`.
    pub currency: u16,
    pub requested_at: i64,
    pub approved_at: i64,
    /// `sha256` of the stored terms in the fixed encoding; the council's approval names it.
    pub terms_hash: [u8; 32],
    /// Hash of the approved pagaré text every draw's pagaré hash is derived from.
    pub pagare_template_hash: [u8; 32],
    /// Funded the request; receives the rent back on withdraw, reject or expiry.
    pub payer: Pubkey,
    /// `Σ amount × drawn_at` over the draws: a bullet line's interest accrued at a mark is
    /// `rate × (drawn × mark − draw_time_weight)`.
    pub draw_time_weight: u128,
    /// The `availability_end` before `suspend_draws` or `propose_acceleration` moved it, which
    /// `resume_draws` restores; zero when draws are not suspended.
    pub availability_end_saved: i64,
    /// When `accelerate_line` may fire, set by `propose_acceleration`; zero when none is pending.
    pub acceleration_due_at: i64,
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 64],
}

/// How the pagaré text was signed: the borrower's wallet alone.
pub const SIGNATURE_WALLET: u8 = 0;
/// A PSC (prestador de servicios de certificación) certificate and the wallet.
pub const SIGNATURE_PSC_WALLET: u8 = 1;
/// The SAT e.firma and the wallet.
pub const SIGNATURE_EFIRMA_WALLET: u8 = 2;

/// `Pagare::status`: the título is outstanding (the asset exists, held by the pool or endorsed).
pub const PAGARE_OUTSTANDING: u8 = 0;
/// `Pagare::status`: paid and extinguished by `cancel_pagare`; the asset is burnt and the record
/// stays as the PAGADO mark.
pub const PAGARE_PAID: u8 = 1;
/// `Pagare::status`: a repayment covered it (the PAGADO mark); `cancel_pagare` must still burn it.
pub const PAGARE_PAID_PENDING_CANCEL: u8 = 2;

/// `Pagare::flags` bit: the asset carries the owner-managed `BurnDelegate` plugin `endorse_pagare`
/// adds at the first procuración, so a later one approves its authority instead of adding it again.
pub const PAGARE_FLAG_BURN_DELEGATE: u8 = 1 << 0;
/// `Pagare::flags` bit: an endoso en procuración is in force.
pub const PAGARE_FLAG_PROCURACION: u8 = 1 << 1;

/// Registry entry kinds (`Pagare::last_kind`, `PagareEntry::kind`).
// Codes 1 and 8 are reserved for the endoso en propiedad and the late endoso; never reuse them.
pub const KIND_EMISION: u8 = 0;
pub const KIND_PROCURACION: u8 = 2;
pub const KIND_REVOCACION: u8 = 3;
pub const KIND_TERMINACION: u8 = 4;
pub const KIND_ABONO: u8 = 5;
pub const KIND_ANOTACION_JUDICIAL: u8 = 6;
pub const KIND_CONSTANCIA_JUDICIAL: u8 = 7;
pub const KIND_PAGADO: u8 = 9;
pub const KIND_SOLICITUD_CANCELACION: u8 = 10;
pub const KIND_RECIBO: u8 = 11;

/// The program's record of one pagaré, beside the Core asset `draw` mints. It holds no folio and
/// nothing personal, and is never closed: after `cancel_pagare` it stays as the PAGADO mark.
#[account]
#[derive(InitSpace)]
pub struct Pagare {
    pub line: Pubkey,
    /// Which tranche of the line, 0-based.
    pub draw_index: u16,
    /// The Core asset at `[PAGARE_ASSET, line, draw_index]`.
    pub asset: Pubkey,
    /// Hash of the signed pagaré data message, as `draw` received it.
    pub pagare_hash: [u8; 32],
    /// The tranche this pagaré documents.
    pub amount: u64,
    pub issued_at: i64,
    /// The line's last due date at issuance.
    pub due_at: i64,
    /// The line's `drawn` after this tranche. Pagarés are covered oldest first, so pagaré k is
    /// paid once the line's `principal_repaid` reaches it.
    pub drawn_through: u64,
    /// Hash of the PSC certificate that signed the pagaré text; zero only when
    /// `signature_method` is `SIGNATURE_WALLET`.
    pub cert_fingerprint: [u8; 32],
    /// `SIGNATURE_*`.
    pub signature_method: u8,
    /// `PAGARE_OUTSTANDING`, `PAGARE_PAID_PENDING_CANCEL` or `PAGARE_PAID`; never reverts.
    pub status: u8,
    /// When the repayment that covered it landed; zero while outstanding.
    pub paid_at: i64,
    /// The receipt hash of that repayment: the payer's, until the holder annotates the real one
    /// (`annotate_payment`, before the burn).
    pub receipt_hash: [u8; 32],
    /// The receipt hash `cancel_pagare` was given; zero until the burn.
    pub cancel_receipt_hash: [u8; 32],
    /// Deadline of the pending cancellation request, after which the owner may cancel without the
    /// attestor; zero when none.
    pub cancel_after: i64,
    /// The receipt hash of that request.
    pub requested_receipt_hash: [u8; 32],
    /// The procurador of the endoso en procuración in force; the zero key when none. Cleared by
    /// `revoke_procuracion` and by the extinction.
    pub procurador: Pubkey,
    /// `PAGARE_FLAG_*` bits.
    pub flags: u8,
    /// The registry: `sha256(log_root || entry_hash)` over every entry, the issuance entry first.
    pub log_root: [u8; 32],
    /// Entries in the chain; the issuance entry is 1.
    pub entry_count: u32,
    /// `KIND_*` of the last entry.
    pub last_kind: u8,
    pub bump: u8,
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 32],
}

/// Installment terms requested by `request_line`; fixed for the life of the line.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleParams {
    /// The first due date (unix seconds), from the signed contract; must be after the opening.
    pub first_due_at: i64,
    pub interval_seconds: i64,
    /// `drawn + interest_due` is paid in this many installments.
    pub installments: u16,
    /// Annual late interest rate on the overdue principal, in basis points.
    pub late_bps: u16,
    /// Missed installments after which anyone may move the line to collection.
    pub max_missed: u8,
}

/// The schedule terms of a scheduled line, read from its fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Schedule {
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
}

impl CreditLine {
    /// The installment schedule; `None` on a bullet line (`installments == 0`).
    pub fn schedule(&self) -> Option<Schedule> {
        (self.installments > 0).then_some(Schedule {
            interval_seconds: self.interval_seconds,
            installments: self.installments,
            late_bps: self.late_bps,
            max_missed: self.max_missed,
        })
    }

    /// The instant one interval before the first due date, from which `math` counts the
    /// installments (`anchor + k * interval`): the single place the calendar is anchored. `None`
    /// on a bullet line.
    pub fn anchor(&self) -> Option<i64> {
        let s = self.schedule()?;
        self.first_due_at.checked_sub(s.interval_seconds)
    }

    /// When the last payment is owed: `first_due_at + (installments - 1) * interval` on a scheduled
    /// line, `term_end` on a bullet line. Each tranche's interest runs from its draw to this instant.
    pub fn last_due(&self) -> Option<i64> {
        match self.schedule() {
            Some(s) => s
                .interval_seconds
                .checked_mul(i64::from(s.installments).checked_sub(1)?)
                .and_then(|span| self.first_due_at.checked_add(span)),
            None => Some(self.term_end),
        }
    }

    /// `principal - drawn`.
    pub fn available(&self) -> Option<u64> {
        self.principal.checked_sub(self.drawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(outstanding_principal: u64) -> Pool {
        Pool {
            council: Pubkey::default(),
            attestor: Pubkey::default(),
            asset_mint: Pubkey::default(),
            share_mint: Pubkey::default(),
            vault: Pubkey::default(),
            dead_shares: Pubkey::default(),
            collection: Pubkey::default(),
            outstanding_principal,
            interest_collected: 0,
            written_off: 0,
            line_count: 0,
            grace_period_seconds: 0,
            collection_period_seconds: 0,
            cancel_delay_seconds: 0,
            max_line_bps: 0,
            asset_controls: 0,
            permissioned: false,
            paused: false,
            bump: 0,
            creator: Pubkey::default(),
            council_kind: COUNCIL_SQUADS,
            multisig: Pubkey::default(),
            council_vault_bump: 0,
            pending_council: Pubkey::default(),
            pending_council_kind: 0,
            guardian: Pubkey::default(),
            max_outstanding_principal: 0,
            acceleration_notice_seconds: 0,
            cancel_grace_seconds: 0,
            impaired_principal: 0,
            provision_bps: 0,
            reserved: [0; 64],
        }
    }

    #[test]
    fn check_accepts_a_consistent_pool_and_refuses_a_broken_one() {
        let p = pool(5);
        assert!(p.check(10, None).is_ok(), "the NAV fits");
        assert!(p.check(10, Some(0)).is_ok(), "nothing deposited yet");
        assert!(p.check(10, Some(DEAD_SHARES)).is_ok(), "the dead shares alone are a valid floor");
        assert!(p.check(10, Some(DEAD_SHARES + 1)).is_ok());
        assert!(p.check(10, Some(DEAD_SHARES - 1)).is_err(), "a supply below the dead shares is impossible");
        assert!(p.check(10, Some(1)).is_err());
        assert!(p.check(u64::MAX, None).is_err(), "vault + outstanding overflows");
        assert!(pool(0).check(u64::MAX, Some(DEAD_SHARES)).is_ok(), "the maximum NAV itself is fine");
    }

    #[test]
    #[cfg(not(any(feature = "devnet", feature = "localnet")))]
    fn without_a_cluster_feature_only_mainnet_usdc_and_mexe_are_listed() {
        assert!(asset_listed(&pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")));
        assert!(asset_listed(&pubkey!("MEXeQE5UjuusUoyS2KxbUXp6o21YbbBJaNvPMMEfr4y")));
        assert!(!asset_listed(&pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")), "devnet USDC is not mainnet's");
        assert!(!asset_listed(&Pubkey::new_unique()));
    }

    #[test]
    #[cfg(feature = "devnet")]
    fn devnet_lists_the_sandbox_mints_and_circles_usdc() {
        assert!(asset_listed(&pubkey!("MEXeDZ8UKQUzKK3yXcXFjHwCG2AAigdSTnaXPuxxchK")));
        assert!(asset_listed(&pubkey!("4D4r1KuS9WxbPCzzAJnWtYUJSSzDEZzrF1CbwajWGVjg")));
        assert!(asset_listed(&pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")));
        assert!(!asset_listed(&pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")));
        assert!(!asset_listed(&Pubkey::new_unique()));
    }

    #[test]
    #[cfg(feature = "localnet")]
    fn localnet_lists_any_mint() {
        assert!(asset_listed(&Pubkey::new_unique()));
    }

    #[test]
    fn min_first_deposit_is_one_unit_with_a_dead_shares_floor() {
        assert_eq!(min_first_deposit(6), Some(1_000_000));
        assert_eq!(min_first_deposit(9), Some(1_000_000_000));
        assert_eq!(min_first_deposit(2), Some(1_000_000), "the floor, not 100");
        assert_eq!(min_first_deposit(0), Some(1_000_000));
        assert_eq!(min_first_deposit(19), Some(10u64.pow(19)));
        assert_eq!(min_first_deposit(20), None, "10^20 does not fit a u64");
    }
}
