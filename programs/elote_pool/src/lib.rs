//! elote_pool: a liquidity pool with a share token. Investors deposit the pool asset and receive
//! shares; the council approves credit lines from the pool; repaid interest raises the value of
//! every share and a recognised loss lowers it.
//!
//! Share accounting follows ERC-4626: `shares = assets * supply / NAV` on deposit and
//! `assets = shares * NAV / supply` on redemption, with `NAV = vault balance + outstanding
//! principal`. Interest counts when it is collected; nothing accrues.

use anchor_lang::prelude::*;

pub mod council;
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

    /// Creates the pool, its vault and its dead-shares account around `asset_mint`, the creator's
    /// share mint and the series collection. The creator signs and pays; the council is vault 0 of
    /// `multisig`, an autonomous Squads v4 multisig of at most ten members with a majority
    /// threshold, and does not sign. Fixes the grace period and the line cap (`1..=10_000` bps of the
    /// NAV) for the pool's life.
    pub fn init_pool(ctx: Context<InitPool>, params: InitPoolParams) -> Result<()> {
        instructions::pool::init_pool(ctx, params)
    }

    /// Sets the seconds a line in collection waits, from its mark, before it can be written off.
    /// Council only; refused below the 30-day floor.
    pub fn set_collection_period(ctx: Context<SetCollectionPeriod>, collection_period_seconds: u32) -> Result<()> {
        instructions::pool::set_collection_period(ctx, collection_period_seconds)
    }

    /// Sets the wait, in seconds, from a cancellation request before the holder may cancel a pagaré
    /// without the attestor; never below seven days. Council only. A pending request keeps its
    /// deadline.
    pub fn set_cancel_delay(ctx: Context<SetCancelDelay>, cancel_delay_seconds: u32) -> Result<()> {
        instructions::pool::set_cancel_delay(ctx, cancel_delay_seconds)
    }

    /// Replaces the pool's attestor. The council and the new attestor sign; refused with the council as
    /// attestor.
    pub fn set_attestor(ctx: Context<SetAttestor>) -> Result<()> {
        instructions::pool::set_attestor(ctx)
    }

    /// Closes the draw window of an `Approved` or `Active` line now, forfeiting the undrawn
    /// remainder. Council only. Repayment is untouched.
    pub fn suspend_draws(ctx: Context<SuspendDraws>) -> Result<()> {
        instructions::lines::suspend_draws(ctx)
    }

    /// Cancels an `Approved` line nobody drew. Council only; refused once anything was drawn.
    pub fn cancel_line(ctx: Context<CancelLine>) -> Result<()> {
        instructions::lines::cancel_line(ctx)
    }

    /// The depositor pays `assets` for shares at the current NAV, at least `min_shares_out`; the
    /// first deposit must meet the minimum and locks `DEAD_SHARES`. Refused while paused.
    pub fn deposit(ctx: Context<Deposit>, assets: u64, min_shares_out: u64) -> Result<()> {
        instructions::pool::deposit(ctx, assets, min_shares_out)
    }

    /// The holder burns `shares` for assets at the current NAV, at least `min_assets_out`, up to
    /// the vault's balance. Refused while paused.
    pub fn redeem(ctx: Context<Redeem>, shares: u64, min_assets_out: u64) -> Result<()> {
        instructions::pool::redeem(ctx, shares, min_assets_out)
    }

    /// Approves line `line_id` (the next): a commitment of `principal` at an annual `rate_bps`,
    /// repaid at `term_end` or on the `schedule`'s calendar. The council, the borrower, the attestor
    /// and the sponsor if named sign; refused while paused, with the council or the attestor as
    /// borrower, or above the line cap. Moves no money.
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

    /// Pays a tranche of `amount` to the borrower against the signed pagaré `document_hash` and
    /// mints the pagaré as a frozen Core asset owned by the pool, with its `Pagare` record. The
    /// borrower, the attestor and the rent payer sign; refused while paused, once the draw window
    /// has closed, or above what the line or the vault has.
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

    /// Extinguishes pagaré `draw_index` once a repayment has marked it paid
    /// (`PAGARE_PAID_PENDING_CANCEL`, whatever the line's status): writes the `Terminacion` entry,
    /// then thaws and burns the asset; the record stays. The owner (the council) or the procurador of
    /// record signs, with the attestor; without the attestor, only the owner after a request's
    /// `cancel_after`, with the requested receipt. Never paused.
    pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::cancel_pagare(ctx, draw_index, receipt_hash)
    }

    /// Endorses outstanding pagaré `draw_index` en procuración to `to`: appends the entry and lets
    /// the program address `["procurador", pagare, to]` burn the asset through an owner-managed
    /// `BurnDelegate`. The council (the holder) and the attestor sign; `kind` must be
    /// `KIND_PROCURACION`; refused with `to` the zero key, the pool, the council or the attestor, or
    /// a procuración in force.
    pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::endorse_pagare(ctx, draw_index, kind, to, endoso_hash)
    }

    /// Revokes the procuración in force on pagaré `draw_index`: appends the revocation entry over
    /// the signed revocation's hash and returns the burn delegate to the owner. The holder (the
    /// council) signs.
    pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::revoke_procuracion(ctx, draw_index, entry_hash)
    }

    /// Records a court's act on pagaré `draw_index`: `kind` is `KIND_ANOTACION_JUDICIAL` (embargo,
    /// suspension, cancellation) or `KIND_CONSTANCIA_JUDICIAL`, over the court document's hash. The
    /// holder (the council) and the attestor sign; refused on a cancelled pagaré; moves nothing.
    pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::court_annotation(ctx, draw_index, kind, document_hash)
    }

    /// The holder (the council) requests the cancellation of pagaré `draw_index`, marked paid by a
    /// repayment, against `receipt_hash`; after `cancel_after = max(now + cancel delay, pending
    /// deadline)` it may cancel without the attestor with that receipt. A new request replaces a
    /// pending one.
    pub fn request_cancellation(ctx: Context<RequestCancellation>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::request_cancellation(ctx, draw_index, receipt_hash)
    }

    /// Anyone pays up to `amount` on an `Active` or `InCollection` line: late interest (which
    /// `amount` must cover), then interest, then principal, never more than is due. The remaining
    /// accounts are exactly the pagaré records `[pagares_paid ..= last_touched]`, oldest first;
    /// each one covered is marked paid with the payer's provisional `receipt_hash`. The line
    /// becomes `Repaid` only when nothing is due and nothing can be drawn; while it can draw, a
    /// payment on a zero debt is refused. Never paused.
    pub fn repay<'info>(ctx: Context<'info, Repay<'info>>, amount: u64, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::lines::repay(ctx, amount, receipt_hash)
    }

    /// The council (the holder) or the attestor attaches a receipt to payment entry `entry_count` of
    /// pagaré `draw_index`, as a `Recibo` entry over `receipt_hash`. The holder's annotation also
    /// replaces the pagaré's provisional `receipt_hash` until the burn (`PAGARE_PAID_PENDING_CANCEL`).
    pub fn annotate_payment(ctx: Context<AnnotatePayment>, draw_index: u16, entry_count: u32, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::pagare::annotate_payment(ctx, draw_index, entry_count, receipt_hash)
    }

    /// Moves a scheduled line to collection once `max_missed` installments are missed and the grace
    /// period of the last of them has run, accelerating the balance. Anyone may call it, also while
    /// paused.
    pub fn mark_in_collection(ctx: Context<MarkInCollection>) -> Result<()> {
        instructions::lines::mark_in_collection(ctx)
    }

    /// Books a line's unpaid principal as a loss. Council only; an `Active` line after `term_end +
    /// grace`, an `InCollection` line after its collection period from the mark and after its
    /// oldest missed due date or its term, plus grace. Never paused.
    pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
        instructions::lines::write_off(ctx)
    }

    /// Pauses or resumes deposits, redemptions, openings and draws. Council only. Nothing else is
    /// ever paused: repayments, pagaré instructions, collection and write-offs.
    /// Names the next council: `kind` 0 (a Squads vault) or 1 (a generic signer, not yet
    /// accepted). Council only; the zero key withdraws the proposal.
    pub fn propose_council(ctx: Context<ProposeCouncil>, new_council: Pubkey, kind: u8) -> Result<()> {
        instructions::pool::propose_council(ctx, new_council, kind)
    }

    /// The proposed council signs and takes over, after the same multisig check as `init_pool`.
    pub fn accept_council(ctx: Context<AcceptCouncil>) -> Result<()> {
        instructions::pool::accept_council(ctx)
    }

    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::pool::set_paused(ctx, paused)
    }
}
