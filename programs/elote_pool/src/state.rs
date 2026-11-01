//! Account layouts.

use anchor_lang::prelude::*;

pub mod seeds {
    /// `[POOL, admin, asset_mint]`
    pub const POOL: &[u8] = b"pool";
    /// `[LINE, pool, line_id (u64 LE)]`
    pub const LINE: &[u8] = b"line";
    /// `[VAULT, pool]`: the pool's asset token account.
    pub const VAULT: &[u8] = b"vault";
    /// `[DEAD, pool]`: the pool-owned share token account holding the dead shares.
    pub const DEAD: &[u8] = b"dead";
}

/// Shares the first deposit locks forever in the pool-owned dead account, so the supply never
/// returns below this floor and a donation cannot round later deposits down to nothing.
pub const DEAD_SHARES: u64 = 1_000;
/// The first deposit is at least this many times `DEAD_SHARES`, whatever the asset's decimals,
/// so the inflation attack keeps needing a donation of about a thousand times the victim's
/// deposit (the ratio the original 1_000_000 minimum on a 6-decimal asset gave).
pub const MIN_FIRST_DEPOSIT_DEAD_RATIO: u64 = 1_000;

/// Smallest first deposit for an asset with `decimals`: one whole unit (`10^decimals`), and never
/// fewer base units than `MIN_FIRST_DEPOSIT_DEAD_RATIO * DEAD_SHARES`, so the opening price of a
/// share cannot be set by a dust deposit on any asset. `None` when `10^decimals` overflows.
pub fn min_first_deposit(decimals: u8) -> Option<u64> {
    let one_unit = 10u64.checked_pow(u32::from(decimals))?;
    Some(one_unit.max(MIN_FIRST_DEPOSIT_DEAD_RATIO.checked_mul(DEAD_SHARES)?))
}
pub const BPS: u64 = 10_000;
/// Pool v2 (2026-10-31): token interface, asset and share mint policies, `attestor` and
/// `collection`. Version 1 pools (the devnet demo) do not deserialize under this layout.
pub const POOL_VERSION: u8 = 2;
/// Line v3 (2026-10-31, multi-draw plan and decision 25): every line the program opens, bullet
/// or scheduled. Tranched draws, an annual `rate_bps` charged per tranche, a payment calendar
/// fixed by the signed contract (`first_due_at`), one repayment order (late interest, interest,
/// principal) with clamped overpayment. Version 1 and 2 lines do not deserialize under this
/// layout.
pub const LINE_VERSION: u8 = 3;
/// Upper bound on the tranches of one line.
pub const MAX_DRAWS: u16 = 64;
/// `CreditLine::sponsor_role`: a public sponsor that refers, trains and shares data, with no
/// payment obligation (decision 19).
pub const SPONSOR_PROMOTER: u8 = 0;
/// `CreditLine::sponsor_role`: a private sponsor (or development banking through a SOFOM) with
/// full liability. The program enforces nothing on it; it is the on-chain record of the promise.
pub const SPONSOR_GUARANTOR: u8 = 1;
/// Upper bound on the installments of one line, so the arrears computation is bounded.
pub const MAX_INSTALLMENTS: u16 = 120;
/// The fewest missed installments a schedule may set before collection: the owner's policy is
/// three months of arrears.
pub const MIN_MAX_MISSED: u8 = 3;
/// The year that annualizes `late_bps`: 365 days.
pub const SECONDS_PER_YEAR: u64 = 365 * 86_400;
/// `Pool::asset_controls` bit: the asset mint has a freeze authority.
pub const ASSET_CONTROL_FREEZE: u8 = 1 << 0;
/// `Pool::asset_controls` bit: the asset mint carries the Token-2022 `Pausable` extension.
pub const ASSET_CONTROL_PAUSABLE: u8 = 1 << 1;

