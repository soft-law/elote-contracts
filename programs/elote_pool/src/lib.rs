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
pub mod registry;
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
    /// and the attestor for the life of the pool, and the collection period (at least 30 days).
    /// The admin signs; refused with the admin as attestor.
    pub fn init_pool(
        ctx: Context<InitPool>,
        grace_period_seconds: u32,
        max_line_bps: u16,
        attestor: Pubkey,
        collection_period_seconds: u32,
        cancel_delay_seconds: u32,
    ) -> Result<()> {
        instructions::pool::init_pool(ctx, grace_period_seconds, max_line_bps, attestor, collection_period_seconds, cancel_delay_seconds)
    }

    /// Sets the seconds a line in collection waits, from its mark, before it can be written off.
    /// Admin only; refused below the 30-day floor.
    pub fn set_collection_period(ctx: Context<SetCollectionPeriod>, collection_period_seconds: u32) -> Result<()> {
        instructions::pool::set_collection_period(ctx, collection_period_seconds)
    }

    /// Sets the backstop's wait, in seconds, from a cancellation request before the holder may
    /// cancel a pagaré without the attestor; never below seven days. Admin only. A request fixes
    /// its own deadline, so a change never moves one pending.
    pub fn set_cancel_delay(ctx: Context<SetCancelDelay>, cancel_delay_seconds: u32) -> Result<()> {
        instructions::pool::set_cancel_delay(ctx, cancel_delay_seconds)
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

    /// Marks pagaré `draw_index` paid against the receipt `receipt_hash`, writes the Terminacion
    /// entry and burns its asset; the record is kept. The holder (the admin while the asset is in
    /// the pool) or the procurador of record signs, with the attestor; refused until the line's
    /// principal repaid covers the pagaré, and in collection or after a write-off. Never paused.
    pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::cancel_pagare(ctx, draw_index, receipt_hash)
    }

    /// Endorses pagaré `draw_index` en procuración to `to` (LGTOC 35): appends the entry and lends
    /// the power to extinguish it to a program-signed delegate. The admin (the pool is the owner)
    /// and the attestor sign; in this version `kind` must be `KIND_PROCURACION`.
    pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::endorse_pagare(ctx, draw_index, kind, to, endoso_hash)
    }

    /// Revokes the procuración in force on pagaré `draw_index` (LGTOC 41): appends the revocation
    /// entry over the signed revocation's hash and takes the delegate back. The holder alone signs.
    pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::revoke_procuracion(ctx, draw_index, entry_hash)
    }

    /// Records a court's act on pagaré `draw_index`: `kind` is AnotacionJudicial (6: embargo,
    /// suspension, cancellation) or ConstanciaJudicial (7, LGTOC 28), over the court document's
    /// hash. The holder and the attestor sign; nothing moves.
    pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::court_annotation(ctx, draw_index, kind, document_hash)
    }

    /// Records the holder's request to cancel pagaré `draw_index` against the receipt
    /// `receipt_hash`, the backstop on the attestor: the same checks as the cancellation itself,
    /// one request at a time; after the pool's cancel delay the holder may cancel alone with the
    /// same receipt.
    pub fn request_cancellation(ctx: Context<RequestCancellation>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::request_cancellation(ctx, draw_index, receipt_hash)
    }

    /// Repays up to `amount` of a drawn line: late interest (which `amount` must cover), then
    /// interest, then principal, and never more than is due. Anyone may pay. A full repayment
    /// closes the line and forfeits the undrawn remainder.
    pub fn repay<'info>(ctx: Context<'info, Repay<'info>>, amount: u64, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::lines::repay(ctx, amount, receipt_hash)
    }

    /// Attaches a receipt to a payment entry of pagaré `draw_index`: appends a Recibo entry over
    /// `receipt_hash`, referencing entry `entry_count` of the pagaré's chain. The holder or the
    /// attestor signs; the repair path when a payer's hash was zero or wrong.
    pub fn annotate_payment(ctx: Context<AnnotatePayment>, draw_index: u16, entry_count: u32, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::annotate_payment(ctx, draw_index, entry_count, receipt_hash)
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
