//! Shared LiteSVM harness for the `elote_pool` integration tests.
//!
//! Everything here builds state the way a client would: real instructions, signed by real
//! keypairs, executed by the compiled `target/deploy/elote_pool.so`. SPL Token and the Associated
//! Token Account program come from `LiteSVM::new()`, which loads them by default in litesvm 0.11.0
//! (`spl_token-3.5.0.so`, or its p-token build when the `replace_spl_token_with_p_token` feature
//! is active, and `spl_associated_token_account-1.1.1.so`; see `litesvm/src/programs/mod.rs`).

#![allow(dead_code)]

use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::{
    get_associated_token_address, get_associated_token_address_with_program_id,
    spl_associated_token_account::instruction as ata_ix,
};
// The Token-2022 interface crate's instruction builders accept both token program ids, so the
// harness uses them for classic and Token-2022 mints alike.
use anchor_spl::token_2022::spl_token_2022;
use spl_token_2022::extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_instruction::Instruction;
use solana_pubkey::{pubkey, Pubkey};
pub use keys::Keypair;
use solana_signer::Signer;
use elote_pool::errors::PoolError;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

/// Test signers. The council is a Squads vault PDA, which signs only inside Squads' execution, so
/// the harness runs with signature verification off and lets a `Pda` key "sign" with a random
/// signature; the program sees only the signer flag of each account, exactly as on chain.
/// `tests/squads.rs` runs the council through the real Squads binary.
pub mod keys {
    use solana_pubkey::Pubkey;
    use solana_signature::Signature;
    use solana_signer::{Signer, SignerError};

    pub enum Keypair {
        Real(solana_keypair::Keypair),
        Pda(Pubkey),
    }

    impl Keypair {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self::Real(solana_keypair::Keypair::new())
        }
        /// A signer for an address nobody holds a key for (a Squads vault).
        pub fn pda(key: Pubkey) -> Self {
            Self::Pda(key)
        }
        pub fn insecure_clone(&self) -> Self {
            match self {
                Self::Real(k) => Self::Real(k.insecure_clone()),
                Self::Pda(k) => Self::Pda(*k),
            }
        }
    }

    impl Signer for Keypair {
        fn try_pubkey(&self) -> Result<Pubkey, SignerError> {
            match self {
                Self::Real(k) => k.try_pubkey(),
                Self::Pda(k) => Ok(*k),
            }
        }
        fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
            match self {
                Self::Real(k) => k.try_sign_message(message),
                Self::Pda(_) => {
                    let mut b = [0u8; 64];
                    b[..32].copy_from_slice(Pubkey::new_unique().as_ref());
                    b[32..].copy_from_slice(Pubkey::new_unique().as_ref());
                    Ok(Signature::from(b))
                }
            }
        }
        fn is_interactive(&self) -> bool {
            false
        }
    }
}

pub const SQUADS_ID: Pubkey = elote_pool::state::SQUADS_PROGRAM_ID;

/// Writes a Squads-owned account; the Squads program itself is not loaded (nothing executes it).
pub fn put_squads_account(svm: &mut LiteSVM, key: &Pubkey, data: Vec<u8>) {
    let lamports = svm.minimum_balance_for_rent_exemption(data.len());
    svm.set_account(*key, solana_account::Account { lamports, data, owner: SQUADS_ID, executable: false, rent_epoch: 0 })
        .expect("set the multisig account");
}

/// `["multisig", "multisig", create_key]` under Squads.
pub fn multisig_pda(create_key: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"multisig", b"multisig", create_key.as_ref()], &SQUADS_ID)
}

/// Vault 0 of `multisig`.
pub fn squads_vault_pda(multisig: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"multisig", multisig.as_ref(), b"vault", &[0u8]], &SQUADS_ID)
}

/// Squads v4 `Permissions` masks.
pub const PERM_INITIATE: u8 = 1;
pub const PERM_VOTE: u8 = 2;
pub const PERM_EXECUTE: u8 = 4;
pub const PERM_ALL: u8 = 7;

/// The bytes of a Squads v4 `Multisig` account (v4 `af94153`): discriminator, create_key,
/// config_authority, threshold, time_lock, transaction_index, stale_transaction_index,
/// rent_collector, bump, members.
pub fn multisig_data(create_key: &Pubkey, config_authority: &Pubkey, threshold: u16, rent_collector: Option<Pubkey>, members: &[(Pubkey, u8)]) -> Vec<u8> {
    let disc = <sha2::Sha256 as sha2::Digest>::digest(b"account:Multisig");
    let mut d = disc[..8].to_vec();
    d.extend_from_slice(create_key.as_ref());
    d.extend_from_slice(config_authority.as_ref());
    d.extend_from_slice(&threshold.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&0u64.to_le_bytes());
    d.extend_from_slice(&0u64.to_le_bytes());
    match rent_collector {
        None => d.push(0),
        Some(k) => {
            d.push(1);
            d.extend_from_slice(k.as_ref());
        }
    }
    d.push(multisig_pda(create_key).1);
    d.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for (k, mask) in members {
        d.extend_from_slice(k.as_ref());
        d.push(*mask);
    }
    d
}

/// `elote_pool`'s `declare_id!`.
pub const PROGRAM_ID: Pubkey = pubkey!("BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM");
pub const TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Token-2022, loaded by `LiteSVM::new()` as `spl_token_2022-10.0.0.so`.
pub const TOKEN_2022_PROGRAM_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
/// Metaplex Core, loaded by `Env::new()` from `tests/fixtures/mpl_core_program-0.15.3.so`
/// (GitHub release `release/core@0.15.3`, 2026-10-07; see `tests/fixtures/README.md`). The
/// program's own `mpl_core::ID`.
pub const MPL_CORE_ID: Pubkey = pubkey!("CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");
pub const MPL_CORE_RELEASE: &str = "release/core@0.15.3";
pub const MPL_CORE_FIXTURE: &str = "programs/elote_pool/tests/fixtures/mpl_core_program-0.15.3.so";
/// sha256 of the fixture, checked at load so a swapped binary fails the suite instead of
/// silently changing it.
pub const MPL_CORE_SHA256: &str = "5b4dacdae4af476d3c9d8aa6712883f559ba0e13705f9b244d96d1fd0dd56ce5";
pub const ATA_PROGRAM_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Anchor's `#[error_code]` numbering: the first variant of the program's error enum is 6000, so
/// each code below follows the enum's order.
pub const ERR_BASE: u32 = 6000;

pub const E_PAUSED: u32 = ERR_BASE + PoolError::Paused as u32;
pub const E_UNAUTHORIZED: u32 = ERR_BASE + PoolError::Unauthorized as u32;
/// Anchor's own `ConstraintSeeds`: an account is not at the address its seeds (and the given bump) derive.
pub const E_CONSTRAINT_SEEDS: u32 = 2006;
pub const E_WRONG_AMOUNT: u32 = ERR_BASE + PoolError::WrongAmount as u32;
pub const E_INSUFFICIENT_LIQUIDITY: u32 = ERR_BASE + PoolError::InsufficientLiquidity as u32;
pub const E_LINE_STATUS: u32 = ERR_BASE + PoolError::LineStatus as u32;
pub const E_TERM_NOT_ENDED: u32 = ERR_BASE + PoolError::TermNotEnded as u32;
pub const E_OVERFLOW: u32 = ERR_BASE + PoolError::Overflow as u32;
pub const E_SPONSOR_REQUIRED: u32 = ERR_BASE + PoolError::SponsorRequired as u32;
pub const E_ZERO_SHARES: u32 = ERR_BASE + PoolError::ZeroShares as u32;
pub const E_FIRST_DEPOSIT_TOO_SMALL: u32 = ERR_BASE + PoolError::FirstDepositTooSmall as u32;
pub const E_WRONG_ACCOUNT: u32 = ERR_BASE + PoolError::WrongAccount as u32;
pub const E_SLIPPAGE_EXCEEDED: u32 = ERR_BASE + PoolError::SlippageExceeded as u32;
pub const E_TERM_ENDED: u32 = ERR_BASE + PoolError::TermEnded as u32;
pub const E_INVALID_SCHEDULE: u32 = ERR_BASE + PoolError::InvalidSchedule as u32;
pub const E_MAX_MISSED_TOO_LOW: u32 = ERR_BASE + PoolError::MaxMissedTooLow as u32;
pub const E_SCHEDULE_ENDS_AFTER_TERM: u32 = ERR_BASE + PoolError::ScheduleEndsAfterTerm as u32;
pub const E_NOT_SCHEDULED: u32 = ERR_BASE + PoolError::NotScheduled as u32;
pub const E_NOT_ENOUGH_MISSED: u32 = ERR_BASE + PoolError::NotEnoughMissed as u32;
pub const E_LATE_INTEREST_UNPAID: u32 = ERR_BASE + PoolError::LateInterestUnpaid as u32;
pub const E_WRITE_OFF_TOO_EARLY: u32 = ERR_BASE + PoolError::WriteOffTooEarly as u32;
pub const E_BORROWER_IS_COUNCIL: u32 = ERR_BASE + PoolError::BorrowerIsCouncil as u32;
pub const E_LINE_TOO_LARGE: u32 = ERR_BASE + PoolError::LineTooLarge as u32;
pub const E_INVARIANT_BROKEN: u32 = ERR_BASE + PoolError::InvariantBroken as u32;
pub const E_ASSET_MINT_NOT_ALLOWED: u32 = ERR_BASE + PoolError::AssetMintNotAllowed as u32;
pub const E_SHARE_MINT_NOT_ALLOWED: u32 = ERR_BASE + PoolError::ShareMintNotAllowed as u32;
pub const E_ATTESTOR_REQUIRED: u32 = ERR_BASE + PoolError::AttestorRequired as u32;
pub const E_AVAILABILITY_ENDED: u32 = ERR_BASE + PoolError::AvailabilityEnded as u32;
pub const E_TOO_MANY_DRAWS: u32 = ERR_BASE + PoolError::TooManyDraws as u32;
pub const E_COLLECTION_NOT_ALLOWED: u32 = ERR_BASE + PoolError::CollectionNotAllowed as u32;
pub const E_DOCUMENT_HASH_REQUIRED: u32 = ERR_BASE + PoolError::DocumentHashRequired as u32;
pub const E_SIGNATURE_METHOD_INVALID: u32 = ERR_BASE + PoolError::SignatureMethodInvalid as u32;
pub const E_PAGARE_NOT_COVERED: u32 = ERR_BASE + PoolError::PagareNotCovered as u32;
pub const E_PAGARE_ALREADY_PAID: u32 = ERR_BASE + PoolError::PagareAlreadyPaid as u32;
pub const E_NOT_HOLDER: u32 = ERR_BASE + PoolError::NotHolder as u32;
pub const E_ATTESTOR_IS_COUNCIL: u32 = ERR_BASE + PoolError::AttestorIsCouncil as u32;
pub const E_BORROWER_IS_ATTESTOR: u32 = ERR_BASE + PoolError::BorrowerIsAttestor as u32;
pub const E_AGREEMENT_HASH_REQUIRED: u32 = ERR_BASE + PoolError::AgreementHashRequired as u32;
pub const E_EXPEDIENTE_ROOT_REQUIRED: u32 = ERR_BASE + PoolError::ExpedienteRootRequired as u32;
pub const E_SPONSOR_ROLE_INVALID: u32 = ERR_BASE + PoolError::SponsorRoleInvalid as u32;
pub const E_COLLECTION_PERIOD_TOO_SHORT: u32 = ERR_BASE + PoolError::CollectionPeriodTooShort as u32;
pub const E_PAGARE_ACCOUNTS_MISMATCH: u32 = ERR_BASE + PoolError::PagareAccountsMismatch as u32;
pub const E_ENDOSO_KIND_INVALID: u32 = ERR_BASE + PoolError::EndosoKindInvalid as u32;
pub const E_ENDORSEE_INVALID: u32 = ERR_BASE + PoolError::EndorseeInvalid as u32;
pub const E_PROCURACION_STATE: u32 = ERR_BASE + PoolError::ProcuracionState as u32;
pub const E_ANNOTATION_KIND_INVALID: u32 = ERR_BASE + PoolError::AnnotationKindInvalid as u32;
pub const E_CANCEL_DELAY_TOO_SHORT: u32 = ERR_BASE + PoolError::CancelDelayTooShort as u32;
pub const E_ENTRY_NOT_FOUND: u32 = ERR_BASE + PoolError::EntryNotFound as u32;
/// Anchor's `AccountNotInitialized`: an account the instruction expects does not exist.
pub const E_ACCOUNT_NOT_INITIALIZED: u32 = 3012;