#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub version: u8,
    pub admin: Pubkey,
    pub asset_mint: Pubkey,
    /// PDA token account `[VAULT, pool]` for `asset_mint`, owned by the pool PDA.
    pub vault: Pubkey,
    /// The share token mint; its mint authority is the pool PDA.
    pub share_mint: Pubkey,
    /// PDA share token account `[DEAD, pool]`, owned by the pool PDA; never burned.
    pub dead_shares: Pubkey,
    /// Principal drawn and neither repaid nor written off.
    pub outstanding_principal: u64,
    /// Informative running total.
    pub interest_collected: u64,
    /// Informative running total.
    pub written_off: u64,
    pub line_count: u64,
    pub paused: bool,
    /// Pool parameters packed into spare bytes, so the account layout does not change:
    ///
    /// | bytes   | field                  | type     | meaning                                   |
    /// |---------|------------------------|----------|-------------------------------------------|
    /// | 0..4    | `grace_period_seconds` | u32 LE   | grace after a due date; zero means none   |
    /// | 4..6    | `max_line_bps`         | u16 LE   | largest line as a share of the NAV, in bps |
    /// | 6       | `permissioned`         | u8 flag  | 1: shares are frozen by default (Token ACL) |
    /// | 7       | `asset_controls`       | u8 bits  | bit 0: asset freeze authority; bit 1: Pausable |
    /// | 8..32   | unused                 | zero     |                                           |
    ///
    /// Read and written only through the accessors below (`grace_period_seconds`,
    /// `max_line_bps`, `permissioned`, `asset_controls` and their setters).
    pub reserved: [u8; 32],
    pub bump: u8,
    // Pool v2 (decision 24): appended after `bump` so every v1 offset holds.
    /// The agreements engine's key. It co-signs `open_line`, `draw` and `endorse_pagare`,
    /// attesting that the signed document behind the hash exists and matches the arguments.
    /// Rotated by the admin with `set_attestor`; never the zero key.
    pub attestor: Pubkey,
    /// The series' Metaplex Core collection, in which `draw` mints one pagaré asset per
    /// disposición. Recorded at `init_pool`; validated as a Core collection from step 6 of the
    /// pool v2 batch.
    pub collection: Pubkey,
}

