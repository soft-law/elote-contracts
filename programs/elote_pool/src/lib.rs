//! elote_pool: a liquidity pool with a share token. Investors deposit the pool asset and receive
//! shares; the admin approves credit lines from the pool; repaid interest raises the value of
//! every share and a recognised loss lowers it.
//!
//! Share accounting follows ERC-4626: `shares = assets * supply / NAV` on deposit and
//! `assets = shares * NAV / supply` on redemption, with `NAV = vault balance + outstanding
//! principal`. Interest counts when it is collected; nothing accrues.

use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod math;
pub mod state;

use instructions::*;
use state::ScheduleParams;

declare_id!("BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM");

#[program]
pub mod elote_pool {
    use super::*;

    /// Creates the pool, its share token mint and its vault for `asset_mint`, with the grace
    /// period (seconds) that applies to every line of the pool.
    pub fn init_pool(ctx: Context<InitPool>, grace_period_seconds: u32) -> Result<()> {
        instructions::pool::init_pool(ctx, grace_period_seconds)
    }

    /// Deposits `assets` into the vault and receives shares at the current NAV, at least
    /// `min_shares_out`. The first deposit locks `DEAD_SHARES` in the pool.
    pub fn deposit(ctx: Context<Deposit>, assets: u64, min_shares_out: u64) -> Result<()> {
        instructions::pool::deposit(ctx, assets, min_shares_out)
    }

    /// Burns `shares` and receives assets at the current NAV, at least `min_assets_out`, up to
    /// the vault's liquidity.
    pub fn redeem(ctx: Context<Redeem>, shares: u64, min_assets_out: u64) -> Result<()> {
        instructions::pool::redeem(ctx, shares, min_assets_out)
    }

    /// Approves credit line `line_id` (the next in sequence) for `borrower`; a named sponsor
    /// co-signs. With `schedule`, the line is repaid in installments (version 2); without it, at a
    /// single maturity (version 1). Moves no money.
    #[allow(clippy::too_many_arguments)]
    pub fn open_line(
        ctx: Context<OpenLine>,
        line_id: u64,
        principal: u64,
        rate_bps: u16,
        term_end: i64,
        agreement_hash: [u8; 32],
        sponsor: Option<Pubkey>,
        schedule: Option<ScheduleParams>,
    ) -> Result<()> {
        instructions::lines::open_line(ctx, line_id, principal, rate_bps, term_end, agreement_hash, sponsor, schedule)
    }

    /// Pays the whole principal of an approved line from the vault to the borrower, before the
    /// term ends.
    pub fn draw(ctx: Context<Draw>) -> Result<()> {
        instructions::lines::draw(ctx)
    }

    /// Repays up to `amount` of a drawn line. Anyone may pay. A single-maturity line takes
    /// principal first, then interest; a scheduled line takes late interest (which `amount` must
    /// cover), then interest, then principal, and never more than is due.
    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        instructions::lines::repay(ctx, amount)
    }

    /// Moves a scheduled line with at least `max_missed` missed installments to collection.
    /// Anyone may call it.
    pub fn mark_in_collection(ctx: Context<MarkInCollection>) -> Result<()> {
        instructions::lines::mark_in_collection(ctx)
    }

    /// Recognises the unpaid principal of an active line past its term as a loss.
    pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
        instructions::lines::write_off(ctx)
    }

    /// Pauses or resumes deposits, redemptions, openings, draws and repayments.
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::pool::set_paused(ctx, paused)
    }
}
