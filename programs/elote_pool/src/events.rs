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

/// Emitted by `init_pool` after `PoolConfigured`: the largest line as a share of the NAV, in bps.
#[event]
pub struct PoolLineCapSet {
    pub pool: Pubkey,
    pub max_line_bps: u16,
}

/// Emitted by `init_pool` after `PoolLineCapSet`. `asset_controls` are the `Pool::asset_controls`
/// bits (bit 0 freeze authority, bit 1 Pausable); `permissioned`: the share mint is frozen by
/// default.
#[event]
pub struct PoolFlags {
    pub pool: Pubkey,
    pub asset_controls: u8,
    pub permissioned: bool,
}

/// The pool's collection period, at `init_pool` and after every `set_collection_period`: seconds
/// from the mark before a line in collection can be written off.
#[event]
pub struct CollectionPeriodSet {
    pub pool: Pubkey,
    pub collection_period_seconds: u32,
}

/// The pool's cancel delay, at `init_pool` and after every `set_cancel_delay`: the backstop's wait
/// from a cancellation request before the holder may cancel alone.
#[event]
pub struct CancelDelaySet {
    pub pool: Pubkey,
    pub cancel_delay_seconds: u32,
}

/// The pool's attestor, at `init_pool` and after every `set_attestor`.
#[event]
pub struct AttestorSet {
    pub pool: Pubkey,
    pub attestor: Pubkey,
}

/// The series' Metaplex Core collection the pool records, at `init_pool`.
#[event]
pub struct PoolCollectionSet {
    pub pool: Pubkey,
    pub collection: Pubkey,
}

/// The NAV and the share supply after every instruction that moves either (`deposit`, `redeem`,
/// `repay`, `write_off`), read back after the CPIs: `price per share = total_assets / share_supply`.
#[event]
pub struct NavUpdated {
    pub pool: Pubkey,
    pub total_assets: u64,
    pub share_supply: u64,
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

/// Emitted by `open_line` after `LineOpened` (and `LineScheduled`). `first_due_at` is zero on a
/// bullet line.
#[event]
pub struct LineTermsSet {
    pub pool: Pubkey,
    pub line_id: u64,
    pub first_due_at: i64,
    pub availability_end: i64,
    pub expediente_root: [u8; 32],
}

/// Emitted by `open_line` after `LineTermsSet`. `role` is `SPONSOR_PROMOTER` or
/// `SPONSOR_GUARANTOR`; `sponsor == None` and role 0 when no sponsor is named.
#[event]
pub struct LineSponsorRole {
    pub pool: Pubkey,
    pub line_id: u64,
    pub sponsor: Option<Pubkey>,
    pub role: u8,
}

/// Emitted by `draw` after `LineDrawState`: the pagaré issued as the Metaplex Core asset `asset`,
/// held frozen by `holder` (the pool PDA). `due_at` is the line's last due date at issuance.
#[event]
pub struct PagareIssued {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub pagare_hash: [u8; 32],
    pub amount: u64,
    pub due_at: i64,
    pub holder: Pubkey,
}

/// One entry of a pagaré's registry (LGTOC 8 fr. VI, 13, 130): the full content the sidecar's
/// hash chain commits to. `stated_at` is the date the entry states (the signed message's date for
/// a late entry), `recorded_at` the slot's time.
#[event]
pub struct PagareEntry {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub kind: u8,
    pub from: Pubkey,
    pub to: Pubkey,
    pub entry_hash: [u8; 32],
    pub entry_count: u16,
    pub recorded_at: i64,
    pub stated_at: i64,
}

/// Emitted by `cancel_pagare` (C22, LGTOC 129), before the burn. `holder` is the key that
/// extinguished the pagaré; the sidecar stays as the PAGADO mark. No AppData length: the engine
/// archives the log off chain before the burn.
#[event]
pub struct PagareCancelled {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub receipt_hash: [u8; 32],
    pub holder: Pubkey,
}

/// Emitted by `suspend_draws`; `available_forfeited` is the undrawn remainder.
#[event]
pub struct LineDrawsSuspended {
    pub pool: Pubkey,
    pub line_id: u64,
    pub available_forfeited: u64,
}

/// Emitted by `cancel_line`.
#[event]
pub struct LineCancelled {
    pub pool: Pubkey,
    pub line_id: u64,
}

/// One tranche; `amount` is the tranche, emitted once per draw.
#[event]
pub struct LineDrawn {
    pub pool: Pubkey,
    pub line_id: u64,
    pub amount: u64,
    pub outstanding_after: u64,
}

/// Emitted by `draw` after `LineDrawn`: the line after the tranche and the signed pagaré's hash,
/// which is not stored on the line.
#[event]
pub struct LineDrawState {
    pub pool: Pubkey,
    pub line_id: u64,
    pub amount: u64,
    pub drawn: u64,
    pub available: u64,
    pub interest_due: u64,
    pub availability_end: i64,
    pub document_hash: [u8; 32],
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

/// A scheduled line moved to collection and its balance accelerated. `remaining` is the principal
/// and interest still due after the acceleration, without late interest; `interest_forgiven` is
/// the ordinary interest of the installments not yet due at the mark, which the line no longer
/// owes (appended, events are append-only).
#[event]
pub struct LineInCollection {
    pub pool: Pubkey,
    pub line_id: u64,
    pub missed: u16,
    pub remaining: u64,
    pub interest_forgiven: u64,
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

/// One payment entry a `repay` wrote: the pagaré, the entry's kind (`Abono` or `Pagado`), its
/// position in that sidecar's chain, and the part of the payment's principal that fell on it.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct PaymentEntry {
    pub draw_index: u16,
    pub kind: u8,
    pub entry_count: u16,
    pub principal_applied: u64,
}

/// The payment entries of one `repay` (art. 130): the payer's receipt hash (zero means "receipt
/// to follow"), the payment's split and one `PaymentEntry` per sidecar touched, oldest first.
/// Emitted after the `PagareEntry` of each entry.
#[event]
pub struct PagarePayment {
    pub pool: Pubkey,
    pub line_id: u64,
    pub payer: Pubkey,
    pub receipt_hash: [u8; 32],
    pub late_part: u64,
    pub interest_part: u64,
    pub principal_part: u64,
    pub entries: Vec<PaymentEntry>,
}

/// A `Recibo` entry written by `annotate_payment`: the receipt attached to the payment entry
/// `payment_entry_count` of the pagaré's chain, as entry `entry_count`.
#[event]
pub struct PagareReceiptAnnotated {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub entry_count: u16,
    pub payment_entry_count: u16,
    pub receipt_hash: [u8; 32],
}