impl Pool {
    /// Seconds after a due date (or after the term) before late interest accrues and before a
    /// write-off is allowed. Fixed at `init_pool`.
    pub fn grace_period_seconds(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.reserved[0..4]);
        u32::from_le_bytes(b)
    }

    /// The largest principal `open_line` accepts, as a share of the NAV at the opening, in basis
    /// points (`1..=BPS`). Fixed at `init_pool`; the cap binds the commitment, not a tranche.
    pub fn max_line_bps(&self) -> u16 {
        let mut b = [0u8; 2];
        b.copy_from_slice(&self.reserved[4..6]);
        u16::from_le_bytes(b)
    }

    pub fn set_max_line_bps(&mut self, bps: u16) {
        self.reserved[4..6].copy_from_slice(&bps.to_le_bytes());
    }

    /// Shares are permissioned: the share mint has `DefaultAccountState = Frozen`, so a holder's
    /// account must be thawed by the mint's freeze authority (Token ACL) before it can receive
    /// or burn shares. Read-only fact recorded at `init_pool`.
    pub fn permissioned(&self) -> bool {
        self.reserved[6] == 1
    }

    pub fn set_permissioned(&mut self, v: bool) {
        self.reserved[6] = u8::from(v);
    }

    /// The issuer controls the asset mint carries that can stop the vault without touching the
    /// program, as bits (decision 15: accepted and recorded, never rejected): bit 0, a freeze
    /// authority (MEXe has one, CETES none); bit 1, the Token-2022 `Pausable` extension.
    /// Read-only facts recorded at `init_pool`.
    pub fn asset_controls(&self) -> u8 {
        self.reserved[7]
    }

    pub fn set_asset_controls(&mut self, bits: u8) {
        self.reserved[7] = bits;
    }

    /// Bit 0 of `asset_controls`: the asset's issuer can freeze the vault.
    pub fn asset_freezable(&self) -> bool {
        self.asset_controls() & ASSET_CONTROL_FREEZE != 0
    }

    /// Bit 1 of `asset_controls`: the asset's issuer can pause every transfer of the mint.
    pub fn asset_pausable(&self) -> bool {
        self.asset_controls() & ASSET_CONTROL_PAUSABLE != 0
    }

    /// `nav * max_line_bps / BPS`, rounded down: the largest line the pool opens at this NAV.
    pub fn max_line_principal(&self, nav: u64) -> Option<u64> {
        (nav as u128)
            .checked_mul(self.max_line_bps() as u128)?
            .checked_div(BPS as u128)
            .and_then(|v| u64::try_from(v).ok())
    }

    /// The invariants every money instruction re-checks before it returns (R-25), on balances
    /// reloaded after the CPIs: `vault + outstanding_principal` fits a `u64` (the NAV exists),
    /// and, where the share mint is an account of the instruction, its supply is zero (nothing
    /// deposited yet) or at least `DEAD_SHARES` (the dead account is never burned). A failure
    /// means state the program considers impossible; the transaction is refused. Returns the
    /// NAV, so the caller reports the figure it checked.
    pub fn check(&self, vault_amount: u64, share_supply: Option<u64>) -> Result<u64> {
        let nav = vault_amount
            .checked_add(self.outstanding_principal)
            .ok_or(crate::errors::PoolError::InvariantBroken)?;
        if let Some(supply) = share_supply {
            require!(supply == 0 || supply >= DEAD_SHARES, crate::errors::PoolError::InvariantBroken);
        }
        Ok(nav)
    }

    pub fn set_grace_period_seconds(&mut self, seconds: u32) {
        self.reserved[0..4].copy_from_slice(&seconds.to_le_bytes());
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum LineStatus {
    Approved,
    Active,
    Repaid,
    WrittenOff,
    /// A scheduled line with `max_missed` or more missed installments, in out-of-court
    /// collection. Repayments are still accepted; it leaves this state only when repaid in full
    /// or written off.
    InCollection,
    /// An approved commitment nobody drew, cleared by the admin (`cancel_line`, decision 25).
    /// Terminal; nothing was ever owed.
    Cancelled,
}

#[account]
#[derive(InitSpace)]
pub struct CreditLine {
    pub version: u8,
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    /// A public program that co-signed the opening; `None` for a commercial line.
    pub sponsor: Option<Pubkey>,
    /// Hash of the signed credit agreement.
    pub agreement_hash: [u8; 32],
    /// The approved commitment; `available = principal - drawn`.
    pub principal: u64,
    /// Annual ordinary interest rate, in basis points, charged on each tranche from its draw to
    /// the last due date (decision 9).
    pub rate_bps: u16,
    /// Ordinary interest charged so far: the sum over tranches of
    /// `amount * rate_bps * (last_due - drawn) / (BPS * SECONDS_PER_YEAR)`, each fixed at its
    /// draw. Zero at opening; it changes only at a draw.
    pub interest_due: u64,
    pub term_end: i64,
    pub principal_repaid: u64,
    pub interest_repaid: u64,
    pub status: LineStatus,
    pub opened_at: i64,
    /// Time of the first draw; informational since decision 25 (the calendar is `first_due_at`).
    pub drawn_at: i64,
    pub closed_at: i64,
    /// Zero on a bullet line (single maturity). On a scheduled line, the installment schedule,
    /// read and written only through [`CreditLine::schedule`] and [`CreditLine::set_schedule`]:
    ///
    /// | bytes  | field              | type    | meaning                                         |
    /// |--------|--------------------|---------|-------------------------------------------------|
    /// | 0..8   | `interval_seconds` | i64 LE  | seconds between due dates, from `drawn_at`      |
    /// | 8..10  | `installments`     | u16 LE  | number of installments, 1..=`MAX_INSTALLMENTS`  |
    /// | 10..12 | `late_bps`         | u16 LE  | annual late interest rate on overdue amounts    |
    /// | 12     | `max_missed`       | u8      | missed installments that allow collection       |
    /// | 13     | `flags`            | u8      | reserved, zero                                  |
    /// | 14..22 | `late_collected`   | u64 LE  | late interest collected so far                  |
    /// | 22..30 | `late_paid_until`  | i64 LE  | late interest is settled up to this time        |
    /// | 30..32 | unused             | zero    |                                                 |
    pub reserved: [u8; 32],
    pub bump: u8,
    // Line v3 (multi-draw plan, decisions 12, 15 and 25): appended after `bump` so every earlier
    // offset holds.
    /// Principal transferred to the borrower so far; `drawn <= principal`, never decreases.
    pub drawn: u64,
    /// Number of tranches drawn; at most `MAX_DRAWS`.
    pub draws: u16,
    /// Last instant a draw is accepted (strict `<`): `first_due_at` on a scheduled line,
    /// `term_end` on a bullet line, fixed at opening; `suspend_draws` brings it to now.
    pub availability_end: i64,
    /// The first due date, from the signed contract; installment `k` is due at
    /// `first_due_at + (k - 1) * interval`. Zero on a bullet line.
    pub first_due_at: i64,
    /// `SPONSOR_PROMOTER` or `SPONSOR_GUARANTOR`; meaningful only when `sponsor` is named.
    pub sponsor_role: u8,
    /// Salted merkle root of the credit file in the agreements engine (decision 15 of the pagaré
    /// design). A commitment: no document, name or folio is derivable from it.
    pub expediente_root: [u8; 32],
}

/// Installment terms requested by `open_line`; fixed for the life of the line.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleParams {
    /// The first due date (unix seconds), from the signed contract; must be after the opening.
    pub first_due_at: i64,
    /// Seconds between due dates; installment `k` is due at `first_due_at + (k - 1) * interval_seconds`.
    pub interval_seconds: i64,
    /// `drawn + interest_due` is paid in this many installments.
    pub installments: u16,
    /// Annual late interest rate on overdue amounts, in basis points.
    pub late_bps: u16,
    /// Missed installments after which anyone may move the line to collection.
    pub max_missed: u8,
}

