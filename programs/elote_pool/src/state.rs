//! Account layouts.

use anchor_lang::prelude::*;

pub mod seeds {
    /// `[POOL, creator, asset_mint]`
    pub const POOL: &[u8] = b"pool";
    /// `[LINE, pool, line_id (u64 LE)]`
    pub const LINE: &[u8] = b"line";
    /// `[VAULT, pool]`: the pool's asset token account.
    pub const VAULT: &[u8] = b"vault";
    /// `[DEAD, pool]`: the pool-owned share token account holding the dead shares.
    pub const DEAD: &[u8] = b"dead";
    /// `[PAGARE, line, draw_index (u16 LE)]`: the program's record of one pagaré.
    pub const PAGARE: &[u8] = b"pagare";
    /// `[PAGARE_ASSET, line, draw_index (u16 LE)]`: the Metaplex Core asset that is the pagaré.
    pub const PAGARE_ASSET: &[u8] = b"pagare_asset";
}

/// Shares the first deposit locks forever in the pool-owned dead account, so the supply never
/// returns below this floor and a donation cannot round later deposits down to nothing.
pub const DEAD_SHARES: u64 = 1_000;
/// The first deposit is at least this many times `DEAD_SHARES`, whatever the asset's decimals,
/// so the inflation attack keeps needing a donation of about a thousand times the victim's deposit.
pub const MIN_FIRST_DEPOSIT_DEAD_RATIO: u64 = 1_000;

/// Smallest first deposit for an asset with `decimals`: one whole unit (`10^decimals`), and never
/// fewer base units than `MIN_FIRST_DEPOSIT_DEAD_RATIO * DEAD_SHARES`, so the opening price of a
/// share cannot be set by a dust deposit on any asset. `None` when `10^decimals` overflows.
pub fn min_first_deposit(decimals: u8) -> Option<u64> {
    let one_unit = 10u64.checked_pow(u32::from(decimals))?;
    Some(one_unit.max(MIN_FIRST_DEPOSIT_DEAD_RATIO.checked_mul(DEAD_SHARES)?))
}
pub const BPS: u64 = 10_000;
/// Upper bound on the tranches of one line.
pub const MAX_DRAWS: u16 = 64;
/// The most pagaré records one `repay` takes as remaining accounts (`TooManySidecars` above it):
/// three keep the heaviest payment, every installment of a 120-installment line missed, at least
/// 5 000 CU under the 100 000 CU ceiling. A payment that would touch more is split by the client
/// into several `repay` calls; the amount is free.
pub const MAX_SIDECARS_PER_REPAY: usize = 3;
/// `CreditLine::sponsor_role`: a sponsor that refers, trains and shares data, with no payment
/// obligation.
pub const SPONSOR_PROMOTER: u8 = 0;
/// `CreditLine::sponsor_role`: a sponsor that takes full liability. Recorded only; the program
/// enforces nothing on it.
pub const SPONSOR_GUARANTOR: u8 = 1;
/// Upper bound on the installments of one line, so the arrears computation is bounded.
pub const MAX_INSTALLMENTS: u16 = 120;
/// The fewest missed installments a schedule may set before collection.
pub const MIN_MAX_MISSED: u8 = 3;
/// The shortest collection period a pool may set: 30 days.
pub const MIN_COLLECTION_PERIOD_SECONDS: u32 = 30 * 86_400;
/// The year that annualizes `late_bps`: 365 days.
pub const SECONDS_PER_YEAR: u64 = 365 * 86_400;
/// `CreditLine::currency`: Mexican pesos (ISO 4217 484). A line's amounts are centavos.
pub const CURRENCY_MXN: u16 = 484;
/// The Squads v4 program whose multisig vaults may be a pool's council.
pub const SQUADS_PROGRAM_ID: Pubkey = pubkey!("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
/// `Pool::council_kind`: the vault (index 0) of an autonomous Squads v4 multisig with a majority
/// threshold, checked on every council instruction.
pub const COUNCIL_SQUADS: u8 = 0;
/// `Pool::council_kind`: any signer, for a future governance PDA; `propose_council` and
/// `accept_council` refuse it until a verified kind exists.
pub const COUNCIL_GENERIC: u8 = 1;
/// The most members a council multisig may have, so the check's cost stays bounded.
pub const MAX_COUNCIL_MEMBERS: usize = 10;
/// How long a requested line waits for the council: `approve_line` refuses after
/// `requested_at + REQUEST_TTL_SECONDS`.
pub const REQUEST_TTL_SECONDS: i64 = 15 * 86_400;
/// The furthest a council decision's `valid_until` may lie ahead of the clock when it executes:
/// `council::fresh` refuses `valid_until > now + MAX_PROPOSAL_HORIZON_SECONDS` (`ProposalTooFar`).
pub const MAX_PROPOSAL_HORIZON_SECONDS: i64 = 15 * 86_400;
/// The shortest notice a pool may set between `propose_acceleration` and `accelerate_line`.
pub const MIN_ACCELERATION_NOTICE_SECONDS: u32 = 7 * 86_400;
#[cfg(all(feature = "devnet", feature = "localnet"))]
compile_error!("the cluster features `devnet` and `localnet` are exclusive");

/// The asset mints `init_pool` accepts (R2). Without a cluster feature, the mainnet list: a build
/// that forgets its feature stays on the strictest list.
#[cfg(not(any(feature = "devnet", feature = "localnet")))]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = Some(&[
    pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"), // USDC
    pubkey!("MEXeQE5UjuusUoyS2KxbUXp6o21YbbBJaNvPMMEfr4y"), // MEXe (Etherfuse)
]);
/// Devnet: the Etherfuse sandbox mints and Circle's devnet USDC.
#[cfg(feature = "devnet")]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = Some(&[
    pubkey!("MEXeDZ8UKQUzKK3yXcXFjHwCG2AAigdSTnaXPuxxchK"), // MEXe, Etherfuse sandbox
    pubkey!("BXTou3CvPxpFVAJvzvEZcAnRLGCHqT1LHKsFTSQft7s"), // USDC, Etherfuse sandbox (GET /supported_assets, 2026-11-08)
    pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"), // USDC, Circle
]);
/// Localnet (the tests and the local e2e): any mint the asset policy accepts.
#[cfg(feature = "localnet")]
pub const ASSET_ALLOWLIST: Option<&[Pubkey]> = None;

