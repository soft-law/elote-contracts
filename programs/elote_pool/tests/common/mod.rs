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
use solana_keypair::Keypair;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

/// `elote_pool`'s `declare_id!`.
pub const PROGRAM_ID: Pubkey = pubkey!("BGdJQxxJoHiLH2VS6HAGHe5kQuKKq9dwrPeX7DEEwYyM");
pub const TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Token-2022, loaded by `LiteSVM::new()` as `spl_token_2022-10.0.0.so`.
pub const TOKEN_2022_PROGRAM_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ATA_PROGRAM_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Anchor's `#[error_code]` numbering: the first variant of the program's error enum is 6000.
pub const ERR_BASE: u32 = 6000;

pub const E_PAUSED: u32 = ERR_BASE; // PoolError::Paused
pub const E_UNAUTHORIZED: u32 = ERR_BASE + 1; // PoolError::Unauthorized
pub const E_WRONG_AMOUNT: u32 = ERR_BASE + 2; // PoolError::WrongAmount
pub const E_INSUFFICIENT_LIQUIDITY: u32 = ERR_BASE + 3; // PoolError::InsufficientLiquidity
pub const E_LINE_STATUS: u32 = ERR_BASE + 4; // PoolError::LineStatus
pub const E_TERM_NOT_ENDED: u32 = ERR_BASE + 5; // PoolError::TermNotEnded
pub const E_OVERFLOW: u32 = ERR_BASE + 6; // PoolError::Overflow
pub const E_SPONSOR_REQUIRED: u32 = ERR_BASE + 7; // PoolError::SponsorRequired
pub const E_ZERO_SHARES: u32 = ERR_BASE + 8; // PoolError::ZeroShares
pub const E_FIRST_DEPOSIT_TOO_SMALL: u32 = ERR_BASE + 9; // PoolError::FirstDepositTooSmall
pub const E_WRONG_ACCOUNT: u32 = ERR_BASE + 10; // PoolError::WrongAccount
pub const E_SLIPPAGE_EXCEEDED: u32 = ERR_BASE + 11; // PoolError::SlippageExceeded
pub const E_TERM_ENDED: u32 = ERR_BASE + 12; // PoolError::TermEnded
pub const E_INVALID_SCHEDULE: u32 = ERR_BASE + 13; // PoolError::InvalidSchedule
pub const E_SCHEDULE_ENDS_AFTER_TERM: u32 = ERR_BASE + 14; // PoolError::ScheduleEndsAfterTerm
pub const E_NOT_SCHEDULED: u32 = ERR_BASE + 15; // PoolError::NotScheduled
pub const E_NOT_ENOUGH_MISSED: u32 = ERR_BASE + 16; // PoolError::NotEnoughMissed
pub const E_LATE_INTEREST_UNPAID: u32 = ERR_BASE + 17; // PoolError::LateInterestUnpaid
pub const E_WRITE_OFF_TOO_EARLY: u32 = ERR_BASE + 18; // PoolError::WriteOffTooEarly
pub const E_BORROWER_IS_ADMIN: u32 = ERR_BASE + 19; // PoolError::BorrowerIsAdmin
pub const E_LINE_TOO_LARGE: u32 = ERR_BASE + 20; // PoolError::LineTooLarge
pub const E_INVARIANT_BROKEN: u32 = ERR_BASE + 21; // PoolError::InvariantBroken
pub const E_ASSET_MINT_NOT_ALLOWED: u32 = ERR_BASE + 22; // PoolError::AssetMintNotAllowed
pub const E_SHARE_MINT_NOT_ALLOWED: u32 = ERR_BASE + 23; // PoolError::ShareMintNotAllowed
pub const E_ATTESTOR_REQUIRED: u32 = ERR_BASE + 24; // PoolError::AttestorRequired
pub const E_AVAILABILITY_ENDED: u32 = ERR_BASE + 25; // PoolError::AvailabilityEnded
pub const E_TOO_MANY_DRAWS: u32 = ERR_BASE + 26; // PoolError::TooManyDraws

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

// -----------------------------------------------------------------------------------------------
// PDAs: the seed layouts of `state::seeds`.
// -----------------------------------------------------------------------------------------------

pub fn pool_pda(admin: &Pubkey, asset_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"pool", admin.as_ref(), asset_mint.as_ref()], &PROGRAM_ID)
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

