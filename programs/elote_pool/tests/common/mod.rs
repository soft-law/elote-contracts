//! Shared LiteSVM harness for the `elote_pool` integration tests.
//!
//! Everything here builds state the way a client would: real instructions, signed by real
//! keypairs, executed by the compiled `target/deploy/elote_pool.so`. SPL Token and the Associated
//! Token Account program come from `LiteSVM::new()`, which loads them by default in litesvm 0.9.0
//! (`spl_token-3.5.0.so`, `spl_associated_token_account-1.1.1.so`).

#![allow(dead_code)]

use anchor_lang::{AccountDeserialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::{
    get_associated_token_address, spl_associated_token_account::instruction as ata_ix,
};
use anchor_spl::token::spl_token;
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

pub fn share_mint_pda(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"shares", pool.as_ref()], &PROGRAM_ID)
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

/// The addresses of one pool, all derived from the admin and the asset mint.
#[derive(Clone, Copy, Debug)]
pub struct PoolKeys {
    pub pool: Pubkey,
    pub asset_mint: Pubkey,
    pub share_mint: Pubkey,
    pub vault: Pubkey,
    pub dead_shares: Pubkey,
}

impl PoolKeys {
    pub fn new(admin: &Pubkey, asset_mint: &Pubkey) -> Self {
        let pool = pool_pda(admin, asset_mint).0;
        Self {
            pool,
            asset_mint: *asset_mint,
            share_mint: share_mint_pda(&pool).0,
            vault: vault_pda(&pool).0,
            dead_shares: dead_shares_pda(&pool).0,
        }
    }
}

/// The `#[event_cpi]` self-CPI authority every event-emitting instruction carries.
pub fn event_authority_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID)
}

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, mint)
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
    pub schedule: Option<elote_pool::state::ScheduleParams>,
}

pub struct Env {
    pub svm: LiteSVM,
    pub admin: Keypair,
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

        Self { svm, admin }
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

    /// Creates a classic SPL mint with `authority` as mint authority.
    pub fn create_mint(&mut self, authority: &Pubkey, decimals: u8) -> Pubkey {
        let mint = Keypair::new();
        let len = anchor_spl::token::Mint::LEN;
        let lamports = self.svm.minimum_balance_for_rent_exemption(len);
        let create = anchor_lang::solana_program::system_instruction::create_account(
            &self.admin.pubkey(),
            &mint.pubkey(),
            lamports,
            len as u64,
            &TOKEN_PROGRAM_ID,
        );
        let init = spl_token::instruction::initialize_mint2(&TOKEN_PROGRAM_ID, &mint.pubkey(), authority, None, decimals)
            .expect("initialize_mint2");
        let admin = self.admin.insecure_clone();
        self.send_all(&[create, init], &admin, &[&mint]);
        mint.pubkey()
    }