/// The decimals of every asset mint `init_pool` accepts: `fx` converts centavos to millionths.
pub const ASSET_DECIMALS: u8 = 6;

/// Whether `mint` is on this build's `ASSET_ALLOWLIST`.
pub fn asset_listed(mint: &Pubkey) -> bool {
    ASSET_ALLOWLIST.is_none_or(|list| list.contains(mint))
}

/// `Pool::asset_controls` bit: the asset mint has a freeze authority.
pub const ASSET_CONTROL_FREEZE: u8 = 1 << 0;
/// `Pool::asset_controls` bit: the asset mint carries the Token-2022 `Pausable` extension.
pub const ASSET_CONTROL_PAUSABLE: u8 = 1 << 1;

#[account]
#[derive(InitSpace)]
pub struct Pool {
    /// The vault of the council multisig: signs every council instruction.
    pub council: Pubkey,
    pub asset_mint: Pubkey,
    /// The share token mint; its mint authority is the pool PDA.
    pub share_mint: Pubkey,
    /// PDA token account `[VAULT, pool]` for `asset_mint`, owned by the pool PDA.
    pub vault: Pubkey,
    /// PDA share token account `[DEAD, pool]`, owned by the pool PDA; never burned.
    pub dead_shares: Pubkey,
    /// The series' Metaplex Core collection `draw` mints the pagaré assets into.
    pub collection: Pubkey,
    /// The cost of the principal drawn and neither repaid, condoned nor written off.
    pub outstanding_principal: u64,
    /// Ordinary interest repaid on every line; informative.
    pub interest_collected: u64,
    /// The cost of the principal written off or condoned on every line (decision 51), less what
    /// recoveries on written-off lines brought back of it: the loss still recognised; informative.
    pub written_off: u64,
    pub line_count: u64,
    /// Seconds from the mark (`CreditLine::accelerated_at`) before a line in collection can be
    /// written off; at least `MIN_COLLECTION_PERIOD_SECONDS`. Set with `set_collection_period`.
    pub collection_period_seconds: u32,
    /// The line cap, as a share of the NAV in basis points (`1..=BPS`): `approve_line` measures the
    /// principal (in MXN at the FIX) against the NAV at the approval, and every `draw` the line's
    /// USDC drawn, the tranche included, against the NAV at the draw. Set at `init_pool` and with
    /// `set_max_line_bps`.
    pub max_line_bps: u16,
    /// `ASSET_CONTROL_*` bits: issuer controls of the asset mint that can stop the vault without
    /// the program. Recorded at `init_pool`, never refused.
    pub asset_controls: u8,
    /// The share mint is `DefaultAccountState = Frozen`: a holder's account must be thawed by the
    /// mint's freeze authority before it can receive or burn shares. Recorded at `init_pool`.
    pub permissioned: bool,
    pub paused: bool,
    pub bump: u8,
    /// Paid for and seeds the pool; no other power.
    pub creator: Pubkey,
    /// `COUNCIL_SQUADS` or `COUNCIL_GENERIC`.
    pub council_kind: u8,
    /// The Squads multisig whose vault is the council; the zero key under `COUNCIL_GENERIC`.
    pub multisig: Pubkey,
    /// The bump of the council's vault PDA `["multisig", multisig, "vault", 0]`.
    pub council_vault_bump: u8,
    /// The authority `propose_council` named, until it signs `accept_council`; the zero key when
    /// none.
    pub pending_council: Pubkey,
    pub pending_council_kind: u8,
    /// The most principal the pool lends at once, in asset units; checked at every draw.
    pub max_outstanding_principal: u64,
    /// Seconds from `propose_acceleration` before `accelerate_line`; at least
    /// `MIN_ACCELERATION_NOTICE_SECONDS`. A proposal stores its own deadline.
    pub acceleration_notice_seconds: u32,
    /// The outstanding USDC cost of the lines in collection: the mark and the council's
    /// acceleration add a line's cost, each recovery subtracts what it recovered, and the
    /// write-off removes the rest as the loss (decision 40).
    pub impaired_principal: u64,
    /// The provision the NAV takes on `impaired_principal`, in basis points: `init_pool` writes
    /// `BPS` (100%), and only the council moves it (`set_provision_bps`).
    pub provision_bps: u16,
    /// The key that may publish the FIX within the band of `fx::within_band` from the one in force;
    /// the zero key when none (the council publishes). Set and rotated by the council.
    pub fx_feeder: Pubkey,
    /// The FIX in force: MXN per USD times `fx::FIX_SCALE`; zero before the first publication.
    pub fix_rate: u64,
    /// The date Banxico determined `fix_rate`, `yyyymmdd`; never moves back.
    pub fix_date: u32,
    /// When `fix_rate` was published, by the feeder or the council; the feeder publishes at most
    /// once per Mexico City day of it.
    pub fix_posted_at: i64,
    /// The FX gain (positive) or loss realized by repayments, in USDC base units; informative.
    pub fx_realized: i64,
    /// The lowest and highest FIX `post_fix` accepts from anyone, feeder or council; zero and zero
    /// for no bounds. Set at `init_pool` and with `set_fix_bounds`.
    pub min_fix_rate: u64,
    pub max_fix_rate: u64,
    /// The cost of the principal written off and still owed: `write_off` adds a line's loss, each
    /// recovery subtracts what it recovered and a quita on a written-off line what it condoned.
    /// Deposits pay for it at par (`entry_nav`, decision 51).
    pub recoverable: u64,
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 48],
}

