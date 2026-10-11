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
    pub reserved: [u8; 32],
    pub bump: u8,
}