/// The addresses of one pool: the PDAs derived from the admin and the asset mint, the
/// admin-supplied share mint, and the token program of each mint.
#[derive(Clone, Copy, Debug)]
pub struct PoolKeys {
    pub admin: Pubkey,
    pub pool: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub vault: Pubkey,
    pub dead_shares: Pubkey,
    pub asset_program: Pubkey,
    pub share_program: Pubkey,
    /// The series' Metaplex Core collection the pool records (pool v2). A fresh placeholder
    /// address here; step 6 of the batch validates it as a Core collection.
    pub collection: Pubkey,
}

impl PoolKeys {
    pub fn new(admin: &Pubkey, asset_mint: &Pubkey, share_mint: &Pubkey, asset_program: Pubkey, share_program: Pubkey) -> Self {
        let pool = pool_pda(admin, asset_mint).0;
        Self {
            admin: *admin,
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
}

impl LineParams {
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
/// The fixed pagaré hash the tests draw with.
pub const DOCUMENT_HASH: [u8; 32] = [0xd0; 32];

pub struct Env {
    pub svm: LiteSVM,
    pub admin: Keypair,
    /// The attestor every pool of this env is created with (pool v2, decision 24): the
    /// agreements engine's key, which co-signs openings, draws and endosos from step 5 on.
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
    /// admin.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        assert!(svm.get_account(&TOKEN_PROGRAM_ID).is_some(), "SPL Token not loaded");
        assert!(svm.get_account(&ATA_PROGRAM_ID).is_some(), "ATA program not loaded");

        let so = workspace_root().join("target/deploy/elote_pool.so");
        assert!(so.exists(), "{} missing — run `NO_DNA=1 anchor build` first", so.display());
        svm.add_program_from_file(PROGRAM_ID, &so).expect("load elote_pool.so");

        let admin = Keypair::new();
        svm.airdrop(&admin.pubkey(), 500 * LAMPORTS_PER_SOL).expect("airdrop admin");
        let attestor = Keypair::new();
        svm.airdrop(&attestor.pubkey(), 10 * LAMPORTS_PER_SOL).expect("airdrop attestor");

        Self { svm, admin, attestor }
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
        let mut signers: Vec<&Keypair> = vec![payer];
        signers.extend_from_slice(extra);
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
            &self.admin.pubkey(),
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
        let admin = self.admin.insecure_clone();
        self.send_all(&ixs, &admin, &[&mint]);
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

    /// Creates `owner`'s associated token account for `mint`, paid by the admin.
    pub fn create_ata(&mut self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let program = self.mint_program(mint);
        let ix = ata_ix::create_associated_token_account(&self.admin.pubkey(), owner, mint, &program);
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[]);
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
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[authority]);
        dest
    }

    /// A `transfer_checked` between two token accounts of `mint`, signed by `owner`.
    pub fn transfer(&mut self, mint: &Pubkey, from: &Pubkey, to: &Pubkey, owner: &Keypair, amount: u64) {
        let program = self.mint_program(mint);
        let decimals = self.mint_decimals(mint);
        let ix = spl_token_2022::instruction::transfer_checked(&program, from, mint, to, &owner.pubkey(), &[], amount, decimals)
            .expect("transfer_checked");
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[owner]);
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

    /// The share mint the admin supplies to `init_pool`: Token-2022, mint authority = the pool
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

    /// The keys of a pool for `asset_mint` (whatever its token program), with a fresh plain
    /// Token-2022 share mint created for it. Does not run `init_pool`.
    pub fn pool_keys_for(&mut self, asset_mint: &Pubkey) -> PoolKeys {
        let admin = self.admin.pubkey();
        let pool = pool_pda(&admin, asset_mint).0;
        let decimals = self.mint_decimals(asset_mint);
        let share_mint = self.create_share_mint(&pool, decimals, None, false);
        PoolKeys::new(&admin, asset_mint, &share_mint, self.mint_program(asset_mint), TOKEN_2022_PROGRAM_ID)
    }

    /// `init_pool` for the pool described by `k`.
    pub fn init_pool_ix_for(&self, k: &PoolKeys, grace_period_seconds: u32, max_line_bps: u16) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::InitPool {
                admin: k.admin,
                asset_mint: k.asset_mint,
                pool: k.pool,
                share_mint: k.share_mint,
                vault: k.vault,
                dead_shares: k.dead_shares,
                collection: k.collection,
                token_program: k.asset_program,
                share_token_program: k.share_program,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::InitPool { grace_period_seconds, max_line_bps, attestor: self.attestor.pubkey() }
                .data(),
        }
    }