impl Pool {
    /// Bit 0 of `asset_controls`: the asset's issuer can freeze the vault.
    pub fn asset_freezable(&self) -> bool {
        self.asset_controls & ASSET_CONTROL_FREEZE != 0
    }

    /// Bit 1 of `asset_controls`: the asset's issuer can pause every transfer of the mint.
    pub fn asset_pausable(&self) -> bool {
        self.asset_controls & ASSET_CONTROL_PAUSABLE != 0
    }

    /// `nav * max_line_bps / BPS`, rounded down: the largest line, in asset units, at this NAV.
    pub fn max_line_principal(&self, nav: u64) -> Option<u64> {
        (nav as u128)
            .checked_mul(self.max_line_bps as u128)?
            .checked_div(BPS as u128)
            .and_then(|v| u64::try_from(v).ok())
    }

    /// Whether the FIX is pinned by the bounds: `min == max != 0` leaves exactly one postable
    /// rate, so the rate cannot drift and age means nothing (decision 42). A pinned FIX is
    /// exempt from the `MAX_FIX_AGE_DAYS` staleness check in `approve_line` and `draw`.
    pub fn fix_pinned(&self) -> bool {
        self.min_fix_rate != 0 && self.min_fix_rate == self.max_fix_rate
    }

    /// The NAV with no provision: the vault plus the whole principal lent out, the impaired cost
    /// at par, plus the cost written off and still owed (`recoverable`, decision 51). Deposits are
    /// priced here (decision 40b, Maple's dual price): whoever enters while a line is impaired or
    /// written off buys that claim at par, so a later recovery hands the entrant nothing and the
    /// sandwich — flash or patient — nets zero or less, while deposits are never gated.
    pub fn entry_nav(&self, vault_amount: u64) -> Option<u64> {
        vault_amount.checked_add(self.outstanding_principal)?.checked_add(self.recoverable)
    }

    /// Net asset value at `vault_amount`: the vault plus the principal lent out, minus the
    /// provision on the lines in collection, `impaired_principal * provision_bps / BPS` rounded
    /// down (decision 40). Redemptions, the line caps and the invariant check price here. `None`
    /// on overflow, or when the provision exceeds what the pool holds.
    pub fn nav(&self, vault_amount: u64) -> Option<u64> {
        let provision = (self.impaired_principal as u128)
            .checked_mul(self.provision_bps as u128)?
            .checked_div(BPS as u128)
            .and_then(|v| u64::try_from(v).ok())?;
        vault_amount.checked_add(self.outstanding_principal)?.checked_sub(provision)
    }