/// Token-2022's `TokenError::AccountFrozen`, surfaced as custom error 17 by a failed CPI.
pub const SPL_TOKEN_ACCOUNT_FROZEN: u32 = 17;

/// `state::BPS` as the `u16` that `init_pool` takes.
pub const BPS: u16 = elote_pool::state::BPS as u16;

/// SPL Token's `TokenError::InsufficientFunds`, surfaced as custom error 1 by a failed CPI.
pub const SPL_TOKEN_INSUFFICIENT_FUNDS: u32 = 1;

/// `state::DEAD_SHARES`: locked by the first deposit.
pub const DEAD_SHARES: u64 = 1_000;

/// One whole test-token unit: the asset mint has 6 decimals.
pub const UNIT: u64 = 1_000_000;

/// The installment interval the schedule tests use: 30 days.
pub const MONTH: i64 = 30 * 24 * 3600;
/// The collection period every pool of the harness is created with: the 30-day floor.
pub const COLLECTION_PERIOD: u32 = elote_pool::state::MIN_COLLECTION_PERIOD_SECONDS;
pub const CANCEL_DELAY: u32 = elote_pool::state::MIN_CANCEL_DELAY_SECONDS;
pub const ACCELERATION_NOTICE: u32 = elote_pool::state::MIN_ACCELERATION_NOTICE_SECONDS;
pub const CANCEL_GRACE: u32 = elote_pool::state::MIN_CANCEL_GRACE_SECONDS;

// -----------------------------------------------------------------------------------------------
// PDAs: the seed layouts of `state::seeds`.
// -----------------------------------------------------------------------------------------------

pub fn pool_pda(council: &Pubkey, asset_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"pool", council.as_ref(), asset_mint.as_ref()], &PROGRAM_ID)
}

pub fn vault_pda(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vault", pool.as_ref()], &PROGRAM_ID)
}

pub fn dead_shares_pda(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"dead", pool.as_ref()], &PROGRAM_ID)
}

pub fn line_pda(pool: &Pubkey, line_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"line", pool.as_ref(), &line_id.to_le_bytes()], &PROGRAM_ID)
}

/// The program's record of pagaré `draw_index` of `line`.
pub fn pagare_pda(line: &Pubkey, draw_index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"pagare", line.as_ref(), &draw_index.to_le_bytes()], &PROGRAM_ID)
}

/// The Metaplex Core asset that is pagaré `draw_index` of `line`.
pub fn pagare_asset_pda(line: &Pubkey, draw_index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"pagare_asset", line.as_ref(), &draw_index.to_le_bytes()], &PROGRAM_ID)
}

/// The addresses of one pool: the PDAs derived from the council and the asset mint, the
/// council-supplied share mint, and the token program of each mint.
#[derive(Clone, Copy, Debug)]
pub struct PoolKeys {
    /// Paid for `init_pool` and seeds the pool's address.
    pub creator: Pubkey,
    pub pool: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub vault: Pubkey,
    pub dead_shares: Pubkey,
    pub asset_program: Pubkey,
    pub share_program: Pubkey,
    /// The series' Metaplex Core collection the pool records: a real collection whose
    /// update authority is the pool PDA when built by `pool_keys_for`, a placeholder from `new`.
    pub collection: Pubkey,
}

impl PoolKeys {
    pub fn new(creator: &Pubkey, asset_mint: &Pubkey, share_mint: &Pubkey, asset_program: Pubkey, share_program: Pubkey) -> Self {
        let pool = pool_pda(creator, asset_mint).0;
        Self {
            creator: *creator,
            pool,
            asset_mint: *asset_mint,
            share_mint: *share_mint,
            vault: vault_pda(&pool).0,
            dead_shares: dead_shares_pda(&pool).0,
            asset_program,
            share_program,
            collection: Pubkey::new_unique(),
        }
    }

    pub fn asset_ata(&self, owner: &Pubkey) -> Pubkey {
        ata_for(owner, &self.asset_mint, &self.asset_program)
    }

    pub fn share_ata(&self, owner: &Pubkey) -> Pubkey {
        ata_for(owner, &self.share_mint, &self.share_program)
    }
}

/// The `#[event_cpi]` self-CPI authority every event-emitting instruction carries.
pub fn event_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID)
}

/// The classic SPL Token associated token account; for a mint under another token program use
/// `ata_for` or `PoolKeys::asset_ata` / `share_ata`.
pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, mint)
}

pub fn ata_for(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, token_program)
}

/// The fixed agreement hash the tests open lines with.
pub const AGREEMENT_HASH: [u8; 32] = [0xa7; 32];

/// `open_line` arguments other than the line id.
#[derive(Clone, Copy, Debug)]
pub struct LineParams {
    pub principal: u64,
    pub rate_bps: u16,
    pub term_end: i64,
    pub agreement_hash: [u8; 32],
    pub sponsor: Option<Pubkey>,
    pub sponsor_role: u8,
    pub schedule: Option<elote_pool::state::ScheduleParams>,
    pub expediente_root: [u8; 32],
    pub pagare_template_hash: [u8; 32],
}

impl LineParams {
    /// The terms `request_line` receives.
    pub fn terms(&self) -> elote_pool::terms::LineTerms {
        elote_pool::terms::LineTerms {
            principal: self.principal,
            rate_bps: self.rate_bps,
            term_end: self.term_end,
            agreement_hash: self.agreement_hash,
            sponsor: self.sponsor,
            sponsor_role: self.sponsor_role,
            schedule: self.schedule,
            expediente_root: self.expediente_root,
            pagare_template_hash: self.pagare_template_hash,
        }
    }

    /// The installment interval of a scheduled line (panics on a bullet line).
    pub fn interval(&self) -> i64 {
        self.schedule.expect("scheduled line").interval_seconds
    }

    /// The first due date of a scheduled line.
    pub fn first_due_at(&self) -> i64 {
        self.schedule.expect("scheduled line").first_due_at
    }
}

/// `SECONDS_PER_YEAR` as the `i64` the clock uses.
pub const YEAR: i64 = elote_pool::state::SECONDS_PER_YEAR as i64;

/// The fixed expediente root the tests open lines with.
pub const EXPEDIENTE_ROOT: [u8; 32] = [0xe7; 32];
/// The fixed pagaré template hash the tests request lines with.
pub const PAGARE_TEMPLATE_HASH: [u8; 32] = [0x7e; 32];
/// The harness FIX: 10,000.0000 MXN per USD, at which one centavo is one USDC base unit.
pub const PARITY_FIX: u64 = 100_000_000;
/// The clock every `Env` starts at: 2026-11-03T20:00Z.
pub const START_TIME: i64 = 1_793_736_000;