    /// Creates `owner`'s associated token account for `mint`, paid by the admin.
    pub fn create_ata(&mut self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let ix = ata_ix::create_associated_token_account(&self.admin.pubkey(), owner, mint, &TOKEN_PROGRAM_ID);
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[]);
        ata(owner, mint)
    }

    /// Mints `amount` of `mint` into `owner`'s ATA (created if missing); `authority` signs.
    pub fn mint_to(&mut self, mint: &Pubkey, authority: &Keypair, owner: &Pubkey, amount: u64) -> Pubkey {
        let dest = ata(owner, mint);
        if self.svm.get_account(&dest).is_none() {
            self.create_ata(owner, mint);
        }
        let ix = spl_token::instruction::mint_to(&TOKEN_PROGRAM_ID, mint, &dest, &authority.pubkey(), &[], amount)
            .expect("mint_to");
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[authority]);
        dest
    }

    /// A plain SPL transfer between two token accounts, signed by `owner`.
    pub fn transfer(&mut self, from: &Pubkey, to: &Pubkey, owner: &Keypair, amount: u64) {
        let ix = spl_token::instruction::transfer(&TOKEN_PROGRAM_ID, from, to, &owner.pubkey(), &[], amount)
            .expect("transfer");
        let admin = self.admin.insecure_clone();
        self.send(ix, &admin, &[owner]);
    }

    /// The balance of a token account, or 0 when it does not exist.
    pub fn token_balance(&self, account: &Pubkey) -> u64 {
        match self.svm.get_account(account) {
            Some(acc) => anchor_spl::token::TokenAccount::try_deserialize(&mut acc.data.as_slice())
                .expect("token account")
                .amount,
            None => 0,
        }
    }

    pub fn mint_supply(&self, mint: &Pubkey) -> u64 {
        self.anchor_account::<anchor_spl::token::Mint>(mint).supply
    }

    pub fn mint_decimals(&self, mint: &Pubkey) -> u8 {
        self.anchor_account::<anchor_spl::token::Mint>(mint).decimals
    }

    // -- pool instructions ---------------------------------------------------------------------

    pub fn init_pool_ix(&self, admin: &Pubkey, asset_mint: &Pubkey) -> Instruction {
        self.init_pool_ix_with_grace(admin, asset_mint, 0)
    }

    pub fn init_pool_ix_with_grace(&self, admin: &Pubkey, asset_mint: &Pubkey, grace_period_seconds: u32) -> Instruction {
        let k = PoolKeys::new(admin, asset_mint);
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::InitPool {
                admin: *admin,
                asset_mint: k.asset_mint,
                pool: k.pool,
                share_mint: k.share_mint,
                vault: k.vault,
                dead_shares: k.dead_shares,
                token_program: TOKEN_PROGRAM_ID,
                system_program: SYSTEM_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::InitPool { grace_period_seconds }.data(),
        }
    }

    /// Creates a 6-decimal test asset mint (authority = admin) and a pool for it.
    pub fn setup_pool(&mut self) -> (PoolKeys, TransactionMetadata) {
        self.setup_pool_with_grace(0)
    }

    /// As `setup_pool`, with a pool grace period of `grace_period_seconds`.
    pub fn setup_pool_with_grace(&mut self, grace_period_seconds: u32) -> (PoolKeys, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let asset_mint = self.create_mint(&admin.pubkey(), 6);
        let ix = self.init_pool_ix_with_grace(&admin.pubkey(), &asset_mint, grace_period_seconds);
        let meta = self.send(ix, &admin, &[]);
        (PoolKeys::new(&admin.pubkey(), &asset_mint), meta)
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
                share_mint: k.share_mint,
                vault: k.vault,
                dead_shares: k.dead_shares,
                depositor_asset_account: asset_account,
                depositor_share_account: share_account,
                token_program: TOKEN_PROGRAM_ID,
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
        self.deposit_ix_full(k, depositor, ata(depositor, &k.asset_mint), ata(depositor, &k.share_mint), assets, min_shares_out)
    }

    pub fn deposit_ix(&self, k: &PoolKeys, depositor: &Pubkey, assets: u64) -> Instruction {
        self.deposit_ix_with(k, depositor, ata(depositor, &k.asset_mint), ata(depositor, &k.share_mint), assets)
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
                share_mint: k.share_mint,
                vault: k.vault,
                holder_share_account: ata(holder, &k.share_mint),
                holder_asset_account: ata(holder, &k.asset_mint),
                token_program: TOKEN_PROGRAM_ID,
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
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
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
                schedule: p.schedule,
            }
            .data(),
        }
    }

    /// Default terms: `principal` at `rate_bps`, 30 days, a fixed agreement hash, no sponsor.
    pub fn line_params(&self, principal: u64, rate_bps: u16) -> LineParams {
        LineParams {
            principal,
            rate_bps,
            term_end: self.now() + 30 * 24 * 3600,
            agreement_hash: AGREEMENT_HASH,
            sponsor: None,
            schedule: None,
        }
    }

    /// Terms of a scheduled line: `installments` monthly installments (30 days), a term that ends
    /// exactly at the last due date if the line is drawn now, and the demo policy of three missed
    /// installments before collection.
    pub fn scheduled_params(&self, principal: u64, rate_bps: u16, installments: u16, late_bps: u16) -> LineParams {
        LineParams {
            principal,
            rate_bps,
            term_end: self.now() + i64::from(installments) * MONTH,
            agreement_hash: AGREEMENT_HASH,
            sponsor: None,
            schedule: Some(elote_pool::state::ScheduleParams {
                interval_seconds: MONTH,
                installments,
                late_bps,
                max_missed: 3,
            }),
        }
    }

    /// Opens the next line of the pool, signed by the admin, the borrower and (if named) the sponsor.
    pub fn open_line_with(
        &mut self,
        k: &PoolKeys,
        borrower: &Keypair,
        p: &LineParams,
        sponsor: Option<&Keypair>,
    ) -> (u64, TransactionMetadata) {
        let admin = self.admin.insecure_clone();
        let line_id = self.pool(k).line_count;
        let ix = self.open_line_ix(k, &admin.pubkey(), line_id, &borrower.pubkey(), p, sponsor.map(|s| s.pubkey()));
        let mut extra: Vec<&Keypair> = vec![borrower];
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

    pub fn draw_ix(&self, k: &PoolKeys, line_id: u64, borrower: &Pubkey) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Draw {
                borrower: *borrower,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                vault: k.vault,
                borrower_asset_account: ata(borrower, &k.asset_mint),
                token_program: TOKEN_PROGRAM_ID,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::Draw {}.data(),
        }
    }

    pub fn draw(&mut self, k: &PoolKeys, line_id: u64, borrower: &Keypair) -> elote_pool::events::LineDrawn {
        let ix = self.draw_ix(k, line_id, &borrower.pubkey());
        let meta = self.send(ix, borrower, &[]);
        find_event(&meta).expect("LineDrawn event")
    }

    pub fn repay_ix(&self, k: &PoolKeys, line_id: u64, payer: &Pubkey, amount: u64) -> Instruction {
        let (event_authority, program) = event_metas();
        Instruction {
            program_id: PROGRAM_ID,
            accounts: elote_pool::accounts::Repay {
                payer: *payer,
                pool: k.pool,
                line: line_pda(&k.pool, line_id).0,
                vault: k.vault,
                payer_asset_account: ata(payer, &k.asset_mint),
                token_program: TOKEN_PROGRAM_ID,
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
                vault: k.vault,
                event_authority,
                program,
            }
            .to_account_metas(None),
            data: elote_pool::instruction::WriteOff {}.data(),
        }
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
        self.transfer(&from, &k.vault, &donor, amount);
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