    /// Invariants every money instruction re-checks on balances reloaded after its CPIs: the NAV
    /// (provision included) fits a `u64`, the provision never exceeds the pool, and a given share
    /// supply is zero or at least `DEAD_SHARES` (the dead shares never move). `InvariantBroken`
    /// otherwise. Returns the NAV it checked.
    pub fn check(&self, vault_amount: u64, share_supply: Option<u64>) -> Result<u64> {
        let nav = self.nav(vault_amount).ok_or(crate::errors::PoolError::InvariantBroken)?;
        require!(self.impaired_principal <= self.outstanding_principal, crate::errors::PoolError::InvariantBroken);
        if let Some(supply) = share_supply {
            require!(supply == 0 || supply >= DEAD_SHARES, crate::errors::PoolError::InvariantBroken);
        }
        Ok(nav)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum LineStatus {
    /// Requested by the borrower, waiting for the council's `approve_line` or `reject_line`.
    Requested,
    Approved,
    Active,
    Repaid,
    /// The council recognised its principal as a loss. Not terminal: the line still owes, takes
    /// recoveries and quitas, and becomes `Repaid` when it owes nothing (decision 51).
    WrittenOff,
    /// A scheduled line with `max_missed` or more missed installments, in out-of-court
    /// collection. Repayments are still accepted; it leaves this state only when repaid in full
    /// or written off.
    InCollection,
    /// An approved commitment nobody drew, cleared by the council (`cancel_line`). Terminal; nothing
    /// was ever owed.
    Cancelled,
}

#[account]
#[derive(InitSpace)]
pub struct CreditLine {
    pub pool: Pubkey,
    pub line_id: u64,
    pub borrower: Pubkey,
    /// Co-signed `request_line`; the zero key when the line has no sponsor, so every field keeps
    /// one offset.
    pub sponsor: Pubkey,
    /// `SPONSOR_PROMOTER` or `SPONSOR_GUARANTOR`; meaningful only when `sponsor` is not the zero
    /// key.
    pub sponsor_role: u8,
    /// Hash of the signed credit agreement.
    pub agreement_hash: [u8; 32],
    /// Salted merkle root of the credit file in the agreements engine. A commitment: no document,
    /// name or folio is derivable from it.
    pub expediente_root: [u8; 32],
    pub status: LineStatus,
    /// The approved commitment; `available = principal - drawn`.
    pub principal: u64,
    /// Annual ordinary interest rate, in basis points, on the principal outstanding, per second
    /// (ACTUAL/365) until the last due date or the acceleration (decision 48).
    pub rate_bps: u16,
    /// The maturity of a bullet line.
    pub term_end: i64,
    /// Last instant a draw is accepted (strict `<`): `first_due_at` on a scheduled line,
    /// `term_end` on a bullet line; `suspend_draws` brings it to now.
    pub availability_end: i64,
    /// Principal transferred to the borrower so far; `drawn <= principal`, never decreases.
    pub drawn: u64,
    /// Tranches drawn; at most `MAX_DRAWS`.
    pub draws: u16,
    /// `∫ (drawn - principal_repaid) dt` up to `accrued_at`, in centavo-seconds: advanced by
    /// `accrue` before every change of the outstanding principal and at the acceleration
    /// (decision 48). The ordinary interest accrued is read from it, rounded down once.
    pub principal_seconds: u128,
    /// The instant `principal_seconds` runs to; never past `accrual_end`.
    pub accrued_at: i64,
    /// Principal settled: repaid, or condoned by the council (decision 51), so it meets
    /// installments and stops accruing interest either way.
    pub principal_repaid: u64,
    /// Ordinary interest settled: repaid, or condoned by the council (decision 50).
    pub interest_repaid: u64,
    /// The index of the oldest pagaré no repayment has marked paid. `repay` requires the records
    /// `[pagares_paid ..= last_touched]` and moves it.
    pub pagares_paid: u16,
    /// The first due date; installment `k` is due at `first_due_at + (k - 1) * interval_seconds`.
    /// Zero on a bullet line.
    pub first_due_at: i64,
    pub interval_seconds: i64,
    /// Installments of the schedule, `1..=MAX_INSTALLMENTS`; zero on a bullet line.
    pub installments: u16,
    /// Annual late interest rate on the overdue principal, in basis points.
    pub late_bps: u16,
    /// Missed installments that allow `mark_in_collection`.
    pub max_missed: u8,
    pub late_collected: u64,
    /// Late interest is settled up to this instant; zero until a repayment collects some.
    pub late_paid_until: i64,
    /// Time of the first draw; informational (the calendar is `first_due_at`).
    pub drawn_at: i64,
    pub closed_at: i64,
    /// When `mark_in_collection` accelerated the balance; zero before. From then on every
    /// installment is due, and `write_off` waits `Pool::collection_period_seconds` from it.
    pub accelerated_at: i64,
    pub bump: u8,
    /// ISO 4217 code of every amount of the line: `CURRENCY_MXN`.
    pub currency: u16,
    pub requested_at: i64,
    pub approved_at: i64,
    /// `sha256` of the stored terms in the fixed encoding; the council's approval names it.
    pub terms_hash: [u8; 32],
    /// Hash of the approved pagaré text every draw's pagaré hash is derived from.
    pub pagare_template_hash: [u8; 32],
    /// Funded the request; receives the rent back on withdraw, reject or expiry.
    pub payer: Pubkey,
    /// The `availability_end` before `suspend_draws` or `propose_acceleration` moved it, which
    /// `resume_draws` restores; zero when draws are not suspended. Suspended ⇔ non-zero.
    pub availability_end_saved: i64,
    /// When `accelerate_line` may fire, set by `propose_acceleration`; zero when none is pending.
    /// `withdraw_acceleration` and `mark_in_collection` clear it, `accelerate_line` keeps it: with
    /// `accelerated_at` it tells the route, the council's (non-zero) or the objective one (zero).
    pub acceleration_due_at: i64,
    /// USDC delivered over the draws: the pool's cost of the line.
    pub usdc_drawn: u64,
    /// The cost of the principal settled (repaid or condoned), at each pagaré's own FIX;
    /// `usdc_drawn - usdc_cost_repaid` is the cost still outstanding.
    pub usdc_cost_repaid: u64,
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 64],
}

/// How the pagaré text was signed: the borrower's wallet alone.
pub const SIGNATURE_WALLET: u8 = 0;
/// A PSC (prestador de servicios de certificación) certificate and the wallet.
pub const SIGNATURE_PSC_WALLET: u8 = 1;
/// The SAT e.firma and the wallet.
pub const SIGNATURE_EFIRMA_WALLET: u8 = 2;

/// `Pagare::status`: the título is outstanding (the asset exists, held by the pool or endorsed).
pub const PAGARE_OUTSTANDING: u8 = 0;
/// `Pagare::status`: a repayment covered it (the PAGADO mark), or the council's quita extinguished
/// it (`PAGARE_FLAG_CONDONED`, decision 51). The título's delivery to the
/// suscriptor — or to the aval who settled it — is `PAGARE_FLAG_DELIVERED`, written by
/// `deliver_pagare` (decision 38) once the line owes nothing (decision 49). Status 2 was PAID_PENDING_CANCEL (the retired cancellation
/// route); never reuse it.
pub const PAGARE_PAID: u8 = 1;

// `Pagare::flags` bit 0 was the owner-managed `BurnDelegate` the retired procuración route added
// to the asset; never reuse it.
/// `Pagare::flags` bit: an endoso en procuración is in force.
pub const PAGARE_FLAG_PROCURACION: u8 = 1 << 1;
/// `Pagare::flags` bit: the paid título was delivered (`deliver_pagare`); terminal.
pub const PAGARE_FLAG_DELIVERED: u8 = 1 << 2;
/// `Pagare::flags` bit: the aval (`CreditLine.sponsor` with `SPONSOR_GUARANTOR`) made the payment
/// that covered the título, so the delivery goes to the aval, who holds it alive. What the aval
/// may collect from the suscriptor — at most what it paid — is a judge's call (LGTOC 115): the
/// pool only records the payments, and each entry states who paid what.
pub const PAGARE_FLAG_PAID_BY_AVAL: u8 = 1 << 3;
// `Pagare::flags` bit 4 was briefly a judicial hold, retired with decision 38-g (the judicial
// stage runs off chain); never reuse it.
/// `Pagare::flags` bit: the council's quita extinguished what was left of the título (decision 51,
/// CCF 2209): it is `PAGARE_PAID` without a payment, so no payer, aval included, is named; it goes
/// to the suscriptor.
pub const PAGARE_FLAG_CONDONED: u8 = 1 << 5;

/// Registry entry kinds (`Pagare::last_kind`, `PagareEntry::kind`).
// Codes 1 and 8 are reserved for the endoso en propiedad and the late endoso, and 10 was the
// holder's cancellation request (the backstop, removed); never reuse them.
pub const KIND_EMISION: u8 = 0;
pub const KIND_PROCURACION: u8 = 2;
pub const KIND_REVOCACION: u8 = 3;
pub const KIND_TERMINACION: u8 = 4;
pub const KIND_ABONO: u8 = 5;
pub const KIND_ANOTACION_JUDICIAL: u8 = 6;
pub const KIND_CONSTANCIA_JUDICIAL: u8 = 7;
pub const KIND_PAGADO: u8 = 9;
pub const KIND_RECIBO: u8 = 11;
/// The payment that covered the título was the aval's (LGTOC 115).
pub const KIND_PAGADO_AVAL: u8 = 12;
/// The paid título was delivered, to the suscriptor or to the aval who settled it.
pub const KIND_ENTREGA: u8 = 13;
/// The council's quita remitted principal of the título (decision 51); never a payment.
pub const KIND_CONDONACION: u8 = 14;

/// The program's record of one pagaré, beside the Core asset `draw` mints. It holds no folio and
/// nothing personal, and is never closed: after the payment, the delivery and even the owner's
/// `burn_pagare` it stays as the PAGADO mark and the registry.
#[account]
#[derive(InitSpace)]
pub struct Pagare {
    pub line: Pubkey,
    /// Which tranche of the line, 0-based.
    pub draw_index: u16,
    /// The Core asset at `[PAGARE_ASSET, line, draw_index]`.
    pub asset: Pubkey,
    /// Hash of the signed pagaré data message, as `draw` received it.
    pub pagare_hash: [u8; 32],
    /// The tranche this pagaré documents, in centavos of MXN: the sum the pagaré promises.
    pub amount_mxn: u64,
    pub issued_at: i64,
    /// The line's last due date at issuance.
    pub due_at: i64,
    /// The line's `drawn` after this tranche. Pagarés are covered oldest first, so pagaré k is
    /// paid once the line's `principal_repaid` reaches it.
    pub drawn_through: u64,
    /// `sha256(salt || fingerprint)` of the PSC or e.firma certificate that signed the pagaré text,
    /// with the salt kept in the expediente: a bare fingerprint can be looked up to the signer's
    /// RFC. Zero only when `signature_method` is `SIGNATURE_WALLET`.
    pub cert_fingerprint: [u8; 32],
    /// `SIGNATURE_*`.
    pub signature_method: u8,
    /// `PAGARE_OUTSTANDING` or `PAGARE_PAID` (paid, or condoned: `PAGARE_FLAG_CONDONED`); never reverts.
    pub status: u8,
    /// When the repayment or the quita that covered it landed; zero while outstanding.
    pub paid_at: i64,
    /// The receipt hash of that repayment, the payer's and provisional: the holder's correction
    /// arrives as a `Recibo` entry (`annotate_payment`) and no field is rewritten (LGTOC 41). For a
    /// título the quita extinguished, the hash of the council's written record.
    pub receipt_hash: [u8; 32],
    /// The procurador of the endoso en procuración in force; the zero key when none. Cleared by
    /// `revoke_procuracion` and by the extinction.
    pub procurador: Pubkey,
    /// `PAGARE_FLAG_*` bits.
    pub flags: u8,
    /// The registry: `sha256(log_root || entry_hash)` over every entry, the issuance entry first.
    pub log_root: [u8; 32],
    /// Entries in the chain; the issuance entry is 1.
    pub entry_count: u32,
    /// `KIND_*` of the last entry.
    pub last_kind: u8,
    pub bump: u8,
    /// The FIX the borrower signed for this draw (`Pool::fix_rate` then) and its date.
    pub fix_rate: u64,
    pub fix_date: u32,
    /// USDC base units delivered: `fx::usdc_for_mxn(amount_mxn, fix_rate)`. The pool's cost of the
    /// tranche.
    pub usdc_delivered: u64,
    /// `sha256(pagare_hash || fix_rate || fix_date || usdc_delivered)`: the delivery the borrower
    /// instructed, kept apart from the pagaré, which promises MXN only.
    pub delivery_hash: [u8; 32],
    /// Zero; for fields added after mainnet.
    pub reserved: [u8; 28],
}

/// Installment terms requested by `request_line`; fixed for the life of the line.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduleParams {
    /// The first due date (unix seconds), from the signed contract; must be after the opening.
    pub first_due_at: i64,
    pub interval_seconds: i64,
    /// `drawn` is repaid in this many installments of principal (decision 48); ordinary interest
    /// is taken first from every payment.
    pub installments: u16,
    /// Missed installments after which anyone may move the line to collection.
    pub max_missed: u8,
}

