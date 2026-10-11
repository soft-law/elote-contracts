//! Events, all emitted with `emit_cpi!`.

use anchor_lang::prelude::*;

#[event]
pub struct PoolInitialized {
    pub pool: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub admin: Pubkey,
}

/// Emitted by `init_pool` after `PoolInitialized`: the pool parameters fixed at launch.
#[event]
pub struct PoolConfigured {
    pub pool: Pubkey,
    pub grace_period_seconds: u32,
}

#[event]
pub struct Deposited {
    pub pool: Pubkey,
    pub depositor: Pubkey,
    pub assets: u64,
    pub shares: u64,
    pub nav_after: u64,
    pub supply_after: u64,
}

#[event]
pub struct Redeemed {
    pub pool: Pubkey,
    pub holder: Pubkey,
    pub shares: u64,
    pub assets: u64,
    pub nav_after: u64,
    pub supply_after: u64,
}

#[event]
pub struct LineOpened {
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    pub sponsor: Option<Pubkey>,
    pub agreement_hash: [u8; 32],
    pub principal: u64,
    pub rate_bps: u16,
    pub interest_due: u64,
    pub term_end: i64,
}

/// Emitted by `open_line` after `LineOpened` when the line has an installment schedule.
#[event]
pub struct LineScheduled {
    pub pool: Pubkey,
    pub line_id: u64,
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
}

#[event]
pub struct LineDrawn {
    pub pool: Pubkey,
    pub line_id: u64,
    pub amount: u64,
    pub outstanding_after: u64,
}

#[event]
pub struct LineRepaid {
    pub pool: Pubkey,
    pub line_id: u64,
    pub payer: Pubkey,
    pub principal_part: u64,
    pub interest_part: u64,
    pub remaining: u64,
    pub nav_after: u64,
}

/// Late interest collected by a repayment of a scheduled line; it stays in the pool and raises
/// the NAV like interest. Emitted before the `LineRepaid` of the same repayment.
#[event]
pub struct LateInterestCollected {
    pub pool: Pubkey,
    pub line_id: u64,
    pub amount: u64,
}

/// A scheduled line moved to collection. `remaining` is the principal and interest still due,
/// without late interest.
#[event]
pub struct LineInCollection {
    pub pool: Pubkey,
    pub line_id: u64,
    pub missed: u16,
    pub remaining: u64,
}

#[event]
pub struct LineWrittenOff {
    pub pool: Pubkey,
    pub line_id: u64,
    pub loss: u64,
    pub nav_after: u64,
}

#[event]
pub struct PoolPaused {
    pub pool: Pubkey,
    pub paused: bool,
}
