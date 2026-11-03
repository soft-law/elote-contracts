//! Events, all emitted with `emit_cpi!`.

use anchor_lang::prelude::*;

/// Every parameter `init_pool` recorded. `asset_controls` are the `ASSET_CONTROL_*` bits;
/// `permissioned`: the share mint is frozen by default.
#[event]
pub struct PoolInitialized {
    pub pool: Pubkey,
    pub admin: Pubkey,
    pub attestor: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub collection: Pubkey,
    pub grace_period_seconds: u32,
    pub collection_period_seconds: u32,
    pub cancel_delay_seconds: u32,
    pub max_line_bps: u16,
    pub asset_controls: u8,
    pub permissioned: bool,
}

/// The pool's collection period after `set_collection_period`: seconds from the mark before a
/// line in collection can be written off.
#[event]
pub struct CollectionPeriodSet {
    pub pool: Pubkey,
    pub collection_period_seconds: u32,
}

/// The pool's cancel delay after `set_cancel_delay`: the wait from a cancellation request before
/// the holder may cancel alone.
#[event]
pub struct CancelDelaySet {
    pub pool: Pubkey,
    pub cancel_delay_seconds: u32,
}

/// The pool's attestor after `set_attestor`.
#[event]
pub struct AttestorSet {
    pub pool: Pubkey,
    pub attestor: Pubkey,
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

/// Every term `open_line` recorded. On a bullet line `installments`, `interval_seconds`,
/// `late_bps`, `max_missed` and `first_due_at` are zero; `sponsor_role` is meaningful only when
/// `sponsor` is named.
#[event]
pub struct LineOpened {
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    pub sponsor: Option<Pubkey>,
    pub sponsor_role: u8,
    pub agreement_hash: [u8; 32],
    pub expediente_root: [u8; 32],
    pub principal: u64,
    pub rate_bps: u16,
    pub term_end: i64,
    pub availability_end: i64,
    pub first_due_at: i64,
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
}

/// Emitted by `draw` after `LineDrawn`: the pagaré issued as the Metaplex Core asset `asset`,
/// owned by `holder` (the pool PDA) and frozen. `due_at` is the line's last due date at issuance.
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

/// One entry of a pagaré's registry: the full content the sidecar's hash chain commits to.
/// `stated_at` is the date the entry states and `recorded_at` the slot's time; every instruction
/// writes both as the slot's time.
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
    pub entry_count: u32,
    pub recorded_at: i64,
    pub stated_at: i64,
}

/// Emitted by `cancel_pagare`, before the burn. `holder` is the key that extinguished the pagaré
/// (the admin or the procurador); the sidecar stays as the PAGADO mark.
#[event]
pub struct PagareCancelled {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub receipt_hash: [u8; 32],
    pub holder: Pubkey,
    /// The holder cancelled alone after a request's deadline, without the attestor.
    pub backstop: bool,
}

/// Emitted by `request_cancellation`, after its `PagareEntry`: the holder may cancel alone with
/// `receipt_hash` once `cancel_after` has passed.
#[event]
pub struct CancellationRequested {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub asset: Pubkey,
    pub receipt_hash: [u8; 32],
    pub cancel_after: i64,
}

/// `available_forfeited` is the undrawn remainder.
#[event]
pub struct LineDrawsSuspended {
    pub pool: Pubkey,
    pub line_id: u64,
    pub available_forfeited: u64,
}

#[event]
pub struct LineCancelled {
    pub pool: Pubkey,
    pub line_id: u64,
}

/// One tranche, `amount`, and the line after it; `outstanding_after` is the pool's.
#[event]
pub struct LineDrawn {
    pub pool: Pubkey,
    pub line_id: u64,
    pub draw_index: u16,
    pub amount: u64,
    pub drawn: u64,
    pub available: u64,
    pub interest_due: u64,
    pub availability_end: i64,
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

/// A scheduled line moved to collection and its balance accelerated. `remaining` is the principal
/// and interest still due after the acceleration, without late interest; `interest_forgiven` is
/// the ordinary interest of the installments not yet due at the mark, which the line no longer
/// owes.
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
    pub entry_count: u32,
    pub principal_applied: u64,
}

/// The payment entries of one `repay`: the payer's receipt hash (zero means "receipt
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
    pub entry_count: u32,
    pub payment_entry_count: u32,
    pub receipt_hash: [u8; 32],
}

/// Every term `request_line` stored, with the hash of their fixed encoding. On a bullet line the
/// schedule fields are zero; `payer` funded the request and gets its rent back.
#[event]
pub struct LineRequested {
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    pub payer: Pubkey,
    pub sponsor: Option<Pubkey>,
    pub sponsor_role: u8,
    pub agreement_hash: [u8; 32],
    pub expediente_root: [u8; 32],
    pub pagare_template_hash: [u8; 32],
    pub terms_hash: [u8; 32],
    pub principal: u64,
    pub rate_bps: u16,
    pub term_end: i64,
    pub first_due_at: i64,
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
}

/// The council approved exactly this expediente and these terms.
#[event]
pub struct LineApproved {
    pub pool: Pubkey,
    pub line_id: u64,
    pub expediente_root: [u8; 32],
    pub terms_hash: [u8; 32],
    pub availability_end: i64,
}

#[event]
pub struct LineRejected {
    pub pool: Pubkey,
    pub line_id: u64,
}

#[event]
pub struct RequestWithdrawn {
    pub pool: Pubkey,
    pub line_id: u64,
}

/// The council proposed to collect the line early; draws are suspended at once, and
/// `accelerate_line` may fire from `due_at`. The notice is addressed to the borrower.
#[event]
pub struct AccelerationProposed {
    pub pool: Pubkey,
    pub line_id: u64,
    pub due_at: i64,
}

/// The proposal is withdrawn; draws stay suspended until `resume_draws`.
#[event]
pub struct AccelerationWithdrawn {
    pub pool: Pubkey,
    pub line_id: u64,
}

/// The council accelerated the line after the notice: the same effect as `LineInCollection`,
/// without missed installments.
#[event]
pub struct LineAccelerated {
    pub pool: Pubkey,
    pub line_id: u64,
    pub remaining: u64,
    pub interest_forgiven: u64,
}

/// Draws resumed with the window they had before the suspension.
#[event]
pub struct LineDrawsResumed {
    pub pool: Pubkey,
    pub line_id: u64,
    pub availability_end: i64,
}

/// `council` must sign `accept_council` before it becomes the pool's council.
#[event]
pub struct CouncilProposed {
    pub pool: Pubkey,
    pub council: Pubkey,
    pub kind: u8,
}

#[event]
pub struct CouncilAccepted {
    pub pool: Pubkey,
    pub old: Pubkey,
    pub new: Pubkey,
    pub kind: u8,
}

#[event]
pub struct PoolResumed {
    pub pool: Pubkey,
}

/// The zero key means no guardian.
#[event]
pub struct GuardianSet {
    pub pool: Pubkey,
    pub guardian: Pubkey,
}

#[event]
pub struct MaxLineBpsSet {
    pub pool: Pubkey,
    pub max_line_bps: u16,
}

#[event]
pub struct OutstandingCapSet {
    pub pool: Pubkey,
    pub max_outstanding_principal: u64,
}

#[event]
pub struct AccelerationNoticeSet {
    pub pool: Pubkey,
    pub acceleration_notice_seconds: u32,
}

#[event]
pub struct CancelGraceSet {
    pub pool: Pubkey,
    pub cancel_grace_seconds: u32,
}
