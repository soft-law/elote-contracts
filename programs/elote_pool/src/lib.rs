//! elote_pool: a liquidity pool with a share token. Investors deposit the pool asset and receive
//! shares; the council approves credit lines from the pool; repaid interest raises the value of
//! every share and a recognised loss lowers it.
//!
//! Share accounting follows ERC-4626: `shares = assets * supply / NAV` on deposit and
//! `assets = shares * NAV / supply` on redemption, with `NAV = vault balance + outstanding
//! principal`. Interest counts when it is collected; nothing accrues.
//!
//! Every council instruction takes `valid_until` as its last argument and is refused past it
//! (`ProposalExpired`): Squads proposals never expire. `pause` (a late pause only protects) and
//! `init_pool` (no council signature) do not.

use anchor_lang::prelude::*;

pub mod council;
pub mod errors;
pub mod events;
pub mod fx;
pub mod instructions;
pub mod math;
pub mod registry;
pub mod state;
pub mod terms;

use instructions::*;
use terms::LineTerms;

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
    pub fn set_collection_period(ctx: Context<SetCollectionPeriod>, collection_period_seconds: u32, valid_until: i64) -> Result<()> {
        instructions::pool::set_collection_period(ctx, collection_period_seconds, valid_until)
    }

    /// Sets the wait from a pagaré's payment before its borrower may cancel it alone; never below
    /// 30 days nor above 365. Applies to pagarés paid from then on: each keeps the grace in force
    /// at its payment. Council only.
    pub fn set_cancel_grace(ctx: Context<SetCancelGrace>, cancel_grace_seconds: u32, valid_until: i64) -> Result<()> {
        instructions::pool::set_cancel_grace(ctx, cancel_grace_seconds, valid_until)
    }

    /// Closes the draw window of an `Approved` or `Active` line now, forfeiting the undrawn
    /// remainder. Council only. Repayment is untouched.
    pub fn suspend_draws(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::suspend_draws(ctx, valid_until)
    }

    /// Cancels an `Approved` line nobody drew. Council only; refused once anything was drawn.
    pub fn cancel_line(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::cancel_line(ctx, valid_until)
    }

    /// Reopens the draw window of a suspended line to its original end. Council only; refused while
    /// an acceleration is pending.
    pub fn resume_draws(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::resume_draws(ctx, valid_until)
    }

    /// Proposes to collect a drawn line early: draws are suspended at once and `accelerate_line`
    /// may fire after the pool'"'"'s notice. Council only.
    pub fn propose_acceleration(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::propose_acceleration(ctx, valid_until)
    }

    /// Withdraws a pending acceleration; draws stay suspended. Council only.
    pub fn withdraw_acceleration(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::withdraw_acceleration(ctx, valid_until)
    }

    /// Fires a pending acceleration once its notice has run: the line moves to collection, as with
    /// `mark_in_collection`, without missed installments. Council only.
    pub fn accelerate_line(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
        instructions::lines::accelerate_line(ctx, valid_until)
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

    /// The borrower requests line `line_id` (the pool's next) on terms `terms`, in centavos of MXN;
    /// a separate payer funds the account and the named sponsor co-signs. Stored as `Requested` with
    /// the terms hash; refused while paused. Moves no money.
    pub fn request_line(ctx: Context<RequestLine>, line_id: u64, terms: LineTerms) -> Result<()> {
        instructions::lines::request_line(ctx, line_id, terms)
    }

    /// The council approves a requested line, naming its expediente root and terms hash, within 15
    /// days of the request and within the line cap; a curator cannot be the borrower.
    pub fn approve_line(ctx: Context<ApproveLine>, expediente_root: [u8; 32], terms_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::lines::approve_line(ctx, expediente_root, terms_hash, valid_until)
    }

    /// The council rejects a requested line; the rent returns to the payer.
    pub fn reject_line(ctx: Context<RejectLine>, valid_until: i64) -> Result<()> {
        instructions::lines::reject_line(ctx, valid_until)
    }

    /// The borrower withdraws a requested line, expired or not; the rent returns to the payer.
    pub fn withdraw_request(ctx: Context<WithdrawRequest>) -> Result<()> {
        instructions::lines::withdraw_request(ctx)
    }

    /// Pays a tranche of `amount_mxn` centavos to the borrower as USDC at the FIX in force, which the
    /// borrower signs as `fx_rate` and `fx_date`, and mints the pagaré (its hash derived on chain
    /// from the approved template and terms) as a frozen Core asset owned by the pool, with its
    /// `Pagare` record. The borrower and the rent payer sign; refused while paused, once the draw
    /// window has closed, on a changed or stale FIX, or above what the line, the vault or the pool cap
    /// allows.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        ctx: Context<Draw>,
        amount_mxn: u64,
        fx_rate: u64,
        fx_date: u32,
        asset_bump: u8,
        pagare_bump: u8,
        cert_fingerprint: [u8; 32],
        signature_method: u8,
    ) -> Result<()> {
        instructions::lines::draw(ctx, amount_mxn, fx_rate, fx_date, asset_bump, pagare_bump, cert_fingerprint, signature_method)
    }

    /// Extinguishes pagaré `draw_index` once a repayment has marked it paid
    /// (`PAGARE_PAID_PENDING_CANCEL`, whatever the line's status): writes the `Terminacion` entry,
    /// then thaws and burns the asset; the record stays. The council (the owner), the procurador of
    /// record alone, or the line's borrower alone once the cancel grace fixed at the payment has run.
    /// Never paused.
    pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::pagare::cancel_pagare(ctx, draw_index, receipt_hash, valid_until)
    }

    /// Endorses outstanding pagaré `draw_index` en procuración to `to`: appends the entry and lets
    /// the program address `["procurador", pagare, to]` burn the asset through an owner-managed
    /// `BurnDelegate`. The council (the holder) signs; `kind` must be `KIND_PROCURACION`; refused
    /// with `to` the zero key, the pool, the council, a curator, the guardian or the borrower, or a
    /// procuración in force.
    pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::pagare::endorse_pagare(ctx, draw_index, kind, to, endoso_hash, valid_until)
    }

    /// Revokes the procuración in force on pagaré `draw_index`: appends the revocation entry over
    /// the signed revocation's hash and returns the burn delegate to the owner. The holder (the
    /// council) signs.
    pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::pagare::revoke_procuracion(ctx, draw_index, entry_hash, valid_until)
    }

    /// Records a court's act on pagaré `draw_index`: `kind` is `KIND_ANOTACION_JUDICIAL` (embargo,
    /// suspension, cancellation) or `KIND_CONSTANCIA_JUDICIAL`, over the court document's hash. The
    /// council signs; refused on a cancelled pagaré; moves nothing.
    pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::pagare::court_annotation(ctx, draw_index, kind, document_hash, valid_until)
    }

    /// Anyone pays up to `max_usdc` on an `Active` or `InCollection` line, credited in MXN at the FIX in
    /// force (`fx_rate`, `fx_date`, whatever its age): late interest (which the credit must cover),
    /// then interest, then principal, never more than is due; the USDC taken is rounded up. The remaining
    /// accounts are exactly the pagaré records `[pagares_paid ..= last_touched]`, oldest first, at
    /// most `MAX_SIDECARS_PER_REPAY` of them; each one covered is marked paid with the payer's provisional `receipt_hash`. The line
    /// becomes `Repaid` only when nothing is due and nothing can be drawn; while it can draw, a
    /// payment on a zero debt is refused. Never paused.
    pub fn repay<'info>(ctx: Context<'info, Repay<'info>>, max_usdc: u64, fx_rate: u64, fx_date: u32, receipt_hash: [u8; 32]) -> Result<()> {
        instructions::lines::repay(ctx, max_usdc, fx_rate, fx_date, receipt_hash)
    }

    /// The council (the holder) attaches a receipt to payment entry `entry_count` of pagaré
    /// `draw_index`, as a `Recibo` entry over `receipt_hash`. The annotation also replaces the pagaré's provisional `receipt_hash` until the burn (`PAGARE_PAID_PENDING_CANCEL`).
    pub fn annotate_payment(ctx: Context<AnnotatePayment>, draw_index: u16, entry_count: u32, receipt_hash: [u8; 32], valid_until: i64) -> Result<()> {
        instructions::pagare::annotate_payment(ctx, draw_index, entry_count, receipt_hash, valid_until)
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
    pub fn write_off(ctx: Context<WriteOff>, valid_until: i64) -> Result<()> {
        instructions::lines::write_off(ctx, valid_until)
    }


    /// Names the next council: `kind` 0 (a Squads vault) or 1 (a generic signer, not yet
    /// accepted). Council only; the zero key withdraws the proposal.
    pub fn propose_council(ctx: Context<ProposeCouncil>, new_council: Pubkey, kind: u8, valid_until: i64) -> Result<()> {
        instructions::pool::propose_council(ctx, new_council, kind, valid_until)
    }

    /// The proposed council signs and takes over, after the same multisig check as `init_pool`.
    pub fn accept_council(ctx: Context<AcceptCouncil>, valid_until: i64) -> Result<()> {
        instructions::pool::accept_council(ctx, valid_until)
    }

    /// Names the key that publishes the FIX (the zero key for none). Council only.
    pub fn set_fx_feeder(ctx: Context<SetFxFeeder>, fx_feeder: Pubkey, valid_until: i64) -> Result<()> {
        instructions::pool::set_fx_feeder(ctx, fx_feeder, valid_until)
    }

    /// Publishes Banxico's FIX: `fix_rate` is MXN per USD times 10^4, `fix_date` the date it was
    /// determined (`yyyymmdd`). The feeder moves forward within 5%; the council publishes the rest.
    pub fn post_fix(ctx: Context<PostFix>, fix_rate: u64, fix_date: u32, valid_until: i64) -> Result<()> {
        instructions::pool::post_fix(ctx, fix_rate, fix_date, valid_until)
    }

    /// Pauses deposits, redemptions, requests, approvals and draws. Any member of the council'"'"'s multisig alone,
    /// the guardian or the council; repayments and everything else go on.
    pub fn pause(ctx: Context<Pause>) -> Result<()> {
        instructions::pool::pause(ctx)
    }

    /// Ends a pause. Council only.
    pub fn resume(ctx: Context<CouncilPool>, valid_until: i64) -> Result<()> {
        instructions::pool::resume(ctx, valid_until)
    }

    /// Sets the guardian, a key that may only pause (the zero key for none). Council only.
    pub fn set_guardian(ctx: Context<CouncilPool>, guardian: Pubkey, valid_until: i64) -> Result<()> {
        instructions::pool::set_guardian(ctx, guardian, valid_until)
    }

    /// Sets the line cap, `1..=10_000` bps of the NAV at the approval. Council only.
    pub fn set_max_line_bps(ctx: Context<CouncilPool>, max_line_bps: u16, valid_until: i64) -> Result<()> {
        instructions::pool::set_max_line_bps(ctx, max_line_bps, valid_until)
    }

    /// Sets the pool-wide cap on the USDC cost lent out. Council only.
    pub fn set_outstanding_cap(ctx: Context<CouncilPool>, max_outstanding_principal: u64, valid_until: i64) -> Result<()> {
        instructions::pool::set_outstanding_cap(ctx, max_outstanding_principal, valid_until)
    }

    /// Sets the acceleration notice, at least seven days. Council only.
    pub fn set_acceleration_notice(ctx: Context<CouncilPool>, acceleration_notice_seconds: u32, valid_until: i64) -> Result<()> {
        instructions::pool::set_acceleration_notice(ctx, acceleration_notice_seconds, valid_until)
    }
}