/// The schedule terms of a scheduled line, read from its fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Schedule {
    pub interval_seconds: i64,
    pub installments: u16,
    pub late_bps: u16,
    pub max_missed: u8,
}

impl CreditLine {
    /// The installment schedule; `None` on a bullet line (`installments == 0`).
    pub fn schedule(&self) -> Option<Schedule> {
        (self.installments > 0).then_some(Schedule {
            interval_seconds: self.interval_seconds,
            installments: self.installments,
            late_bps: self.late_bps,
            max_missed: self.max_missed,
        })
    }

    /// The instant one interval before the first due date, from which `math` counts the
    /// installments (`anchor + k * interval`): the single place the calendar is anchored. `None`
    /// on a bullet line.
    pub fn anchor(&self) -> Option<i64> {
        let s = self.schedule()?;
        self.first_due_at.checked_sub(s.interval_seconds)
    }

    /// When the last payment is owed: `first_due_at + (installments - 1) * interval` on a scheduled
    /// line, `term_end` on a bullet line.
    pub fn last_due(&self) -> Option<i64> {
        match self.schedule() {
            Some(s) => s
                .interval_seconds
                .checked_mul(i64::from(s.installments).checked_sub(1)?)
                .and_then(|span| self.first_due_at.checked_add(span)),
            None => Some(self.term_end),
        }
    }

