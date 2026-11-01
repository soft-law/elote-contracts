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

#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "elote_pool",
    project_url: "https://elote.baby",
    contacts: "link:https://github.com/soft-law/elote-contracts/security/advisories/new",
    policy: "https://github.com/soft-law/elote-contracts/blob/main/SECURITY.md",
    preferred_languages: "en,es",
    source_code: "https://github.com/soft-law/elote-contracts",
    auditors: "None"
}

#[program]
pub mod elote_pool {
    use super::*;

    /// Creates the pool, its vault and its dead-shares account around `asset_mint` and the
    /// admin-supplied share mint, with the grace period (seconds) that applies to every line of
    /// the pool, the largest line `open_line` accepts as a share of the NAV in basis points
    /// (`1..=10_000`), and the attestor (the agreements engine's key).
    pub fn init_pool(ctx: Context<InitPool>, grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey) -> Result<()> {
        instructions::pool::init_pool(ctx, grace_period_seconds, max_line_bps, attestor)
    }

    /// Replaces the pool's attestor. Admin only; the recovery when the agreements engine's key
    /// rotates or is lost.
    pub fn set_attestor(ctx: Context<SetAttestor>, attestor: Pubkey) -> Result<()> {
        instructions::pool::set_attestor(ctx, attestor)
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

    /// Approves credit line `line_id` (the next in sequence): a commitment of `principal` to
    /// `borrower` at an annual `rate_bps`, drawable in tranches until the first due date (or the
    /// term on a bullet line). The admin, the borrower, the attestor and a named sponsor sign.
    /// With `schedule`, the line is repaid in installments on the contract's calendar; without
    /// it, at `term_end`. Moves no money.
    #[allow(clippy::too_many_arguments)]
    pub fn open_line(
        ctx: Context<OpenLine>,
        line_id: u64,
        principal: u64,
        rate_bps: u16,
        term_end: i64,
        agreement_hash: [u8; 32],
        sponsor: Option<Pubkey>,
        sponsor_role: u8,
        schedule: Option<ScheduleParams>,
        expediente_root: [u8; 32],
    ) -> Result<()> {
        instructions::lines::open_line(
            ctx,
            line_id,
            principal,
            rate_bps,
            term_end,
            agreement_hash,
            sponsor,
            sponsor_role,
            schedule,
            expediente_root,
        )
    }

    /// Draws a tranche of `amount` from the vault to the borrower while the draw window is open,
    /// against the signed pagaré whose hash is `document_hash`. The borrower and the attestor
    /// sign. Interest on the tranche, at the annual rate to the last due date, is fixed here.
    pub fn draw(ctx: Context<Draw>, amount: u64, document_hash: [u8; 32]) -> Result<()> {
        instructions::lines::draw(ctx, amount, document_hash)
    }

    /// Repays up to `amount` of a drawn line: late interest (which `amount` must cover), then
    /// interest, then principal, and never more than is due. Anyone may pay. A full repayment
    /// closes the line and forfeits the undrawn remainder.
    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        instructions::lines::repay(ctx, amount)
    }

    /// Moves a scheduled line to collection once `max_missed` installments are missed and the
    /// grace period of the last of them has run. Anyone may call it, also while paused.
    pub fn mark_in_collection(ctx: Context<MarkInCollection>) -> Result<()> {
        instructions::lines::mark_in_collection(ctx)
    }

    /// Recognises the unpaid principal of a line as a loss. Admin only. Allowed on an active line
    /// once `now > term_end + grace`, and on a line in collection once its oldest missed
    /// installment is more than `grace` past due (or the term is). It does not wait: the call
    /// books the loss the moment one of those conditions holds.
    pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
        instructions::lines::write_off(ctx)
    }

    /// Pauses or resumes deposits, redemptions, openings and draws. Repayments, collection and
    /// write-offs are never paused.
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::pool::set_paused(ctx, paused)
    }
}