    pub fn set_attestor_ix(&self, k: &PoolKeys, admin: &Pubkey, attestor: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetAttestor { admin: *admin, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::SetAttestor { attestor: *attestor }.data(),
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

    /// Creates a 6-decimal classic test asset mint (authority = admin) and a pool for it with a
    /// plain Token-2022 share mint.
    pub fn setup_pool(&mut self) -> (PoolKeys, TransactionMetadata) {
        self.setup_pool_with_grace(0)
    }

    /// As `setup_pool`, with a pool grace period of `grace_period_seconds`.
    pub fn setup_pool_with_grace(&mut self, grace_period_seconds: u32) -> (PoolKeys, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let asset_mint = self.create_mint(&admin.pubkey(), 6);
        let (k, ix) = self.init_pool_full(&asset_mint, grace_period_seconds, BPS);
        let meta = self.send(ix, &admin, &[]);
        (k, meta)
    }

    /// A pool on a plain Token-2022 asset (6 decimals, no extensions, no grace, no effective cap).
    pub fn setup_pool_2022(&mut self) -> (PoolKeys, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let asset_mint = self.create_mint_2022(&admin.pubkey(), 6);
        let (k, ix) = self.init_pool_plain(&asset_mint);
        let meta = self.send(ix, &admin, &[]);
        (k, meta)
    }

    /// A pool on a classic asset whose shares are permissioned: frozen by default, with `gate`
    /// as the share mint's freeze authority. Nothing is thawed here.
    pub fn setup_permissioned_pool(&mut self, gate: &Pubkey) -> (PoolKeys, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let asset_mint = self.create_mint(&admin.pubkey(), 6);
        let pool = pool_pda(&admin.pubkey(), &asset_mint).0;
        let share_mint = self.create_share_mint(&pool, 6, Some(gate), true);
        let k = PoolKeys::new(&admin.pubkey(), &asset_mint, &share_mint, TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID);
        let ix = self.init_pool_ix_for(&k, 0, BPS);
        let meta = self.send(ix, &admin, &[]);
        (k, meta)
    }

    /// Thaws a token account of `mint`, signed by its freeze authority `gate` (the stand-in for
    /// Token ACL's thaw).
    pub fn thaw_ix(&self, mint: &Pubkey, account: &Pubkey, gate: &Pubkey) -> Instruction {
        spl_token_2022::instruction::thaw_account(&self.mint_program(mint), account, mint, gate, &[]).expect("thaw")
    }

    pub fn thaw(&mut self, mint: &Pubkey, account: &Pubkey, gate: &Keypair) {
        let ix = self.thaw_ix(mint, account, &gate.pubkey());
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[gate]);
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
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[current]);
    }

    /// A funded wallet holding `assets` of the pool asset, with its share account already created.
    pub fn investor(&mut self, k: &PoolKeys, assets: u64) -> Keypair {
        let kp = self.funded(10);
        let admin = self.admin.insecure_clone();
        if assets > 0 {
            self.mint_to(&k.asset_mint, &admin, &kp.pubkey(), assets);
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

    pub fn set_paused_ix(&self, k: &PoolKeys, admin: &Pubkey, paused: bool) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::SetPaused { admin: *admin, pool: k.pool, event_authority, program }
                .to_account_metas(None),
            data: elote_pool::instruction::SetPaused { paused }.data(),
        }
    }

