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
    /// admin's share mint; fixes the grace period, the line cap (`1..=10_000` bps of the NAV)
    /// and the attestor for the life of the pool. The admin signs.
    pub fn init_pool(ctx: Context<InitPool>, grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey) -> Result<()> {
        instructions::pool::init_pool(ctx, grace_period_seconds, max_line_bps, attestor)
    }

    /// Replaces the pool's attestor. The admin and the new attestor sign.
    pub fn set_attestor(ctx: Context<SetAttestor>) -> Result<()> {
        instructions::pool::set_attestor(ctx)
    }

    /// Closes a line's draw window now. Admin only; refused unless the line is approved or
    /// active. Repayment is untouched.
    pub fn suspend_draws(ctx: Context<SuspendDraws>) -> Result<()> {
        instructions::lines::suspend_draws(ctx)
    }

    /// Clears an approved commitment nobody drew. Admin only; refused once anything was drawn.
    pub fn cancel_line(ctx: Context<CancelLine>) -> Result<()> {
        instructions::lines::cancel_line(ctx)
    }

    /// Deposits `assets` for shares at the current NAV, at least `min_shares_out`; the first
    /// deposit locks `DEAD_SHARES`. Refused while paused.
    pub fn deposit(ctx: Context<Deposit>, assets: u64, min_shares_out: u64) -> Result<()> {
        instructions::pool::deposit(ctx, assets, min_shares_out)
    }

    /// Burns `shares` for assets at the current NAV, at least `min_assets_out`, up to the vault's
    /// liquidity. Refused while paused.
    pub fn redeem(ctx: Context<Redeem>, shares: u64, min_assets_out: u64) -> Result<()> {
        instructions::pool::redeem(ctx, shares, min_assets_out)
    }

    /// Approves line `line_id` (the next): a commitment of `principal` at an annual `rate_bps`,
    /// repaid at `term_end` or on the `schedule`'s calendar. The admin, the borrower, the attestor
    /// and the named sponsor sign; refused while paused, with the admin as borrower, or above the
    /// line cap. Moves no money.
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

    /// Draws a tranche of `amount` to the borrower against the signed pagaré `document_hash` and
    /// mints the pagaré asset with its record. The borrower, the attestor and the rent payer
    /// sign; refused while paused, once the draw window has closed, or above what is available.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        ctx: Context<Draw>,
        amount: u64,
        document_hash: [u8; 32],
        asset_bump: u8,
        pagare_bump: u8,
        cert_fingerprint: [u8; 32],
        signature_method: u8,
    ) -> Result<()> {
        instructions::lines::draw(ctx, amount, document_hash, asset_bump, pagare_bump, cert_fingerprint, signature_method)
    }

    /// Marks pagaré `draw_index` paid against the receipt `receipt_hash` and burns its asset; the
    /// record is kept. The holder (the admin while the asset is in the pool) and the attestor
    /// sign; refused until the line's principal repaid covers the pagaré, and in collection or
    /// after a write-off. Never paused.
    pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::cancel_pagare(ctx, draw_index, receipt_hash)
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

    /// Books the unpaid principal of a line as a loss. Admin only; refused before
    /// `term_end + grace`, or on a line in collection before its oldest missed due date plus grace.
    pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
        instructions::lines::write_off(ctx)
    }

    /// Pauses or resumes deposits, redemptions, openings and draws. Repayments, collection and
    /// write-offs are never paused.
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::pool::set_paused(ctx, paused)
    }
}