/// The schedule of a version-2 line, unpacked from `CreditLine.reserved`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Schedule {
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
    pub flags: u8,
    pub late_collected: u64,
    /// Every repayment settles the late interest accrued until its own time; late interest
    /// accrues again only after this watermark. Zero until the first repayment.
    pub late_paid_until: i64,
}

impl Schedule {
    pub fn unpack(r: &[u8; 32]) -> Self {
        let mut i64b = [0u8; 8];
        let mut u16b = [0u8; 2];
        let mut u64b = [0u8; 8];
        i64b.copy_from_slice(&r[0..8]);
        let interval_seconds = i64::from_le_bytes(i64b);
        u16b.copy_from_slice(&r[8..10]);
        let installments = u16::from_le_bytes(u16b);
        u16b.copy_from_slice(&r[10..12]);
        let late_bps = u16::from_le_bytes(u16b);
        u64b.copy_from_slice(&r[14..22]);
        let late_collected = u64::from_le_bytes(u64b);
        i64b.copy_from_slice(&r[22..30]);
        let late_paid_until = i64::from_le_bytes(i64b);
        Self { interval_seconds, installments, late_bps, max_missed: r[12], flags: r[13], late_collected, late_paid_until }
    }

    pub fn pack(&self, r: &mut [u8; 32]) {
        r[0..8].copy_from_slice(&self.interval_seconds.to_le_bytes());
        r[8..10].copy_from_slice(&self.installments.to_le_bytes());
        r[10..12].copy_from_slice(&self.late_bps.to_le_bytes());
        r[12] = self.max_missed;
        r[13] = self.flags;
        r[14..22].copy_from_slice(&self.late_collected.to_le_bytes());
        r[22..30].copy_from_slice(&self.late_paid_until.to_le_bytes());
    }
}

impl CreditLine {
    /// The installment schedule of a scheduled line; `None` for a bullet line (whose `reserved`
    /// bytes are all zero, so `installments == 0`).
    pub fn schedule(&self) -> Option<Schedule> {
        let s = Schedule::unpack(&self.reserved);
        (s.installments > 0).then_some(s)
    }

    pub fn set_schedule(&mut self, s: &Schedule) {
        s.pack(&mut self.reserved);
    }

    /// The instant one interval before the first due date, from which `math` counts the
    /// installments (`anchor + k * interval`): the single place the calendar is anchored
    /// (decision 25). `None` on a bullet line.
    pub fn anchor(&self) -> Option<i64> {
        let s = self.schedule()?;
        self.first_due_at.checked_sub(s.interval_seconds)
    }

    /// When the last payment is owed: the last due date,
    /// `first_due_at + (installments - 1) * interval`, on a scheduled line; `term_end` on a bullet
    /// line. Each tranche's interest runs from its draw to this instant (D1).
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
            version: POOL_VERSION,
            admin: Pubkey::default(),
            asset_mint: Pubkey::default(),
            vault: Pubkey::default(),
            share_mint: Pubkey::default(),
            dead_shares: Pubkey::default(),
            outstanding_principal,
            interest_collected: 0,
            written_off: 0,
            line_count: 0,
            paused: false,
            reserved: [0; 32],
            bump: 0,
            attestor: Pubkey::default(),
            collection: Pubkey::default(),
        }
    }

    /// R-25: the invariants every money instruction re-checks before returning.
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
    fn min_first_deposit_is_one_unit_with_a_dead_shares_floor() {
        assert_eq!(min_first_deposit(6), Some(1_000_000));
        assert_eq!(min_first_deposit(9), Some(1_000_000_000));
        assert_eq!(min_first_deposit(2), Some(1_000_000), "the floor, not 100");
        assert_eq!(min_first_deposit(0), Some(1_000_000));
        assert_eq!(min_first_deposit(19), Some(10u64.pow(19)));
        assert_eq!(min_first_deposit(20), None, "10^20 does not fit a u64");
    }
}