    /// `principal - drawn`.
    pub fn available(&self) -> Option<u64> {
        self.principal.checked_sub(self.drawn)
    }

    /// The principal outstanding: `drawn - principal_repaid`.
    pub fn outstanding(&self) -> Option<u64> {
        self.drawn.checked_sub(self.principal_repaid)
    }

    /// Where ordinary interest stops (decision 48): the last due date, or the acceleration when it
    /// came first. After it only late interest runs.
    pub fn accrual_end(&self) -> Option<i64> {
        let last = self.last_due()?;
        Some(if self.accelerated_at > 0 { last.min(self.accelerated_at) } else { last })
    }

    /// `principal_seconds` carried from `accrued_at` to `min(now, accrual_end)` at the principal
    /// outstanding.
    fn principal_seconds_at(&self, now: i64) -> Option<u128> {
        let t = now.min(self.accrual_end()?);
        let elapsed = if t > self.accrued_at { u128::try_from(t.checked_sub(self.accrued_at)?).ok()? } else { 0 };
        self.principal_seconds.checked_add(u128::from(self.outstanding()?).checked_mul(elapsed)?)
    }

    /// Brings the accrual to `now` (decision 48); called before every change of the outstanding
    /// principal and before `accelerated_at` is set, so each second accrues on the balance it had.
    /// The interest accrued at `now` is the same before and after.
    pub fn accrue(&mut self, now: i64) -> Option<()> {
        let t = now.min(self.accrual_end()?);
        self.principal_seconds = self.principal_seconds_at(now)?;
        self.accrued_at = self.accrued_at.max(t);
        Some(())
    }

    /// The ordinary interest accrued by `now`, repaid or not: `rate_bps * ∫ outstanding dt /
    /// (BPS * SECONDS_PER_YEAR)` to `min(now, accrual_end)`, rounded down once.
    pub fn interest_accrued(&self, now: i64) -> Option<u64> {
        crate::math::ordinary_interest(self.rate_bps, self.principal_seconds_at(now)?)
    }

    /// The ordinary interest accrued by `now` and not yet settled (repaid or condoned).
    pub fn interest_unpaid(&self, now: i64) -> Option<u64> {
        self.interest_accrued(now)?.checked_sub(self.interest_repaid)
    }

    /// What the line owes at `now`, late interest aside: the principal outstanding and the
    /// ordinary interest accrued and unpaid.
    pub fn owed(&self, now: i64) -> Option<u64> {
        self.outstanding()?.checked_add(self.interest_unpaid(now)?)
    }