    pub fn set_paused(&mut self, k: &PoolKeys, paused: bool) -> TransactionMetadata {
        let admin = self.admin.insecure_clone();
        let ix = self.set_paused_ix(k, &admin.pubkey(), paused);
        self.send(ix, &admin, &[])
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

    #[allow(clippy::too_many_arguments)]
    pub fn open_line_ix(
        &self,
        k: &PoolKeys,
        admin: &Pubkey,
        line_id: u64,
        borrower: &Pubkey,
        p: &LineParams,
        sponsor_account: Option<Pubkey>,
    ) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::OpenLine {
                admin: *admin,
                borrower: *borrower,
                sponsor: sponsor_account,
                attestor: self.attestor.pubkey(),
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                vault: k.vault,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::OpenLine {
                line_id,
                principal: p.principal,
                rate_bps: p.rate_bps,
                term_end: p.term_end,
                agreement_hash: p.agreement_hash,
                sponsor: p.sponsor,
                sponsor_role: p.sponsor_role,
                schedule: p.schedule,
                expediente_root: p.expediente_root,
            }
            .data(),
        }
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
        }
    }

    /// Opens the next line of the pool, signed by the admin, the borrower, the attestor and (if
    /// named) the sponsor.
    pub fn open_line_with(
        &mut self,
        k: &PoolKeys,
        borrower: &Keypair,
        p: &LineParams,
        sponsor: Option<&Keypair>,
    ) -> (u64, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let attestor = self.attestor.insecure_clone();
        let line_id = self.pool(k).line_count;
        let ix = self.open_line_ix(k, &admin.pubkey(), line_id, &borrower.pubkey(), p, sponsor.map(|s| s.pubkey()));
        let mut extra: Vec<&Keypair> = vec![borrower, &attestor];
        if let Some(s) = sponsor {
            extra.push(s);
        }
        let meta = self.send(ix, &admin, &extra);
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

    /// A tranche of `amount` against the fixed test pagaré hash, signed by the borrower and the
    /// attestor (the attestor is added by `draw`/`draw_amount`; `draw_ix` only names it).
    pub fn draw_ix_amount(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey, amount: u64, document_hash: [u8; 32]) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Draw {
                borrower: *borrower,
                attestor: self.attestor.pubkey(),
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                asset_mint: k.asset_mint,
                vault: k.vault,
                borrower_asset_account: k.asset_ata(borrower),
                token_program: k.asset_program,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::Draw { amount, document_hash }.data(),
        }
    }

    /// A draw of everything still available on the line.
    pub fn draw_ix(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey) -> Instruction {
        let line = self.line(k, line_id);
        self.draw_ix_amount(k, line_id, borrower, line.principal - line.drawn, DOCUMENT_HASH)
    }

    /// Draws everything still available; signed by the borrower and the attestor.
    pub fn draw(&mut self, k: &PoolKeys, line_id: u64, borrower: &Keypair) -> elote_pool::events::LineDrawn {
        let ix = self.draw_ix(k, line_id, &borrower.pubkey());
        let attestor = self.attestor.insecure_clone();
        let meta = self.send(ix, borrower, &[&attestor]);
        find_event(&meta).expect("LineDrawn event")
    }

    /// Draws one tranche of `amount`; signed by the borrower and the attestor.
    pub fn draw_amount(&mut self, k: &PoolKeys, line_id: u64, borrower: &Keypair, amount: u64) -> elote_pool::events::LineDrawState {
        let ix = self.draw_ix_amount(k, line_id, &borrower.pubkey(), amount, DOCUMENT_HASH);
        let attestor = self.attestor.insecure_clone();
        let meta = self.send(ix, borrower, &[&attestor]);
        find_event(&meta).expect("LineDrawState event")
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

    pub fn repay_ix(&self, k: &PoolKeys, line_id: u64, payer: &Pubkey, amount: u64) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Repay {
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
            .to_account_metas(None),
            data: elote_pool::instruction::Repay { amount }.data(),
        }
    }

    pub fn repay(&mut self, k: &PoolKeys, line_id: u64, payer: &Keypair, amount: u64) -> elote_pool::events::LineRepaid {
        let ix = self.repay_ix(k, line_id, &payer.pubkey(), amount);
        let meta = self.send(ix, payer, &[]);
        find_event(&meta).expect("LineRepaid event")
    }

    pub fn write_off_ix(&self, k: &PoolKeys, line_id: u64, admin: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::WriteOff {
                admin: *admin,
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
            grace: self.pool(k).grace_period_seconds(),
            late_paid_until: s.late_paid_until,
            principal: line.drawn,
            late_collected: s.late_collected,
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
        let admin = self.admin.insecure_clone();
        self.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), interest);
        self.repay(k, line_id, &borrower, principal + interest);
        interest
    }

    /// A direct SPL transfer into the vault from a fresh wallet: value enters without new shares.
    pub fn donate(&mut self, k: &PoolKeys, amount: u64) {
        let admin = self.admin.insecure_clone();
        let donor = self.funded(1);
        let from = self.mint_to(&k.asset_mint, &admin, &donor.pubkey(), amount);
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