/// `yyyymmdd` of a day number (days since 1970-01-01).
pub fn yyyymmdd(days: i64) -> u32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y * 10_000 + m * 100 + d) as u32
}
/// The fingerprint of the PSC certificate behind the default test signature (method 1, PSC + wallet).
pub const CERT_FINGERPRINT: [u8; 32] = [0xcf; 32];
pub const SIGNATURE_METHOD: u8 = elote_pool::state::SIGNATURE_PSC_WALLET;
/// The hash of the FEA-signed receipt `cancel_pagare` is given in the tests.
pub const RECEIPT_HASH: [u8; 32] = [0x4e; 32];
/// The hash of the signed endoso the tests pass.
pub const ENDOSO_HASH: [u8; 32] = [0x5e; 32];

/// `["procurador", pagare, procurador]`: the program-signed authority of a pagaré's burn delegate.
pub fn procurador_pda(pagare: &Pubkey, procurador: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"procurador", pagare.as_ref(), procurador.as_ref()], &PROGRAM_ID)
}
#[allow(unused_imports)]
pub use elote_pool::state::{PAGARE_OUTSTANDING, PAGARE_PAID};

pub struct Env {
    pub svm: LiteSVM,
    /// Vault 0 of `multisig`: the council of every pool this env creates. It also pays for and
    /// owns the test mints, as any funded key could.
    pub council: Keypair,
    /// Pays for `init_pool` and seeds the pool; no other power.
    pub creator: Keypair,
    /// A 2-of-3 autonomous Squads multisig of `curators`, written into the SVM.
    pub multisig: Pubkey,
    pub council_vault_bump: u8,
    pub curators: Vec<Keypair>,
    /// Pools whose FIX the harness keeps current at `PARITY_FIX` (see `ensure_fix`).
    pub parity_pools: Vec<PoolKeys>,
    /// The attestor every pool of this env is created with : the agreements
    /// engine's key, which co-signs openings, draws and cancellations.
    pub attestor: Keypair,
}