    /// Whether the line can still draw at `now`: active, the window open (strictly) and something
    /// left of the commitment.
    pub fn can_draw(&self, now: i64) -> Option<bool> {
        Some(self.status == LineStatus::Active && now < self.availability_end && self.available()? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(outstanding_principal: u64) -> Pool {
        Pool {
            council: Pubkey::default(),
            asset_mint: Pubkey::default(),
            share_mint: Pubkey::default(),
            vault: Pubkey::default(),
            dead_shares: Pubkey::default(),
            collection: Pubkey::default(),
            outstanding_principal,
            interest_collected: 0,
            written_off: 0,
            line_count: 0,
            collection_period_seconds: 0,
            max_line_bps: 0,
            asset_controls: 0,
            permissioned: false,
            paused: false,
            bump: 0,
            creator: Pubkey::default(),
            council_kind: COUNCIL_SQUADS,
            multisig: Pubkey::default(),
            council_vault_bump: 0,
            pending_council: Pubkey::default(),
            pending_council_kind: 0,
            max_outstanding_principal: 0,
            acceleration_notice_seconds: 0,
            impaired_principal: 0,
            provision_bps: 0,
            fx_feeder: Pubkey::default(),
            fix_rate: 0,
            fix_date: 0,
            fix_posted_at: 0,
            fx_realized: 0,
            recoverable: 0,
            min_fix_rate: 0,
            max_fix_rate: 0,
            reserved: [0; 48],
        }
    }

    #[test]
    fn check_accepts_a_consistent_pool_and_refuses_a_broken_one() {
        let p = pool(5);
        assert!(p.check(10, None).is_ok(), "the NAV fits");
        assert!(p.check(10, Some(0)).is_ok(), "nothing deposited yet");
        assert!(p.check(10, Some(DEAD_SHARES)).is_ok(), "the dead shares alone are a valid floor");
        assert!(p.check(10, Some(DEAD_SHARES + 1)).is_ok());
        assert!(p.check(10, Some(DEAD_SHARES - 1)).is_err(), "a supply below the dead shares is impossible");
        assert!(p.check(10, Some(1)).is_err());
        assert!(p.check(u64::MAX, None).is_err(), "vault + outstanding overflows");
        assert!(pool(0).check(u64::MAX, Some(DEAD_SHARES)).is_ok(), "the maximum NAV itself is fine");
    }

    #[test]
    fn nav_takes_the_provision_and_check_bounds_the_impairment() {
        let mut p = pool(100);
        p.impaired_principal = 40;
        p.provision_bps = BPS as u16;
        assert_eq!(p.nav(10), Some(70), "a full provision removes the whole impairment");
        p.provision_bps = (BPS / 2) as u16;
        assert_eq!(p.nav(10), Some(90));
        p.provision_bps = 0;
        assert_eq!(p.nav(10), Some(110), "no provision, the old NAV");
        p.provision_bps = BPS as u16;
        assert!(p.check(10, None).is_ok());
        p.impaired_principal = 101;
        assert!(p.check(10, None).is_err(), "more impaired than outstanding is impossible");
        assert_eq!(pool(0).nav(u64::MAX), Some(u64::MAX));
    }

    #[test]
    #[cfg(not(any(feature = "devnet", feature = "localnet")))]
    fn without_a_cluster_feature_only_mainnet_usdc_and_mexe_are_listed() {
        assert!(asset_listed(&pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")));
        assert!(asset_listed(&pubkey!("MEXeQE5UjuusUoyS2KxbUXp6o21YbbBJaNvPMMEfr4y")));
        assert!(!asset_listed(&pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")), "devnet USDC is not mainnet's");
        assert!(!asset_listed(&Pubkey::new_unique()));
    }

    #[test]
    #[cfg(feature = "devnet")]
    fn devnet_lists_the_sandbox_mints_and_circles_usdc() {
        assert!(asset_listed(&pubkey!("MEXeDZ8UKQUzKK3yXcXFjHwCG2AAigdSTnaXPuxxchK")));
        assert!(asset_listed(&pubkey!("BXTou3CvPxpFVAJvzvEZcAnRLGCHqT1LHKsFTSQft7s")));
        assert!(asset_listed(&pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU")));
        assert!(!asset_listed(&pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")));
        assert!(!asset_listed(&Pubkey::new_unique()));
    }

    #[test]
    #[cfg(feature = "localnet")]
    fn localnet_lists_any_mint() {
        assert!(asset_listed(&Pubkey::new_unique()));
    }

    const MONTH: i64 = SECONDS_PER_YEAR as i64 / 12;

    /// A bullet line of `principal` at 12% ending a year after t = 1_000_000, nothing drawn yet.
    fn line(principal: u64) -> CreditLine {
        CreditLine {
            pool: Pubkey::default(),
            line_id: 0,
            borrower: Pubkey::default(),
            sponsor: Pubkey::default(),
            sponsor_role: 0,
            agreement_hash: [0; 32],
            expediente_root: [0; 32],
            status: LineStatus::Active,
            principal,
            rate_bps: 1_200,
            term_end: 1_000_000 + 12 * MONTH,
            availability_end: 1_000_000 + 12 * MONTH,
            drawn: 0,
            draws: 0,
            principal_seconds: 0,
            accrued_at: 0,
            principal_repaid: 0,
            interest_repaid: 0,
            pagares_paid: 0,
            first_due_at: 0,
            interval_seconds: 0,
            installments: 0,
            late_bps: 0,
            max_missed: 0,
            late_collected: 0,
            late_paid_until: 0,
            drawn_at: 0,
            closed_at: 0,
            accelerated_at: 0,
            bump: 0,
            currency: CURRENCY_MXN,
            requested_at: 0,
            approved_at: 0,
            terms_hash: [0; 32],
            pagare_template_hash: [0; 32],
            payer: Pubkey::default(),
            availability_end_saved: 0,
            acceleration_due_at: 0,
            usdc_drawn: 0,
            usdc_cost_repaid: 0,
            reserved: [0; 64],
        }
    }

    fn draw(l: &mut CreditLine, now: i64, amount: u64) {
        l.accrue(now).unwrap();
        l.drawn += amount;
    }

    fn repay_principal(l: &mut CreditLine, now: i64, amount: u64) {
        l.accrue(now).unwrap();
        l.principal_repaid += amount;
    }

    /// Decision 48: 60,000.00 at 12% accrues 600.00 a month; repaying 10,000.00 of principal lowers
    /// the next month to 500.00; the accrual stops at the term.
    #[test]
    fn interest_accrues_on_the_outstanding_principal_and_stops_at_the_end() {
        let t0 = 1_000_000;
        let mut l = line(6_000_000);
        assert_eq!(l.interest_accrued(t0 + MONTH), Some(0), "nothing drawn");
        draw(&mut l, t0, 6_000_000);
        assert_eq!(l.interest_accrued(t0), Some(0));
        assert_eq!(l.interest_accrued(t0 + MONTH), Some(60_000));
        repay_principal(&mut l, t0 + MONTH, 1_000_000);
        assert_eq!(l.interest_accrued(t0 + MONTH), Some(60_000), "accrue leaves the accrual unchanged");
        assert_eq!(l.interest_accrued(t0 + 2 * MONTH), Some(110_000));
        l.interest_repaid = 60_000;
        assert_eq!(l.interest_unpaid(t0 + 2 * MONTH), Some(50_000));
        assert_eq!(l.owed(t0 + 2 * MONTH), Some(5_050_000));
        // Twelve months, the first at 6.0 and eleven at 5.0: 60,000 + 11 * 50,000; nothing after.
        assert_eq!(l.interest_accrued(t0 + 12 * MONTH), Some(610_000));
        assert_eq!(l.interest_accrued(t0 + 40 * MONTH), Some(610_000));
        assert_eq!(l.accrual_end(), Some(t0 + 12 * MONTH));
    }

    /// The accrual is exact under any number of operations: a second-by-second accrue gives the
    /// figure a single read gives, rounded once (1.5 centavos a second, never 1).
    #[test]
    fn accruing_every_second_rounds_nothing_away() {
        let t0 = 1_000_000;
        let mut l = line(394_200_000);
        draw(&mut l, t0, 394_200_000); // 394,200,000 * 12% / 31,536,000 = 1.5 a second
        let single = l.interest_accrued(t0 + 3_600);
        assert_eq!(l.interest_accrued(t0 + 1), Some(1), "1.5 rounded down, once");
        for s in 1..=3_600 {
            l.accrue(t0 + s).unwrap();
        }
        assert_eq!((single, l.interest_accrued(t0 + 3_600)), (Some(5_400), Some(5_400)));
    }

    /// The acceleration stops the ordinary accrual at the mark; a clock before `accrued_at` reads
    /// nothing new.
    #[test]
    fn the_acceleration_ends_the_accrual() {
        let t0 = 1_000_000;
        let mut l = line(6_000_000);
        draw(&mut l, t0, 6_000_000);
        l.accrue(t0 + 2 * MONTH).unwrap();
        l.accelerated_at = t0 + 2 * MONTH;
        assert_eq!(l.accrual_end(), Some(t0 + 2 * MONTH));
        assert_eq!(l.interest_accrued(t0 + 9 * MONTH), Some(120_000));
        assert_eq!(l.interest_accrued(t0 + MONTH), Some(120_000), "never below what was accrued");
        // An acceleration after the last due date leaves the end there.
        let mut l = line(6_000_000);
        draw(&mut l, t0, 6_000_000);
        l.accelerated_at = t0 + 20 * MONTH;
        assert_eq!(l.interest_accrued(t0 + 30 * MONTH), Some(720_000));
    }

    #[test]
    fn can_draw_needs_an_active_line_an_open_window_and_something_left() {
        let t0 = 1_000_000;
        let mut l = line(6_000_000);
        draw(&mut l, t0, 3_000_000);
        assert_eq!(l.can_draw(t0), Some(true));
        assert_eq!(l.can_draw(l.availability_end), Some(false), "the window closes strictly");
        l.status = LineStatus::InCollection;
        assert_eq!(l.can_draw(t0), Some(false));
        l.status = LineStatus::Active;
        l.drawn = 6_000_000;
        assert_eq!(l.can_draw(t0), Some(false));
    }

    #[test]
    fn min_first_deposit_is_one_unit_with_a_dead_shares_floor() {
        assert_eq!(min_first_deposit(6), Some(1_000_000));
        assert_eq!(min_first_deposit(9), Some(1_000_000_000));
        assert_eq!(min_first_deposit(2), Some(1_000_000), "the floor, not 100");
        assert_eq!(min_first_deposit(0), Some(1_000_000));
        assert_eq!(min_first_deposit(19), Some(10u64.pow(19)));
        assert_eq!(min_first_deposit(20), None, "10^20 does not fit a u64");
    }
}
