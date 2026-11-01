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

/// Emitted by `init_pool` after `PoolConfigured` (a new event, since events are append-only):
/// the concentration cap, the largest line as a share of the NAV in basis points.
#[event]
pub struct PoolLineCapSet {
    pub pool: Pubkey,
    pub max_line_bps: u16,
}

/// Emitted by `init_pool` after `PoolLineCapSet`: the read-only facts about the two mints.
/// `asset_controls`: the issuer controls on the asset mint, as `Pool::asset_controls` bits
/// (bit 0 freeze authority, bit 1 Pausable). `permissioned`: the share mint is frozen by
/// default, so holders must be thawed (Token ACL).
#[event]
pub struct PoolFlags {
    pub pool: Pubkey,
    pub asset_controls: u8,
    pub permissioned: bool,
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

/// The NAV and the share supply after every instruction that moves either (`deposit`,
/// `redeem`, `repay`, `write_off`), read back after the CPIs. For indexers, the shelf page and a
/// later secondary venue: `price per share = total_assets / share_supply`.
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

/// Emitted by `open_line` after `LineOpened` (and `LineScheduled`): the facts of a version-3
/// opening that the earlier events cannot carry. `first_due_at` is zero on a bullet line.
#[event]
pub struct LineTermsSet {
    pub pool: Pubkey,
    pub line_id: u64,
    pub first_due_at: i64,
    pub availability_end: i64,
    pub expediente_root: [u8; 32],
}

/// Emitted by `open_line` after `LineTermsSet`: which kind of promise the sponsor made
/// (decision 19, D12). `role` is `SPONSOR_PROMOTER` or `SPONSOR_GUARANTOR`; emitted with
/// `sponsor == None` and role 0 when no sponsor is named.
#[event]
pub struct LineSponsorRole {
    pub pool: Pubkey,
    pub line_id: u64,
    pub sponsor: Option<Pubkey>,
    pub role: u8,
}

/// Emitted by `draw` after `LineDrawState` (decision 24): the pagaré of this disposición was
/// issued as the Metaplex Core asset `asset`, held frozen by `holder` (the pool PDA, the series
/// vehicle's custody account). `due_at` is the line's last due date at issuance.
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

/// Emitted by `suspend_draws` (D10): the admin closed the draw window early; `available_forfeited`
/// is the undrawn remainder that can no longer be drawn. Repayment is untouched.
#[event]
pub struct LineDrawsSuspended {
    pub pool: Pubkey,
    pub line_id: u64,
    pub available_forfeited: u64,
}

/// Emitted by `cancel_line` (decision 25): an approved commitment nobody drew is cleared.
#[event]
pub struct LineCancelled {
    pub pool: Pubkey,
    pub line_id: u64,
}

/// One tranche. Since line v3 `amount` is the tranche, not the whole principal, and `draw`
/// emits it once per tranche, so indexers that sum it keep working.
#[event]
pub struct LineDrawn {
    pub pool: Pubkey,
    pub line_id: u64,
    pub amount: u64,
    pub outstanding_after: u64,
}

/// Emitted by `draw` after `LineDrawn`: the line after the tranche and the hash of the signed
/// pagaré of this disposición (D9, decision 13). The hash is not stored on the line.
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