fn event_metas() -> (Pubkey, Pubkey) {
    (event_authority_pda().0, PROGRAM_ID)
}

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/programs/elote_pool`.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

impl Env {
    /// A fresh SVM with the program loaded (SPL Token and ATA are LiteSVM defaults) and a funded
    /// council.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new().with_sigverify(false);
        assert!(svm.get_account(&TOKEN_PROGRAM_ID).is_some(), "SPL Token not loaded");
        assert!(svm.get_account(&ATA_PROGRAM_ID).is_some(), "ATA program not loaded");

        let so = workspace_root().join("target/deploy/elote_pool.so");
        assert!(so.exists(), "{} missing — run `NO_DNA=1 anchor build` first", so.display());
        svm.add_program_from_file(PROGRAM_ID, &so).expect("load elote_pool.so");

        // Metaplex Core, pinned by sha256 (the pagaré assets are minted into it).
        let core = std::fs::read(workspace_root().join(MPL_CORE_FIXTURE)).expect("read the Metaplex Core fixture");
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&core);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, MPL_CORE_SHA256, "{MPL_CORE_FIXTURE} is not the {MPL_CORE_RELEASE} binary");
        svm.add_program(MPL_CORE_ID, &core).expect("load mpl_core_program.so");

        let curators: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
        let create_key = Pubkey::new_unique();
        let multisig = multisig_pda(&create_key).0;
        let members: Vec<(Pubkey, u8)> = curators.iter().map(|c| (c.pubkey(), PERM_ALL)).collect();
        put_squads_account(&mut svm, &multisig, multisig_data(&create_key, &Pubkey::default(), 2, None, &members));
        let (vault, council_vault_bump) = squads_vault_pda(&multisig);
        let council = Keypair::pda(vault);
        svm.airdrop(&council.pubkey(), 500 * LAMPORTS_PER_SOL).expect("airdrop council");
        let creator = Keypair::new();
        svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).expect("airdrop creator");
        let attestor = Keypair::new();
        svm.airdrop(&attestor.pubkey(), 10 * LAMPORTS_PER_SOL).expect("airdrop attestor");

        // A real date, so FIX dates (2000-2199) are valid: 2026-11-03T20:00Z, 14:00 in Mexico City.
        let mut clock = svm.get_sysvar::<anchor_lang::prelude::Clock>();
        clock.unix_timestamp = START_TIME;
        svm.set_sysvar(&clock);
        Self { svm, council, creator, multisig, council_vault_bump, curators, attestor, parity_pools: Vec::new() }
    }

    pub fn funded(&mut self, sol: u64) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), sol * LAMPORTS_PER_SOL).expect("airdrop");
        kp
    }

    // -- transaction plumbing ------------------------------------------------------------------

    /// Sends `ix`, panicking with the program logs if it fails.
    pub fn send(&mut self, ix: Instruction, payer: &Keypair, extra: &[&Keypair]) -> TransactionMetadata {
        self.send_all(&[ix], payer, extra)
    }

    #[allow(clippy::result_large_err)] // litesvm's error carries the whole transaction metadata
    pub fn try_send(
        &mut self,
        ix: Instruction,
        payer: &Keypair,
        extra: &[&Keypair],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        self.try_send_all(&[ix], payer, extra)
    }

    #[allow(clippy::result_large_err)]
    pub fn try_send_all(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        extra: &[&Keypair],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        // Extra keys the message does not ask to sign are dropped, so a caller may pass a key a
        // later version stopped requiring; a signer the message needs and nobody gave still fails.
        let required: Vec<Pubkey> = ixs.iter().flat_map(|ix| ix.accounts.iter().filter(|m| m.is_signer).map(|m| m.pubkey)).collect();
        let mut signers: Vec<&Keypair> = vec![payer];
        signers.extend(extra.iter().copied().filter(|k| required.contains(&k.pubkey()) && k.pubkey() != payer.pubkey()));
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, self.svm.latest_blockhash());
        let res = self.svm.send_transaction(tx);
        // A fresh blockhash per transaction, so two identical instructions are two transactions.
        self.svm.expire_blockhash();
        res
    }

    pub fn send_all(&mut self, ixs: &[Instruction], payer: &Keypair, extra: &[&Keypair]) -> TransactionMetadata {
        match self.try_send_all(ixs, payer, extra) {
            Ok(meta) => meta,
            Err(e) => panic!("transaction failed: {:?}\n{}", e.err, e.meta.pretty_logs()),
        }
    }

    /// Asserts the instruction fails with Anchor custom error `code`, printing logs otherwise.
    pub fn expect_err(&mut self, ix: Instruction, payer: &Keypair, extra: &[&Keypair], code: u32) {
        match self.try_send(ix, payer, extra) {
            Ok(meta) => panic!("expected custom error {code}, but the tx succeeded\n{}", meta.pretty_logs()),
            Err(e) => {
                let got = custom_error_code(&e.err);
                assert_eq!(got, Some(code), "expected custom error {code}, got {:?}\n{}", e.err, e.meta.pretty_logs());
            }
        }
    }

    // -- account access ------------------------------------------------------------------------

    pub fn account(&self, key: &Pubkey) -> solana_account::Account {
        self.svm.get_account(key).unwrap_or_else(|| panic!("account {key} not found"))
    }

    pub fn anchor_account<T: AccountDeserialize>(&self, key: &Pubkey) -> T {
        let acc = self.account(key);
        T::try_deserialize(&mut acc.data.as_slice()).expect("anchor deserialize")
    }

    /// The SVM's current `Clock::unix_timestamp`, what the program reads as `now`.
    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<anchor_lang::prelude::Clock>().unix_timestamp
    }

    /// Moves the SVM clock to `unix_timestamp = ts`, keeping every other `Clock` field.
    pub fn warp(&mut self, ts: i64) {
        let mut clock = self.svm.get_sysvar::<anchor_lang::prelude::Clock>();
        clock.unix_timestamp = ts;
        self.svm.set_sysvar(&clock);
        for k in self.parity_pools.clone() {
            self.ensure_fix(&k);
        }
    }

    /// Today's date in Mexico City as `yyyymmdd`.
    pub fn today(&self) -> u32 {
        yyyymmdd(elote_pool::fx::mexico_city_day(self.now()))
    }

    /// Publishes `PARITY_FIX` for today through the council unless the pool already has today's
    /// date or a later one, and keeps it current after every `warp`. The arithmetic tests then read
    /// centavos and USDC base units as one unit; tests of the conversion set their own FIX.
    pub fn ensure_fix(&mut self, k: &PoolKeys) {
        if !self.parity_pools.iter().any(|p| p.pool == k.pool) {
            self.parity_pools.push(*k);
        }
        let today = self.today();
        let pool = self.pool(k);
        if pool.fix_date >= today && pool.fix_rate != 0 {
            return;
        }
        let council = self.council.insecure_clone();
        let ix = self.post_fix_ix(k, &council.pubkey(), PARITY_FIX, today);
        self.send(ix, &council, &[]);
    }

    /// Stops keeping `k` at `PARITY_FIX`.
    pub fn release_fix(&mut self, k: &PoolKeys) {
        self.parity_pools.retain(|p| p.pool != k.pool);
    }

    // -- SPL Token helpers ---------------------------------------------------------------------

    /// Creates a classic SPL mint with `authority` as mint authority and no freeze authority.
    pub fn create_mint(&mut self, authority: &Pubkey, decimals: u8) -> Pubkey {
        self.create_mint_in(&TOKEN_PROGRAM_ID, authority, None, decimals, &[], |_| vec![])
    }

    /// Creates a plain Token-2022 mint (no extensions) with `authority` as mint authority.
    pub fn create_mint_2022(&mut self, authority: &Pubkey, decimals: u8) -> Pubkey {
        self.create_mint_in(&TOKEN_2022_PROGRAM_ID, authority, None, decimals, &[], |_| vec![])
    }

    /// Creates a mint under `token_program` with the given mint and freeze authorities. For
    /// Token-2022, `extensions` sizes the account and `extension_ixs(mint)` returns the extension
    /// initializers, which must run before `initialize_mint2`.
    pub fn create_mint_in(
        &mut self,
        token_program: &Pubkey,
        authority: &Pubkey,
        freeze_authority: Option<&Pubkey>,
        decimals: u8,
        extensions: &[ExtensionType],
        extension_ixs: impl FnOnce(&Pubkey) -> Vec<Instruction>,
    ) -> Pubkey {
        let mint = Keypair::new();
        let len = ExtensionType::try_calculate_account_len::<spl_token_2022::state::Mint>(extensions).expect("mint size");
        let lamports = self.svm.minimum_balance_for_rent_exemption(len);
        let mut ixs = vec![anchor_lang::solana_program::system_instruction::create_account(
            &self.council.pubkey(),
            &mint.pubkey(),
            lamports,
            len as u64,
            token_program,
        )];
        ixs.extend(extension_ixs(&mint.pubkey()));
        ixs.push(
            spl_token_2022::instruction::initialize_mint2(token_program, &mint.pubkey(), authority, freeze_authority, decimals)
                .expect("initialize_mint2"),
        );
        let council = self.council.insecure_clone();
        self.send_all(&ixs, &council, &[&mint]);
        mint.pubkey()
    }

    /// The token program that owns `mint`.
    pub fn mint_program(&self, mint: &Pubkey) -> Pubkey {
        self.account(mint).owner
    }

    /// `owner`'s associated token account for `mint`, under the mint's own token program.
    pub fn ata_of(&self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        ata_for(owner, mint, &self.mint_program(mint))
    }

    /// Creates `owner`'s associated token account for `mint`, paid by the council.
    pub fn create_ata(&mut self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let program = self.mint_program(mint);
        let ix = ata_ix::create_associated_token_account(&self.council.pubkey(), owner, mint, &program);
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[]);
        ata_for(owner, mint, &program)
    }

    /// Mints `amount` of `mint` into `owner`'s ATA (created if missing); `authority` signs.
    pub fn mint_to(&mut self, mint: &Pubkey, authority: &Keypair, owner: &Pubkey, amount: u64) -> Pubkey {
        let program = self.mint_program(mint);
        let dest = ata_for(owner, mint, &program);
        if self.svm.get_account(&dest).is_none() {
            self.create_ata(owner, mint);
        }
        let ix = spl_token_2022::instruction::mint_to(&program, mint, &dest, &authority.pubkey(), &[], amount).expect("mint_to");
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[authority]);
        dest
    }

    /// A `transfer_checked` between two token accounts of `mint`, signed by `owner`.
    pub fn transfer(&mut self, mint: &Pubkey, from: &Pubkey, to: &Pubkey, owner: &Keypair, amount: u64) {
        let program = self.mint_program(mint);
        let decimals = self.mint_decimals(mint);
        let ix = spl_token_2022::instruction::transfer_checked(&program, from, mint, to, &owner.pubkey(), &[], amount, decimals)
            .expect("transfer_checked");
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[owner]);
    }

    /// The unpacked token account (classic or Token-2022, extensions skipped).
    pub fn token_account(&self, account: &Pubkey) -> spl_token_2022::state::Account {
        let acc = self.account(account);
        StateWithExtensions::<spl_token_2022::state::Account>::unpack(&acc.data).expect("token account").base
    }

    /// The balance of a token account, or 0 when it does not exist.
    pub fn token_balance(&self, account: &Pubkey) -> u64 {
        match self.svm.get_account(account) {
            Some(acc) => StateWithExtensions::<spl_token_2022::state::Account>::unpack(&acc.data).expect("token account").base.amount,
            None => 0,
        }
    }

    /// The unpacked mint (classic or Token-2022) and the list of its extensions.
    pub fn mint_state(&self, mint: &Pubkey) -> (spl_token_2022::state::Mint, Vec<ExtensionType>) {
        let acc = self.account(mint);
        let s = StateWithExtensions::<spl_token_2022::state::Mint>::unpack(&acc.data).expect("mint");
        (s.base, s.get_extension_types().expect("extension types"))
    }

    pub fn mint_supply(&self, mint: &Pubkey) -> u64 {
        self.mint_state(mint).0.supply
    }

    pub fn mint_decimals(&self, mint: &Pubkey) -> u8 {
        self.mint_state(mint).0.decimals
    }

    // -- pool instructions ---------------------------------------------------------------------

    /// The share mint the council supplies to `init_pool`: Token-2022, mint authority = the pool
    /// PDA, the asset's decimals, freeze authority `gate` (a plain keypair standing in for Token
    /// ACL's `MintConfig`; the program never looks at who it is), and, with `frozen_default`,
    /// the `DefaultAccountState = Frozen` extension that makes the shares permissioned.
    pub fn create_share_mint(&mut self, pool: &Pubkey, decimals: u8, gate: Option<&Pubkey>, frozen_default: bool) -> Pubkey {
        let p = TOKEN_2022_PROGRAM_ID;
        if frozen_default {
            self.create_mint_in(&p, pool, gate, decimals, &[ExtensionType::DefaultAccountState], |mint| {
                vec![spl_token_2022::extension::default_account_state::instruction::initialize_default_account_state(
                    &p,
                    mint,
                    &spl_token_2022::state::AccountState::Frozen,
                )
                .expect("default state")]
            })
        } else {
            self.create_mint_in(&p, pool, gate, decimals, &[], |_| vec![])
        }
    }

    /// Creates a Metaplex Core collection (the series' collection of pagaré assets) with
    /// `update_authority`, paid by the council, through the mpl-core crate's builder.
    pub fn create_collection(&mut self, update_authority: &Pubkey) -> Pubkey {
        self.create_collection_with(update_authority, vec![])
    }

    /// As `create_collection`, with collection-level plugins.
    pub fn create_collection_with(&mut self, update_authority: &Pubkey, plugins: Vec<mpl_core::types::PluginAuthorityPair>) -> Pubkey {
        let collection = Keypair::new();
        let ix = mpl_core::instructions::CreateCollectionV2Builder::new()
            .collection(collection.pubkey())
            .update_authority(Some(*update_authority))
            .payer(self.council.pubkey())
            .name("ELOTE-CR-TEST".to_string())
            .uri(String::new())
            .plugins(plugins)
            .instruction();
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[&collection]);
        collection.pubkey()
    }

    /// Creates a plain Metaplex Core asset (no collection, no plugins) owned by `owner`.
    pub fn create_core_asset(&mut self, owner: &Pubkey) -> Pubkey {
        let asset = Keypair::new();
        let ix = mpl_core::instructions::CreateV2Builder::new()
            .asset(asset.pubkey())
            .payer(self.council.pubkey())
            .owner(Some(*owner))
            .name("asset".to_string())
            .uri(String::new())
            .instruction();
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[&asset]);
        asset.pubkey()
    }

    /// The base record of a Core collection, read back through the crate's decoder.
    pub fn collection_state(&self, collection: &Pubkey) -> mpl_core::accounts::BaseCollectionV1 {
        mpl_core::accounts::BaseCollectionV1::from_bytes(&self.account(collection).data).expect("collection")
    }

    /// The keys of a pool for `asset_mint` (whatever its token program), with a fresh plain
    /// Token-2022 share mint and a fresh Core collection (update authority = the pool PDA)
    /// created for it. Does not run `init_pool`.
    pub fn pool_keys_for(&mut self, asset_mint: &Pubkey) -> PoolKeys {
        let creator = self.council.pubkey();
        self.pool_keys_by(&creator, asset_mint)
    }

    /// As `pool_keys_for`, for a pool `creator` pays for and seeds.
    pub fn pool_keys_by(&mut self, creator: &Pubkey, asset_mint: &Pubkey) -> PoolKeys {
        let council = *creator;
        let pool = pool_pda(&council, asset_mint).0;
        let decimals = self.mint_decimals(asset_mint);
        let share_mint = self.create_share_mint(&pool, decimals, None, false);
        let mut k = PoolKeys::new(&council, asset_mint, &share_mint, self.mint_program(asset_mint), TOKEN_2022_PROGRAM_ID);
        k.collection = self.create_collection(&pool);
        k
    }

    /// `init_pool` for the pool described by `k`.
    pub fn init_pool_ix_for(&self, k: &PoolKeys, grace_period_seconds: u32, max_line_bps: u16) -> Instruction {
        self.init_pool_ix_with_attestor(k, grace_period_seconds, max_line_bps, self.attestor.pubkey())
    }

    /// `init_pool` naming `attestor` instead of the env's.
    pub fn init_pool_ix_with_attestor(&self, k: &PoolKeys, grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey) -> Instruction {
        self.init_pool_ix_full(k, grace_period_seconds, max_line_bps, attestor, COLLECTION_PERIOD, CANCEL_DELAY)
    }

    /// The `init_pool` parameters the tests use, with the env's council.
    pub fn init_params(&self, grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey, collection_period_seconds: u32, cancel_delay_seconds: u32) -> elote_pool::instructions::pool::InitPoolParams {
        elote_pool::instructions::pool::InitPoolParams {
            grace_period_seconds,
            max_line_bps,
            attestor,
            collection_period_seconds,
            cancel_delay_seconds,
            guardian: Pubkey::default(),
            max_outstanding_principal: u64::MAX,
            acceleration_notice_seconds: ACCELERATION_NOTICE,
            cancel_grace_seconds: CANCEL_GRACE,
        }
    }

    /// `init_pool` for `k` naming `multisig`, with `params`.
    pub fn init_pool_ix_params(&self, k: &PoolKeys, multisig: &Pubkey, params: elote_pool::instructions::pool::InitPoolParams) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::InitPool {
                creator: k.creator,
                asset_mint: k.asset_mint,
                pool: k.pool,
                share_mint: k.share_mint,
                vault: k.vault,
                dead_shares: k.dead_shares,
                collection: k.collection,
                multisig: *multisig,
                token_program: k.asset_program,
                share_token_program: k.share_program,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::InitPool { params }.data(),
        }
    }

    /// `init_pool` with every argument explicit.
    pub fn init_pool_ix_full(
        &self,
        k: &PoolKeys,
        grace_period_seconds: u32,
        max_line_bps: u16,
        attestor: Pubkey,
        collection_period_seconds: u32,
        cancel_delay_seconds: u32,
    ) -> Instruction {
        let params = self.init_params(grace_period_seconds, max_line_bps, attestor, collection_period_seconds, cancel_delay_seconds);
        self.init_pool_ix_params(k, &self.multisig.clone(), params)
    }

    /// `propose_council`: the council names `new_council` of `kind`.
    pub fn propose_council_ix(&self, k: &PoolKeys, council: &Pubkey, new_council: &Pubkey, kind: u8) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::ProposeCouncil { council: *council, pool: k.pool, event_authority, program }.to_account_metas(None),
            data: elote_pool::instruction::ProposeCouncil { new_council: *new_council, kind }.data(),
        }
    }

    /// `accept_council`: the proposed key signs, naming its multisig.
    pub fn accept_council_ix(&self, k: &PoolKeys, new_council: &Pubkey, multisig: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::AcceptCouncil { new_council: *new_council, multisig: *multisig, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::AcceptCouncil {}.data(),
        }
    }

    pub fn set_cancel_delay_ix(&self, k: &PoolKeys, council: &Pubkey, seconds: u32) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetCancelDelay { council: *council, pool: k.pool, event_authority, program }.to_account_metas(None),
            data: elote_pool::instruction::SetCancelDelay { cancel_delay_seconds: seconds }.data(),
        }
    }

    pub fn set_collection_period_ix(&self, k: &PoolKeys, council: &Pubkey, seconds: u32) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetCollectionPeriod { council: *council, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::SetCollectionPeriod { collection_period_seconds: seconds }.data(),
        }
    }

    /// `set_attestor`: the council and the new attestor both sign (condition C14).
    pub fn set_attestor_ix(&self, k: &PoolKeys, council: &Pubkey, new_attestor: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetAttestor {
                council: *council,
                new_attestor: *new_attestor,
                pool: k.pool,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::SetAttestor {}.data(),
        }
    }

    /// Keys and `init_pool` instruction for `asset_mint` with the given grace and cap; a fresh
    /// plain Token-2022 share mint is created for it.
    pub fn init_pool_full(&mut self, asset_mint: &Pubkey, grace_period_seconds: u32, max_line_bps: u16) -> (PoolKeys, Instruction) {
        let k = self.pool_keys_for(asset_mint);
        let ix = self.init_pool_ix_for(&k, grace_period_seconds, max_line_bps);
        (k, ix)
    }

    /// As `init_pool_full` with no grace and no effective concentration cap (`max_line_bps` =
    /// 10,000, the whole NAV), so the tests that open large lines keep working; the cap has its
    /// own tests.
    pub fn init_pool_plain(&mut self, asset_mint: &Pubkey) -> (PoolKeys, Instruction) {
        self.init_pool_full(asset_mint, 0, BPS)
    }

    /// Creates a 6-decimal classic test asset mint (authority = council) and a pool for it with a
    /// plain Token-2022 share mint.
    pub fn setup_pool(&mut self) -> (PoolKeys, TransactionMetadata) {
        self.setup_pool_with_grace(0)
    }

    /// As `setup_pool`, with a pool grace period of `grace_period_seconds`.
    pub fn setup_pool_with_grace(&mut self, grace_period_seconds: u32) -> (PoolKeys, TransactionMetadata) {
        let council = self.council.insecure_clone();
        let asset_mint = self.create_mint(&council.pubkey(), 6);
        let (k, ix) = self.init_pool_full(&asset_mint, grace_period_seconds, BPS);
        let meta = self.send(ix, &council, &[]);
        (k, meta)
    }

    /// A pool on a plain Token-2022 asset (6 decimals, no extensions, no grace, no effective cap).
    pub fn setup_pool_2022(&mut self) -> (PoolKeys, TransactionMetadata) {
        let council = self.council.insecure_clone();
        let asset_mint = self.create_mint_2022(&council.pubkey(), 6);
        let (k, ix) = self.init_pool_plain(&asset_mint);
        let meta = self.send(ix, &council, &[]);
        (k, meta)
    }

    /// A pool on a classic asset whose shares are permissioned: frozen by default, with `gate`
    /// as the share mint's freeze authority. Nothing is thawed here.
    pub fn setup_permissioned_pool(&mut self, gate: &Pubkey) -> (PoolKeys, TransactionMetadata) {
        let council = self.council.insecure_clone();
        let asset_mint = self.create_mint(&council.pubkey(), 6);
        let pool = pool_pda(&council.pubkey(), &asset_mint).0;
        let share_mint = self.create_share_mint(&pool, 6, Some(gate), true);
        let mut k = PoolKeys::new(&council.pubkey(), &asset_mint, &share_mint, TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID);
        k.collection = self.create_collection(&pool);
        let ix = self.init_pool_ix_for(&k, 0, BPS);
        let meta = self.send(ix, &council, &[]);
        (k, meta)
    }

    /// Thaws a token account of `mint`, signed by its freeze authority `gate` (the stand-in for
    /// Token ACL's thaw).
    pub fn thaw_ix(&self, mint: &Pubkey, account: &Pubkey, gate: &Pubkey) -> Instruction {
        spl_token_2022::instruction::thaw_account(&self.mint_program(mint), account, mint, gate, &[]).expect("thaw")
    }

    pub fn thaw(&mut self, mint: &Pubkey, account: &Pubkey, gate: &Keypair) {
        let ix = self.thaw_ix(mint, account, &gate.pubkey());
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[gate]);
    }

    /// Hands `authority_type` of `mint` from `current` to `new` (`None` revokes it for good).
    pub fn set_mint_authority(
        &mut self,
        mint: &Pubkey,
        authority_type: spl_token_2022::instruction::AuthorityType,
        current: &Keypair,
        new: Option<&Pubkey>,
    ) {
        let ix = spl_token_2022::instruction::set_authority(
            &self.mint_program(mint),
            mint,
            new,
            authority_type,
            &current.pubkey(),
            &[],
        )
        .expect("set_authority");
        let council = self.council.insecure_clone();
        self.send(ix, &council, &[current]);
    }

    /// A funded wallet holding `assets` of the pool asset, with its share account already created.
    pub fn investor(&mut self, k: &PoolKeys, assets: u64) -> Keypair {
        let kp = self.funded(10);
        let council = self.council.insecure_clone();
        if assets > 0 {
            self.mint_to(&k.asset_mint, &council, &kp.pubkey(), assets);
        } else {
            self.create_ata(&kp.pubkey(), &k.asset_mint);
        }
        self.create_ata(&kp.pubkey(), &k.share_mint);
        kp
    }

    #[allow(clippy::too_many_arguments)]
    pub fn deposit_ix_full(
        &self,
        k: &PoolKeys,
        depositor: &Pubkey,
        asset_account: Pubkey,
        share_account: Pubkey,
        assets: u64,
        min_shares_out: u64,
    ) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Deposit {
                depositor: *depositor,
                pool: k.pool,
                asset_mint: k.asset_mint,
                share_mint: k.share_mint,
                vault: k.vault,
                dead_shares: k.dead_shares,
                depositor_asset_account: asset_account,
                depositor_share_account: share_account,
                token_program: k.asset_program,
                share_token_program: k.share_program,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::Deposit { assets, min_shares_out }.data(),
        }
    }

    pub fn deposit_ix_with(&self, k: &PoolKeys, depositor: &Pubkey, asset_account: Pubkey, share_account: Pubkey, assets: u64) -> Instruction {
        self.deposit_ix_full(k, depositor, asset_account, share_account, assets, 0)
    }

    pub fn deposit_ix_min(&self, k: &PoolKeys, depositor: &Pubkey, assets: u64, min_shares_out: u64) -> Instruction {
        self.deposit_ix_full(k, depositor, k.asset_ata(depositor), k.share_ata(depositor), assets, min_shares_out)
    }

    pub fn deposit_ix(&self, k: &PoolKeys, depositor: &Pubkey, assets: u64) -> Instruction {
        self.deposit_ix_with(k, depositor, k.asset_ata(depositor), k.share_ata(depositor), assets)
    }

    pub fn deposit(&mut self, k: &PoolKeys, depositor: &Keypair, assets: u64) -> elote_pool::events::Deposited {
        let ix = self.deposit_ix(k, &depositor.pubkey(), assets);
        let meta = self.send(ix, depositor, &[]);
        find_event(&meta).expect("Deposited event")
    }

    pub fn redeem_ix(&self, k: &PoolKeys, holder: &Pubkey, shares: u64) -> Instruction {
        self.redeem_ix_min(k, holder, shares, 0)
    }

    pub fn redeem_ix_min(&self, k: &PoolKeys, holder: &Pubkey, shares: u64, min_assets_out: u64) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Redeem {
                holder: *holder,
                pool: k.pool,
                asset_mint: k.asset_mint,
                share_mint: k.share_mint,
                vault: k.vault,
                holder_share_account: k.share_ata(holder),
                holder_asset_account: k.asset_ata(holder),
                token_program: k.asset_program,
                share_token_program: k.share_program,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::Redeem { shares, min_assets_out }.data(),
        }
    }

    pub fn redeem(&mut self, k: &PoolKeys, holder: &Keypair, shares: u64) -> elote_pool::events::Redeemed {
        let ix = self.redeem_ix(k, &holder.pubkey(), shares);
        let meta = self.send(ix, holder, &[]);
        find_event(&meta).expect("Redeemed event")
    }

    pub fn set_paused_ix(&self, k: &PoolKeys, council: &Pubkey, paused: bool) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetPaused { council: *council, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::SetPaused { paused }.data(),
        }
    }

    pub fn set_paused(&mut self, k: &PoolKeys, paused: bool) -> TransactionMetadata {
        let council = self.council.insecure_clone();
        let ix = self.set_paused_ix(k, &council.pubkey(), paused);
        self.send(ix, &council, &[])
    }

    // -- pool reads ----------------------------------------------------------------------------

    pub fn pool(&self, k: &PoolKeys) -> elote_pool::state::Pool {
        self.anchor_account(&k.pool)
    }

    /// Net asset value as the program computes it: vault balance + outstanding principal.
    pub fn nav(&self, k: &PoolKeys) -> u64 {
        self.token_balance(&k.vault) + self.pool(k).outstanding_principal
    }

    pub fn supply(&self, k: &PoolKeys) -> u64 {
        self.mint_supply(&k.share_mint)
    }

    // -- credit line instructions --------------------------------------------------------------

    /// `request_line` for line `line_id`, funded by `payer`.
    pub fn request_line_ix(&self, k: &PoolKeys, payer: &Pubkey, line_id: u64, borrower: &Pubkey, p: &LineParams, sponsor_account: Option<Pubkey>) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::RequestLine {
                borrower: *borrower,
                payer: *payer,
                sponsor: sponsor_account,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::RequestLine { line_id, terms: p.terms() }.data(),
        }
    }

    /// The terms hash of line `line_id` requested by `borrower` on `p`, as the curators compute it.
    pub fn terms_hash(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey, p: &LineParams) -> [u8; 32] {
        elote_pool::terms::terms_hash(&k.pool, line_id, borrower, elote_pool::state::CURRENCY_MXN, &p.terms())
    }

    pub fn set_fx_feeder_ix(&self, k: &PoolKeys, council: &Pubkey, fx_feeder: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetFxFeeder { council: *council, multisig: self.pool(k).multisig, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::SetFxFeeder { fx_feeder: *fx_feeder }.data(),
        }
    }

    pub fn post_fix_ix(&self, k: &PoolKeys, poster: &Pubkey, fix_rate: u64, fix_date: u32) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::PostFix { poster: *poster, multisig: self.pool(k).multisig, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::PostFix { fix_rate, fix_date }.data(),
        }
    }

    /// `approve_line` naming `expediente_root` and `terms_hash`, signed by `council`.
    pub fn approve_line_ix(&self, k: &PoolKeys, council: &Pubkey, line_id: u64, expediente_root: [u8; 32], terms_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::ApproveLine {
                council: *council,
                multisig: self.pool(k).multisig,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                vault: k.vault,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::ApproveLine { expediente_root, terms_hash }.data(),
        }
    }

    pub fn reject_line_ix(&self, k: &PoolKeys, council: &Pubkey, line_id: u64, payer: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::RejectLine {
                council: *council,
                multisig: self.pool(k).multisig,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                payer: *payer,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::RejectLine {}.data(),
        }
    }

    pub fn withdraw_request_ix(&self, k: &PoolKeys, borrower: &Pubkey, line_id: u64, payer: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::WithdrawRequest {
                borrower: *borrower,
                line: line_pda(&k.pool, line_id).0,
                payer: *payer,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::WithdrawRequest {}.data(),
        }
    }

    /// Requests the next line of the pool for `borrower` (the council's vault pays, as Elote's
    /// backend would), signed by the borrower and the named sponsor. Returns the id and the
    /// request's transaction.
    pub fn request_line_with(&mut self, k: &PoolKeys, borrower: &Keypair, p: &LineParams, sponsor: Option<&Keypair>) -> (u64, TransactionMetadata) {
        self.ensure_fix(k);
        let payer = self.council.insecure_clone();
        let line_id = self.pool(k).line_count;
        let ix = self.request_line_ix(k, &payer.pubkey(), line_id, &borrower.pubkey(), p, sponsor.map(|s| s.pubkey()));
        let mut extra: Vec<&Keypair> = vec![borrower];
        if let Some(s) = sponsor {
            extra.push(s);
        }
        let meta = self.send(ix, &payer, &extra);
        (line_id, meta)
    }

    /// The council approves line `line_id` as requested on `p`.
    pub fn approve_line(&mut self, k: &PoolKeys, line_id: u64, borrower: &Pubkey, p: &LineParams) -> TransactionMetadata {
        let council = self.council.insecure_clone();
        let hash = self.terms_hash(k, line_id, borrower, p);
        let ix = self.approve_line_ix(k, &council.pubkey(), line_id, p.expediente_root, hash);
        self.send(ix, &council, &[])
    }

    /// The pagaré hash `draw` derives for tranche `draw_index` of `amount_mxn` drawn at `drawn_at`.
    pub fn expected_pagare_hash(&self, k: &PoolKeys, line_id: u64, draw_index: u16, amount_mxn: u64, drawn_at: i64) -> [u8; 32] {
        let key = line_pda(&k.pool, line_id).0;
        elote_pool::instructions::lines::pagare_hash(&self.line(k, line_id), &key, draw_index, amount_mxn, drawn_at)
    }

    /// Default terms of a bullet line: `principal` at an annual `rate_bps` for exactly one year
    /// (so a line drawn in full right away owes `principal * rate_bps / BPS` of interest), a
    /// fixed agreement hash and expediente root, no sponsor.
    pub fn line_params(&self, principal: u64, rate_bps: u16) -> LineParams {
        LineParams {
            principal,
            rate_bps,
            term_end: self.now() + YEAR,
            agreement_hash: AGREEMENT_HASH,
            sponsor: None,
            sponsor_role: elote_pool::state::SPONSOR_PROMOTER,
            schedule: None,
            expediente_root: EXPEDIENTE_ROOT,
            pagare_template_hash: PAGARE_TEMPLATE_HASH,
        }
    }

    /// Terms of a scheduled line: `installments` equal installments spanning exactly one year
    /// (interval `YEAR / installments`, exact for 3, 4, 6, 12, 18 and 120), the first due date one
    /// interval from now, a term that ends one year from now, and the demo policy of three missed
    /// installments before collection. Drawn in full right away, the line owes
    /// `principal * rate_bps / BPS` of interest, the same figure as the old flat rate.
    pub fn scheduled_params(&self, principal: u64, rate_bps: u16, installments: u16, late_bps: u16) -> LineParams {
        let interval = YEAR / i64::from(installments);
        LineParams {
            principal,
            rate_bps,
            term_end: self.now() + i64::from(installments) * interval,
            agreement_hash: AGREEMENT_HASH,
            sponsor: None,
            sponsor_role: elote_pool::state::SPONSOR_PROMOTER,
            schedule: Some(elote_pool::state::ScheduleParams {
                first_due_at: self.now() + interval,
                interval_seconds: interval,
                installments,
                late_bps,
                max_missed: 3,
            }),
            expediente_root: EXPEDIENTE_ROOT,
            pagare_template_hash: PAGARE_TEMPLATE_HASH,
        }
    }

    /// Requests the next line of the pool and has the council approve it. Returns the id and the
    /// request's transaction.
    pub fn open_line_with(
        &mut self,
        k: &PoolKeys,
        borrower: &Keypair,
        p: &LineParams,
        sponsor: Option<&Keypair>,
    ) -> (u64, TransactionMetadata) {
        let (line_id, meta) = self.request_line_with(k, borrower, p, sponsor);
        self.approve_line(k, line_id, &borrower.pubkey(), p);
        (line_id, meta)
    }

    pub fn open_line(&mut self, k: &PoolKeys, borrower: &Keypair, principal: u64, rate_bps: u16) -> u64 {
        let p = self.line_params(principal, rate_bps);
        self.open_line_with(k, borrower, &p, None).0
    }

    /// A funded borrower wallet with an (empty) asset account.
    pub fn borrower(&mut self, k: &PoolKeys) -> Keypair {
        let kp = self.funded(10);
        self.create_ata(&kp.pubkey(), &k.asset_mint);
        kp
    }

    /// A tranche of `amount` against `document_hash`, with the borrower paying the pagaré's
    /// rent; signed by the borrower and the attestor (the attestor is added by `draw` and
    /// `draw_amount`; `draw_ix` only names it).
    pub fn draw_ix_amount(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey, amount: u64) -> Instruction {
        self.draw_ix_paid_by(k, line_id, borrower, borrower, amount)
    }

    /// As `draw_ix_amount`, with `payer` (the relayer) paying the rent of the pagaré asset and
    /// its sidecar. The asset and the sidecar are PDAs of the line and the next draw index, read
    /// from the line when the instruction is built: build each draw after the previous one was
    /// sent, or two instructions carry the same index and the second fails on the seeds.
    pub fn draw_ix_paid_by(
        &self,
        k: &PoolKeys,
        line_id: u64,
        borrower: &Pubkey,
        payer: &Pubkey,
        amount: u64,
    ) -> Instruction {
        let line = line_pda(&k.pool, line_id).0;
        let draw_index = self.line(k, line_id).draws;
        let (asset_bump, pagare_bump) = (pagare_asset_pda(&line, draw_index).1, pagare_pda(&line, draw_index).1);
        self.draw_ix_full(k, line_id, borrower, payer, amount, asset_bump, pagare_bump, CERT_FINGERPRINT, SIGNATURE_METHOD)
    }

    /// As `draw_ix_amount`, with the signature facts of the pagaré given explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ix_signed(
        &self,
        k: &PoolKeys,
        line_id: u64,
        borrower: &Pubkey,
        amount: u64,
        cert_fingerprint: [u8; 32],
        signature_method: u8,
    ) -> Instruction {
        let line = line_pda(&k.pool, line_id).0;
        let draw_index = self.line(k, line_id).draws;
        let (asset_bump, pagare_bump) = (pagare_asset_pda(&line, draw_index).1, pagare_pda(&line, draw_index).1);
        self.draw_ix_full(k, line_id, borrower, borrower, amount, asset_bump, pagare_bump, cert_fingerprint, signature_method)
    }

    /// As `draw_ix_paid_by`, with the two PDA bumps given explicitly (the program verifies them).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ix_full(
        &self,
        k: &PoolKeys,
        line_id: u64,
        borrower: &Pubkey,
        payer: &Pubkey,
        amount: u64,
        asset_bump: u8,
        pagare_bump: u8,
        cert_fingerprint: [u8; 32],
        signature_method: u8,
    ) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        let draw_index = self.line(k, line_id).draws;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Draw {
                borrower: *borrower,
                pool: k.pool,
                line,
                asset_mint: k.asset_mint,
                vault: k.vault,
                borrower_asset_account: k.asset_ata(borrower),
                token_program: k.asset_program,
                payer: *payer,
                asset: pagare_asset_pda(&line, draw_index).0,
                pagare: pagare_pda(&line, draw_index).0,
                collection: k.collection,
                mpl_core_program: MPL_CORE_ID,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: {
                let pool = self.pool(k);
                elote_pool::instruction::Draw { amount_mxn: amount, fx_rate: pool.fix_rate, fx_date: pool.fix_date, asset_bump, pagare_bump, cert_fingerprint, signature_method }.data()
            },
        }
    }

    /// `cancel_pagare(draw_index, receipt_hash)`: the holder (the council while the asset is in the
    /// pool) and the attestor sign; `payer` receives the asset's rent back from the Core program.
    /// The asset is read from the sidecar, as a client would.
    #[allow(clippy::too_many_arguments)]
    pub fn cancel_pagare_ix(
        &self,
        k: &PoolKeys,
        line_id: u64,
        draw_index: u16,
        holder: &Pubkey,
        payer: &Pubkey,
        receipt_hash: [u8; 32],
    ) -> Instruction {
        let asset = self.pagare(k, line_id, draw_index).asset;
        self.cancel_pagare_ix_full(k, line_id, draw_index, holder, payer, receipt_hash, asset, k.collection, None, Some(self.attestor.pubkey()))
    }
    /// `cancel_pagare` by the holder alone, the backstop: no attestor account.
    pub fn cancel_pagare_alone_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, holder: &Pubkey, payer: &Pubkey, receipt_hash: [u8; 32]) -> Instruction {
        let asset = self.pagare(k, line_id, draw_index).asset;
        self.cancel_pagare_ix_full(k, line_id, draw_index, holder, payer, receipt_hash, asset, k.collection, None, None)
    }
    /// `request_cancellation(draw_index, receipt_hash)` by `holder`.
    pub fn request_cancellation_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, holder: &Pubkey, receipt_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::RequestCancellation {
                holder: *holder,
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::RequestCancellation { draw_index, receipt_hash }.data(),
        }
    }
    /// `cancel_pagare` signed by the procurador of record: `holder` is the procurador and the
    /// program address `["procurador", pagare, procurador]` is passed as the burn's authority.
    pub fn cancel_pagare_by_procurador_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, procurador: &Pubkey, payer: &Pubkey, receipt_hash: [u8; 32]) -> Instruction {
        let line = line_pda(&k.pool, line_id).0;
        let asset = self.pagare(k, line_id, draw_index).asset;
        let pda = procurador_pda(&pagare_pda(&line, draw_index).0, procurador).0;
        self.cancel_pagare_ix_full(k, line_id, draw_index, procurador, payer, receipt_hash, asset, k.collection, Some(pda), Some(self.attestor.pubkey()))
    }

    /// As `cancel_pagare_ix`, with the asset and the collection given explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn cancel_pagare_ix_full(
        &self,
        k: &PoolKeys,
        line_id: u64,
        draw_index: u16,
        holder: &Pubkey,
        payer: &Pubkey,
        receipt_hash: [u8; 32],
        asset: Pubkey,
        collection: Pubkey,
        procurador_authority: Option<Pubkey>,
        attestor: Option<Pubkey>,
    ) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::CancelPagare {
                holder: *holder,
                attestor,
                payer: *payer,
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                asset,
                collection,
                procurador_authority,
                mpl_core_program: MPL_CORE_ID,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::CancelPagare { draw_index, receipt_hash }.data(),
        }
    }

    /// `endorse_pagare(draw_index, kind, to, endoso_hash)` by the council as holder and payer.
    #[allow(clippy::too_many_arguments)]
    pub fn endorse_pagare_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, holder: &Pubkey, kind: u8, to: Pubkey, endoso_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::EndorsePagare {
                holder: *holder,
                attestor: self.attestor.pubkey(),
                payer: *holder,
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                asset: self.pagare(k, line_id, draw_index).asset,
                collection: k.collection,
                mpl_core_program: MPL_CORE_ID,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::EndorsePagare { draw_index, kind, to, endoso_hash }.data(),
        }
    }

    /// `revoke_procuracion(draw_index, entry_hash)` by `holder` as payer too.
    pub fn revoke_procuracion_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, holder: &Pubkey, entry_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::RevokeProcuracion {
                holder: *holder,
                payer: *holder,
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                asset: self.pagare(k, line_id, draw_index).asset,
                collection: k.collection,
                mpl_core_program: MPL_CORE_ID,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::RevokeProcuracion { draw_index, entry_hash }.data(),
        }
    }

    /// `court_annotation(draw_index, kind, document_hash)` by `holder`, the pool's attestor as the second signer.
    pub fn court_annotation_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, holder: &Pubkey, kind: u8, document_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::CourtAnnotation {
                holder: *holder,
                attestor: self.attestor.pubkey(),
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::CourtAnnotation { draw_index, kind, document_hash }.data(),
        }
    }
    /// A court annotation of pagaré `draw_index`, signed by the council and the attestor.
    pub fn court_annotation(&mut self, k: &PoolKeys, line_id: u64, draw_index: u16, kind: u8, document_hash: [u8; 32]) -> TransactionMetadata {
        let council = self.council.insecure_clone();
        let attestor = self.attestor.insecure_clone();
        let ix = self.court_annotation_ix(k, line_id, draw_index, &council.pubkey(), kind, document_hash);
        self.send(ix, &council, &[&attestor])
    }

    /// An endoso en procuración of pagaré `draw_index` to `procurador`, signed by the council and the attestor.
    pub fn procuracion(&mut self, k: &PoolKeys, line_id: u64, draw_index: u16, procurador: &Pubkey, endoso_hash: [u8; 32]) -> TransactionMetadata {
        let council = self.council.insecure_clone();
        let attestor = self.attestor.insecure_clone();
        let ix = self.endorse_pagare_ix(k, line_id, draw_index, &council.pubkey(), elote_pool::state::KIND_PROCURACION, *procurador, endoso_hash);
        self.send(ix, &council, &[&attestor])
    }

    /// Cancels pagaré `draw_index` of a line: the council as holder and payer, the attestor co-signing.
    pub fn cancel_pagare(&mut self, k: &PoolKeys, line_id: u64, draw_index: u16) -> TransactionMetadata {
        let council = self.council.insecure_clone();
        let attestor = self.attestor.insecure_clone();
        let ix = self.cancel_pagare_ix(k, line_id, draw_index, &council.pubkey(), &council.pubkey(), RECEIPT_HASH);
        self.send(ix, &council, &[&attestor])
    }

    /// Asserts a cancellation of pagaré `draw_index` by the council and the attestor fails with `code`.
    pub fn expect_cancel_err(&mut self, k: &PoolKeys, line_id: u64, draw_index: u16, code: u32) {
        let council = self.council.insecure_clone();
        let attestor = self.attestor.insecure_clone();
        let ix = self.cancel_pagare_ix(k, line_id, draw_index, &council.pubkey(), &council.pubkey(), RECEIPT_HASH);
        self.expect_err(ix, &council, &[&attestor], code);
    }

    /// The sidecar of pagaré `draw_index` of a line.
    pub fn pagare(&self, k: &PoolKeys, line_id: u64, draw_index: u16) -> elote_pool::state::Pagare {
        self.anchor_account(&pagare_pda(&line_pda(&k.pool, line_id).0, draw_index).0)
    }

    /// A Core asset with every plugin, read back through the crate's decoder.
    pub fn asset_state(&self, asset: &Pubkey) -> Box<mpl_core::Asset> {
        mpl_core::Asset::from_bytes(&self.account(asset).data).expect("asset")
    }

    /// A draw of everything still available on the line.
    pub fn draw_ix(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey) -> Instruction {
        let line = self.line(k, line_id);
        self.draw_ix_amount(k, line_id, borrower, line.principal - line.drawn)
    }

    /// Draws everything still available; signed by the borrower and the attestor.
    pub fn draw(&mut self, k: &PoolKeys, line_id: u64, borrower: &Keypair) -> elote_pool::events::LineDrawn {
        let ix = self.draw_ix(k, line_id, &borrower.pubkey());
        let attestor = self.attestor.insecure_clone();
        let meta = self.send(ix, borrower, &[&attestor]);
        find_event(&meta).expect("LineDrawn event")
    }

    /// Draws one tranche of `amount`; signed by the borrower and the attestor.
    pub fn draw_amount(&mut self, k: &PoolKeys, line_id: u64, borrower: &Keypair, amount: u64) -> elote_pool::events::LineDrawn {
        let ix = self.draw_ix_amount(k, line_id, &borrower.pubkey(), amount);
        let attestor = self.attestor.insecure_clone();
        let meta = self.send(ix, borrower, &[&attestor]);
        find_event(&meta).expect("LineDrawn event")
    }

    /// Sends `ix` signed by `payer` and the attestor; for the draw instructions built by hand.
    pub fn send_with_attestor(&mut self, ix: Instruction, payer: &Keypair) -> TransactionMetadata {
        let attestor = self.attestor.insecure_clone();
        self.send(ix, payer, &[&attestor])
    }

    /// Asserts a draw instruction fails with `code` once the attestor has signed too.
    pub fn expect_err_with_attestor(&mut self, ix: Instruction, payer: &Keypair, code: u32) {
        let attestor = self.attestor.insecure_clone();
        self.expect_err(ix, payer, &[&attestor], code);
    }

    /// The sidecars a `repay` of `amount` must pass, `[pagares_paid ..= last_touched]`, computed
    /// as the program does: the payment's principal part, then every pagaré from the cursor through
    /// the first one it leaves uncovered (or the last draw); none once the cursor has passed the
    /// last draw or the line never drew.
    pub fn payment_sidecars(&self, k: &PoolKeys, line_id: u64, amount: u64) -> Vec<Pubkey> {
        let line = self.line(k, line_id);
        let cursor = line.pagares_paid;
        if cursor >= line.draws {
            return vec![];
        }
        let late = if line.schedule().is_some() { self.arrears(k, line_id).late_due } else { 0 };
        let interest_left = line.interest_due - line.interest_repaid;
        let remaining = (line.drawn - line.principal_repaid) + interest_left;
        let rest = amount.saturating_sub(late).min(remaining);
        let principal_after = line.principal_repaid + (rest - rest.min(interest_left));
        let line_key = line_pda(&k.pool, line_id).0;
        let mut out = Vec::new();
        for idx in cursor..line.draws {
            out.push(pagare_pda(&line_key, idx).0);
            if self.pagare(k, line_id, idx).drawn_through > principal_after {
                break;
            }
        }
        out
    }
    /// `repay(amount, RECEIPT_HASH)` with the sidecars the payment touches as remaining accounts.
    pub fn repay_ix(&self, k: &PoolKeys, line_id: u64, payer: &Pubkey, amount: u64) -> Instruction {
        let sidecars = self.payment_sidecars(k, line_id, amount);
        self.repay_ix_full(k, line_id, payer, amount, RECEIPT_HASH, &sidecars)
    }
    /// `repay(amount, receipt_hash)` with the remaining accounts given explicitly.
    pub fn repay_ix_full(&self, k: &PoolKeys, line_id: u64, payer: &Pubkey, amount: u64, receipt_hash: [u8; 32], sidecars: &[Pubkey]) -> Instruction {
        let (event_authority, program) = event_metas();
        let mut accounts = elote_pool::accounts::Repay {
            payer: *payer,
            pool: k.pool,
            line: line_pda(&k.pool, line_id).0,
            asset_mint: k.asset_mint,
            share_mint: k.share_mint,
            vault: k.vault,
            payer_asset_account: k.asset_ata(payer),
            token_program: k.asset_program,
            event_authority,
            program,
        }
        .to_account_metas(None);
        accounts.extend(sidecars.iter().map(|s| anchor_lang::solana_program::instruction::AccountMeta::new(*s, false)));
        Instruction { program_id: PROGRAM_ID, accounts, data: elote_pool::instruction::Repay { amount, receipt_hash }.data() }
    }
    /// `annotate_payment(draw_index, entry_count, receipt_hash)` by `annotator`.
    pub fn annotate_payment_ix(&self, k: &PoolKeys, line_id: u64, draw_index: u16, annotator: &Pubkey, entry_count: u32, receipt_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        let line = line_pda(&k.pool, line_id).0;
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::AnnotatePayment {
                annotator: *annotator,
                pool: k.pool,
                line,
                pagare: pagare_pda(&line, draw_index).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::AnnotatePayment { draw_index, entry_count, receipt_hash }.data(),
        }
    }

    pub fn repay(&mut self, k: &PoolKeys, line_id: u64, payer: &Keypair, amount: u64) -> elote_pool::events::LineRepaid {
        let ix = self.repay_ix(k, line_id, &payer.pubkey(), amount);
        let meta = self.send(ix, payer, &[]);
        find_event(&meta).expect("LineRepaid event")
    }

    pub fn write_off_ix(&self, k: &PoolKeys, line_id: u64, council: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::WriteOff {
                council: *council,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                share_mint: k.share_mint,
                vault: k.vault,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::WriteOff {}.data(),
        }
    }

    pub fn suspend_draws_ix(&self, k: &PoolKeys, line_id: u64, council: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SuspendDraws {
                council: *council,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::SuspendDraws {}.data(),
        }
    }

    pub fn cancel_line_ix(&self, k: &PoolKeys, line_id: u64, council: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::CancelLine {
                council: *council,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::CancelLine {}.data(),
        }
    }

    pub fn mark_in_collection_ix(&self, k: &PoolKeys, line_id: u64) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::MarkInCollection {
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::MarkInCollection {}.data(),
        }
    }

    /// The arrears of a scheduled line at the SVM's current time, as the program computes them.
    pub fn arrears(&self, k: &PoolKeys, line_id: u64) -> elote_pool::math::Arrears {
        let line = self.line(k, line_id);
        let s = line.schedule().expect("scheduled line");
        let terms = elote_pool::math::ScheduleTerms {
            total: line.drawn + line.interest_due,
            anchor: line.anchor().expect("anchor"),
            interval: s.interval_seconds,
            installments: s.installments,
            late_bps: s.late_bps,
            grace: self.pool(k).grace_period_seconds,
            late_paid_until: line.late_paid_until,
            principal: line.drawn,
            late_collected: line.late_collected,
            accelerated_at: line.accelerated_at,
        };
        elote_pool::math::arrears(&terms, line.principal_repaid + line.interest_repaid, self.now()).expect("arrears")
    }

    pub fn line(&self, k: &PoolKeys, line_id: u64) -> elote_pool::state::CreditLine {
        self.anchor_account(&line_pda(&k.pool, line_id).0)
    }

    /// Opens, draws and fully repays a line of `principal` at `rate_bps`: the pool earns the interest.
    pub fn lend_and_repay(&mut self, k: &PoolKeys, principal: u64, rate_bps: u16) -> u64 {
        let borrower = self.borrower(k);
        let line_id = self.open_line(k, &borrower, principal, rate_bps);
        self.draw(k, line_id, &borrower);
        let interest = self.line(k, line_id).interest_due;
        let council = self.council.insecure_clone();
        self.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
        self.repay(k, line_id, &borrower, principal + interest);
        interest
    }

    /// A direct SPL transfer into the vault from a fresh wallet: value enters without new shares.
    pub fn donate(&mut self, k: &PoolKeys, amount: u64) {
        let council = self.council.insecure_clone();
        let donor = self.funded(1);
        let from = self.mint_to(&k.asset_mint, &council, &donor.pubkey(), amount);
        self.transfer(&k.asset_mint, &from, &k.vault, &donor, amount);
    }
}

/// The Anchor custom error number inside a failed transaction, if it carries one.
pub fn custom_error_code(err: &TransactionError) -> Option<u32> {
    match err {
        TransactionError::InstructionError(_, solana_instruction::error::InstructionError::Custom(n)) => Some(*n),
        _ => None,
    }
}

/// Finds an `emit_cpi!` event in a transaction's inner instructions.
///
/// `#[event_cpi]` emits events as a self-CPI whose instruction data is
/// `EVENT_IX_TAG_LE (8) || event discriminator (8) || borsh(event)`; it never reaches the
/// `Program data:` logs that plain `emit!` uses, so the inner instruction list is the only place
/// to look.
pub fn find_event<E>(meta: &TransactionMetadata) -> Option<E>
where
    E: Discriminator + anchor_lang::AnchorDeserialize,
{
    for inner in meta.inner_instructions.iter().flatten() {
        let data = &inner.instruction.data;
        if data.len() < 16 {
            continue;
        }
        if &data[..8] != anchor_lang::event::EVENT_IX_TAG_LE {
            continue;
        }
        if &data[8..16] != E::DISCRIMINATOR {
            continue;
        }
        return Some(E::deserialize(&mut &data[16..]).expect("event borsh"));
    }
    None
}
