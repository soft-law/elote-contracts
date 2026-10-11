//! Account layouts.

use anchor_lang::prelude::*;

pub mod seeds {
    /// `[POOL, admin, asset_mint]`
    pub const POOL: &[u8] = b"pool";
    /// `[LINE, pool, line_id (u64 LE)]`
    pub const LINE: &[u8] = b"line";
    /// `[SHARES, pool]`: the share token mint.
    pub const SHARES: &[u8] = b"shares";
    /// `[VAULT, pool]`: the pool's asset token account.
    pub const VAULT: &[u8] = b"vault";
    /// `[DEAD, pool]`: the pool-owned share token account holding the dead shares.
    pub const DEAD: &[u8] = b"dead";
}

/// Smallest first deposit (1.0 of a 6-decimal asset), so the opening price of a share cannot be
/// set by a dust deposit.
pub const MIN_FIRST_DEPOSIT: u64 = 1_000_000;
/// Shares the first deposit locks forever in the pool-owned dead account, so the supply never
/// returns below this floor and a donation cannot round later deposits down to nothing.
pub const DEAD_SHARES: u64 = 1_000;
pub const BPS: u64 = 10_000;
pub const POOL_VERSION: u8 = 1;
pub const LINE_VERSION: u8 = 1;
/// Version of a line opened with an installment schedule (see [`Schedule`]).
pub const LINE_VERSION_SCHEDULED: u8 = 2;
/// Upper bound on the installments of one line, so the arrears computation is bounded.
pub const MAX_INSTALLMENTS: u16 = 120;

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
    /// | 4..32   | unused                 | zero     |                                           |
    ///
    /// Read and written only through [`Pool::grace_period_seconds`] and
    /// [`Pool::set_grace_period_seconds`].
    pub reserved: [u8; 32],
    pub bump: u8,
}

impl Pool {
    /// Seconds after a due date (or after the term) before late interest accrues and before a
    /// write-off is allowed. Fixed at `init_pool`.
    pub fn grace_period_seconds(&self) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.reserved[0..4]);
        u32::from_le_bytes(b)
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
    pub principal: u64,
    /// Fixed interest on the principal.
    pub rate_bps: u16,
    /// `principal * rate_bps / BPS`, fixed at opening.
    pub interest_due: u64,
    pub term_end: i64,
    pub principal_repaid: u64,
    pub interest_repaid: u64,
    pub status: LineStatus,
    pub opened_at: i64,
    pub drawn_at: i64,
    pub closed_at: i64,
    /// Zero on a version-1 line (single maturity). On a version-2 line, the installment schedule,
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
    /// | 22..32 | unused             | zero    |                                                 |
    pub reserved: [u8; 32],
    pub bump: u8,
}

/// Installment terms requested by `open_line`; fixed for the life of the line.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleParams {
    /// Seconds between due dates; installment `k` is due at `drawn_at + k * interval_seconds`.
    pub interval_seconds: i64,
    /// `principal + interest_due` is paid in this many installments.
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
        Self { interval_seconds, installments, late_bps, max_missed: r[12], flags: r[13], late_collected }
    }

    pub fn pack(&self, r: &mut [u8; 32]) {
        r[0..8].copy_from_slice(&self.interval_seconds.to_le_bytes());
        r[8..10].copy_from_slice(&self.installments.to_le_bytes());
        r[10..12].copy_from_slice(&self.late_bps.to_le_bytes());
        r[12] = self.max_missed;
        r[13] = self.flags;
        r[14..22].copy_from_slice(&self.late_collected.to_le_bytes());
    }
}

impl CreditLine {
    /// The installment schedule of a version-2 line; `None` for a single-maturity line.
    pub fn schedule(&self) -> Option<Schedule> {
        (self.version == LINE_VERSION_SCHEDULED).then(|| Schedule::unpack(&self.reserved))
    }

    pub fn set_schedule(&mut self, s: &Schedule) {
        s.pack(&mut self.reserved);
    }
}
