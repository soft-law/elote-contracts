//! Pool, deposits and redemptions at net asset value.

mod common;

use anchor_lang::{InstructionData, Space};
use anchor_spl::token_2022::spl_token_2022;
use common::*;
use elote_pool::events::{
    AttestorSet, NavUpdated, PoolCollectionSet, PoolConfigured, PoolFlags, PoolInitialized, PoolLineCapSet, PoolPaused,
};
use elote_pool::state::{ASSET_CONTROL_FREEZE, ASSET_CONTROL_PAUSABLE};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use spl_token_2022::extension::{self as ext, ExtensionType};
use spl_token_2022::state::AccountState;

/// Value of one whole share in asset units, at `nav` and `supply`, rounded down.
fn per_share(nav: u64, supply: u64) -> u128 {
    (nav as u128) * (UNIT as u128) / (supply as u128)
}

#[test]
fn init_pool_creates_share_mint_and_vault() {
    let mut env = Env::new();
    let (k, meta) = env.setup_pool();

    let pool = env.pool(&k);
    assert_eq!(pool.version, 2, "pool v2");
    assert_eq!(pool.admin, env.admin.pubkey());
    assert_eq!(pool.asset_mint, k.asset_mint);
    assert_eq!(pool.vault, k.vault);
    assert_eq!(pool.share_mint, k.share_mint);
    assert_eq!(pool.outstanding_principal, 0);
    assert_eq!(pool.interest_collected, 0);
    assert_eq!(pool.written_off, 0);
    assert_eq!(pool.line_count, 0);
    assert!(!pool.paused);
    assert_eq!(pool.bump, pool_pda(&env.admin.pubkey(), &k.asset_mint).1);

    // The share mint is supplied by the admin, not created here: Token-2022,
    // mint authority = the pool PDA, the asset's decimals, nothing minted yet.
    let (share_mint, extensions) = env.mint_state(&k.share_mint);
    assert_eq!(env.mint_program(&k.share_mint), TOKEN_2022_PROGRAM_ID);
    assert_eq!(share_mint.decimals, 6, "share decimals follow the asset");
    assert_eq!(share_mint.supply, 0);
    assert_eq!(Option::<solana_pubkey::Pubkey>::from(share_mint.mint_authority), Some(k.pool));
    assert_eq!(Option::<solana_pubkey::Pubkey>::from(share_mint.freeze_authority), None);
    assert!(extensions.is_empty());
    assert!(!pool.permissioned(), "no DefaultAccountState: holders need no thaw");

    let vault = env.token_account(&k.vault);
    assert_eq!(vault.mint, k.asset_mint);
    assert_eq!(vault.owner, k.pool);
    assert_eq!(vault.amount, 0);
    assert_eq!(k.vault, vault_pda(&k.pool).0, "the vault is a PDA token account, not an ATA");
    assert_eq!(pool.dead_shares, k.dead_shares);
    let dead = env.token_account(&k.dead_shares);
    assert_eq!(dead.mint, k.share_mint);
    assert_eq!(dead.owner, k.pool);
    assert_eq!(dead.amount, 0);
    assert_eq!(env.account(&k.dead_shares).owner, TOKEN_2022_PROGRAM_ID, "the dead account follows the share mint's program");

    let ev: PoolInitialized = find_event(&meta).expect("PoolInitialized");
    assert_eq!(ev.pool, k.pool);
    assert_eq!(ev.asset_mint, k.asset_mint);
    assert_eq!(ev.share_mint, k.share_mint);
    assert_eq!(ev.admin, env.admin.pubkey());

    // A second init for the same admin and asset is refused (the pool account already exists).
    let admin = env.admin.insecure_clone();
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    assert!(env.try_send(ix, &admin, &[]).is_err());
}

#[test]
fn first_deposit_mints_one_to_one_and_enforces_minimum() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 5 * UNIT);

    let ix = env.deposit_ix(&k, &a.pubkey(), 0);
    env.expect_err(ix, &a, &[], E_WRONG_AMOUNT);
    let ix = env.deposit_ix(&k, &a.pubkey(), UNIT - 1);
    env.expect_err(ix, &a, &[], E_FIRST_DEPOSIT_TOO_SMALL);
    assert_eq!(env.token_balance(&k.vault), 0);

    let ev = env.deposit(&k, &a, UNIT);
    assert_eq!(ev.pool, k.pool);
    assert_eq!(ev.depositor, a.pubkey());
    assert_eq!(ev.assets, UNIT);
    // One-to-one, minus the dead shares locked in the pool.
    assert_eq!(ev.shares, UNIT - DEAD_SHARES);
    assert_eq!(ev.nav_after, UNIT);
    assert_eq!(ev.supply_after, UNIT);
    assert_eq!(env.token_balance(&k.share_ata(&a.pubkey())), UNIT - DEAD_SHARES);
    assert_eq!(env.token_balance(&k.dead_shares), DEAD_SHARES);
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.asset_mint)), 4 * UNIT);
    assert_eq!(env.token_balance(&k.vault), UNIT);

    // The minimum and the dead shares only apply while the supply is zero.
    let ev = env.deposit(&k, &a, 1);
    assert_eq!(ev.shares, 1);
    assert_eq!(env.supply(&k), UNIT + 1);
    assert_eq!(env.token_balance(&k.dead_shares), DEAD_SHARES);
}

#[test]
fn deposit_after_interest_mints_fewer_shares() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 10 * UNIT);
    let b = env.investor(&k, UNIT);
    env.deposit(&k, &a, 10 * UNIT);

    // A line of 4.0 at 10% is drawn and repaid with its interest: NAV 10.4, supply 10.
    let interest = env.lend_and_repay(&k, 4 * UNIT, 1_000);
    assert_eq!(interest, 400_000);
    assert_eq!(env.nav(&k), 10_400_000);
    assert_eq!(env.supply(&k), 10 * UNIT);

    let ev = env.deposit(&k, &b, UNIT);
    // floor(1_000_000 * 10_000_000 / 10_400_000) = floor(961_538.46)
    assert_eq!(ev.shares, 961_538);
    assert!(ev.shares < UNIT);
    assert_eq!(ev.nav_after, 11_400_000);
    assert_eq!(ev.supply_after, 10_961_538);
}

#[test]
fn redeem_returns_pro_rata_and_respects_liquidity() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 10 * UNIT);
    let b = env.investor(&k, 5_200_000);
    let nobody = env.investor(&k, 0);
    env.deposit(&k, &a, 10 * UNIT);
    env.lend_and_repay(&k, 4 * UNIT, 1_000); // NAV 10.4, supply 10.0: 1.04 per share
    let ev = env.deposit(&k, &b, 5_200_000);
    assert_eq!(ev.shares, 5 * UNIT);
    assert_eq!((ev.nav_after, ev.supply_after), (15_600_000, 15 * UNIT));

    // A second line takes 12.0 out of the vault: 3.6 liquid, 12.0 lent.
    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 12 * UNIT, 1_000);
    env.draw(&k, line_id, &borrower);
    assert_eq!(env.token_balance(&k.vault), 3_600_000);
    assert_eq!(env.nav(&k), 15_600_000);
    let before = per_share(env.nav(&k), env.supply(&k));

    let ix = env.redeem_ix(&k, &a.pubkey(), 0);
    env.expect_err(ix, &a, &[], E_WRONG_AMOUNT);

    // Everything A holds is worth about 10.4: more than the vault has. Nothing is burned.
    let ix = env.redeem_ix(&k, &a.pubkey(), 10 * UNIT - DEAD_SHARES);
    env.expect_err(ix, &a, &[], E_INSUFFICIENT_LIQUIDITY);
    assert_eq!(env.token_balance(&k.share_ata(&a.pubkey())), 10 * UNIT - DEAD_SHARES);
    assert_eq!(env.supply(&k), 15 * UNIT);

    // What fits is paid pro rata.
    let ev = env.redeem(&k, &a, 3 * UNIT);
    assert_eq!(ev.holder, a.pubkey());
    assert_eq!(ev.shares, 3 * UNIT);
    assert_eq!(ev.assets, 3_120_000);
    assert_eq!(ev.nav_after, 12_480_000);
    assert_eq!(ev.supply_after, 12 * UNIT);
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.asset_mint)), 3_120_000);
    assert_eq!(env.token_balance(&k.share_ata(&a.pubkey())), 7 * UNIT - DEAD_SHARES);
    assert_eq!(per_share(env.nav(&k), env.supply(&k)), before, "redeeming leaves the value per share unchanged");

    // 1.0 share is worth 1.04; 0.48 is left in the vault.
    let ix = env.redeem_ix(&k, &b.pubkey(), UNIT);
    env.expect_err(ix, &b, &[], E_INSUFFICIENT_LIQUIDITY);

    // Shares the holder does not have cannot be burned.
    let ix = env.redeem_ix(&k, &nobody.pubkey(), 1);
    assert!(env.try_send(ix, &nobody, &[]).is_err());
    assert_eq!(env.supply(&k), 12 * UNIT);
}


#[test]
fn zero_shares_deposit_fails() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, UNIT);
    let victim = env.investor(&k, 100);
    env.deposit(&k, &a, UNIT);

    // Inflation attempt: a direct donation lifts NAV to 11.0 against a supply of 1.0.
    env.donate(&k, 10 * UNIT);

    let ix = env.deposit_ix(&k, &victim.pubkey(), 1);
    env.expect_err(ix, &victim, &[], E_ZERO_SHARES);
    let ix = env.deposit_ix(&k, &victim.pubkey(), 10);
    env.expect_err(ix, &victim, &[], E_ZERO_SHARES);
    assert_eq!(env.token_balance(&ata(&victim.pubkey(), &k.asset_mint)), 100, "nothing left the depositor");

    let ev = env.deposit(&k, &victim, 11);
    assert_eq!(ev.shares, 1);
}

#[test]
fn deposit_and_redeem_reject_wrong_accounts() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 2 * UNIT);
    let other = env.investor(&k, 0);

    // Share account of somebody else.
    let ix = env.deposit_ix_with(&k, &a.pubkey(), ata(&a.pubkey(), &k.asset_mint), k.share_ata(&other.pubkey()), UNIT);
    env.expect_err(ix, &a, &[], E_WRONG_ACCOUNT);
    // Asset and share accounts swapped: each has the wrong mint (and, now, the wrong token
    // program, which Anchor's owner check catches first).
    let ix = env.deposit_ix_with(&k, &a.pubkey(), k.share_ata(&a.pubkey()), ata(&a.pubkey(), &k.asset_mint), UNIT);
    env.expect_err(ix, &a, &[], E_WRONG_ACCOUNT);

    // A vault that is not the pool's.
    let mut ix = env.deposit_ix(&k, &a.pubkey(), UNIT);
    let vault_index = ix.accounts.iter().position(|m| m.pubkey == k.vault).expect("vault meta");
    ix.accounts[vault_index].pubkey = ata(&other.pubkey(), &k.asset_mint);
    env.expect_err(ix, &a, &[], E_WRONG_ACCOUNT);
}

#[test]
fn paused_blocks_deposit_and_redeem() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 3 * UNIT);
    env.deposit(&k, &a, UNIT);

    // Only the admin pauses.
    let ix = env.set_paused_ix(&k, &a.pubkey(), true);
    env.expect_err(ix, &a, &[], E_UNAUTHORIZED);

    let meta = env.set_paused(&k, true);
    let ev: PoolPaused = find_event(&meta).expect("PoolPaused");
    assert_eq!((ev.pool, ev.paused), (k.pool, true));
    assert!(env.pool(&k).paused);

    let ix = env.deposit_ix(&k, &a.pubkey(), UNIT);
    env.expect_err(ix, &a, &[], E_PAUSED);
    let ix = env.redeem_ix(&k, &a.pubkey(), UNIT);
    env.expect_err(ix, &a, &[], E_PAUSED);

    let meta = env.set_paused(&k, false);
    let ev: PoolPaused = find_event(&meta).expect("PoolPaused");
    assert!(!ev.paused);
    env.deposit(&k, &a, UNIT);
    env.redeem(&k, &a, UNIT);
}

/// Reviewer's proof of concept: redeem down to a tiny supply, donate, and let the next depositor
/// round down. The dead shares keep the supply at 1_000 or more, so the victim is priced fairly.
#[test]
fn dead_shares_stop_redeem_then_donate_inflation() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let attacker = env.investor(&k, UNIT);
    let victim = env.investor(&k, 2_000);

    let ev = env.deposit(&k, &attacker, UNIT);
    assert_eq!(ev.shares, UNIT - DEAD_SHARES);
    // The old attack redeemed 999,999 and kept 1 share; the attacker now holds only 999,000.
    // The program's checks pass (999,999 assets fit in the vault); SPL Token's burn refuses it.
    let ix = env.redeem_ix(&k, &attacker.pubkey(), 999_999);
    env.expect_err(ix, &attacker, &[], SPL_TOKEN_INSUFFICIENT_FUNDS);
    let ev = env.redeem(&k, &attacker, UNIT - DEAD_SHARES);
    assert_eq!(ev.assets, UNIT - DEAD_SHARES);
    assert_eq!(env.supply(&k), DEAD_SHARES);
    assert_eq!(env.nav(&k), DEAD_SHARES);

    env.donate(&k, 1_000); // NAV 2_000 against 1_000 dead shares

    // The victim's slippage floor is honoured either way.
    let ix = env.deposit_ix_min(&k, &victim.pubkey(), 2_000, 1_001);
    env.expect_err(ix, &victim, &[], E_SLIPPAGE_EXCEEDED);
    let ix = env.deposit_ix_min(&k, &victim.pubkey(), 2_000, 1_000);
    let meta = env.send(ix, &victim, &[]);
    let ev: elote_pool::events::Deposited = find_event(&meta).expect("Deposited");
    assert_eq!(ev.shares, 1_000);
    assert_eq!((ev.nav_after, ev.supply_after), (4_000, 2_000));

    // The victim's shares are worth exactly what was deposited.
    let ev = env.redeem(&k, &victim, 1_000);
    assert_eq!(ev.assets, 2_000);
}

#[test]
fn redeem_rounds_down_and_honours_min_assets_out() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &a, 10 * UNIT);
    env.lend_and_repay(&k, 4 * UNIT, 1_000); // NAV 10.4, supply 10.0

    // 333_333 * 10_400_000 / 10_000_000 = 346_666.32: the holder receives 346_666.
    let ix = env.redeem_ix_min(&k, &a.pubkey(), 333_333, 346_667);
    env.expect_err(ix, &a, &[], E_SLIPPAGE_EXCEEDED);
    let ix = env.redeem_ix_min(&k, &a.pubkey(), 333_333, 346_666);
    let meta = env.send(ix, &a, &[]);
    let ev: elote_pool::events::Redeemed = find_event(&meta).expect("Redeemed");
    assert_eq!(ev.assets, 346_666);
    assert_eq!(ev.nav_after, 10_400_000 - 346_666);
    assert_eq!(ev.supply_after, 10 * UNIT - 333_333);
    // 7 * 1.04 = 7.28 rounds down to 7.
    assert_eq!(env.redeem(&k, &a, 7).assets, 7);
}

#[test]
fn init_pool_records_the_grace_period() {
    let mut env = Env::new();
    let grace: u32 = 5 * 24 * 3600;
    let (k, meta) = env.setup_pool_with_grace(grace);

    let pool = env.pool(&k);
    assert_eq!(pool.version, 2);
    assert_eq!(pool.grace_period_seconds(), grace);
    assert_eq!(&pool.reserved[0..4], &grace.to_le_bytes(), "bytes 0..4 of reserved, little endian");
    assert_eq!(&pool.reserved[4..6], &BPS.to_le_bytes(), "bytes 4..6: the harness default cap of 10,000 bps");
    assert_eq!(&pool.reserved[6..8], &[0u8; 2], "flags are zero");
    assert_eq!(&pool.reserved[8..12], &CANCEL_DELAY.to_le_bytes(), "bytes 8..12: the cancel delay of the backstop");
    assert_eq!(&pool.reserved[12..16], &COLLECTION_PERIOD.to_le_bytes(), "bytes 12..16: the collection period");
    assert_eq!(&pool.reserved[16..], &[0u8; 16], "the rest of reserved stays zero");

    let ev: PoolInitialized = find_event(&meta).expect("PoolInitialized");
    assert_eq!(ev.pool, k.pool);
    let ev: PoolConfigured = find_event(&meta).expect("PoolConfigured");
    assert_eq!((ev.pool, ev.grace_period_seconds), (k.pool, grace));

    // The default pool of the other tests has no grace.
    let (k0, meta0) = env.setup_pool();
    assert_eq!(env.pool(&k0).grace_period_seconds(), 0);
    assert_eq!(&env.pool(&k0).reserved[0..4], &[0u8; 4]);
    let ev: PoolConfigured = find_event(&meta0).expect("PoolConfigured");
    assert_eq!(ev.grace_period_seconds, 0);
}

/// `max_line_bps` is fixed at `init_pool`, lives in `reserved[4..6]` and is
/// announced by `PoolLineCapSet`. Zero and anything above 10,000 bps are refused.
#[test]
fn init_pool_records_the_line_cap_and_bounds_it() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 6);
    let (k, ix) = env.init_pool_full(&mint, 0, 1_000);
    let meta = env.send(ix, &admin, &[]);

    let pool = env.pool(&k);
    assert_eq!(pool.max_line_bps(), 1_000);
    assert_eq!(&pool.reserved[4..6], &1_000u16.to_le_bytes(), "bytes 4..6 of reserved, little endian");
    assert_eq!(pool.max_line_principal(10 * UNIT), Some(UNIT));
    let ev: PoolLineCapSet = find_event(&meta).expect("PoolLineCapSet");
    assert_eq!((ev.pool, ev.max_line_bps), (k.pool, 1_000));

    for bad in [0u16, BPS + 1, u16::MAX] {
        let mint = env.create_mint(&admin.pubkey(), 6);
        let (_, ix) = env.init_pool_full(&mint, 0, bad);
        env.expect_err(ix, &admin, &[], E_WRONG_AMOUNT);
    }
}

/// Series-shelf 2a: the program speaks the token interface, so a Token-2022 asset (MEX, CETES)
/// runs the whole cycle: deposit, draw, repay with interest, redeem at the higher NAV.
#[test]
fn a_pool_on_a_token_2022_asset_runs_the_full_cycle() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_2022();
    assert_eq!(env.mint_program(&k.asset_mint), TOKEN_2022_PROGRAM_ID);
    assert_eq!(env.account(&k.vault).owner, TOKEN_2022_PROGRAM_ID, "the vault lives under Token-2022");

    let a = env.investor(&k, 10 * UNIT);
    let minted = env.deposit(&k, &a, 10 * UNIT).shares;
    assert_eq!(minted, 10 * UNIT - DEAD_SHARES);
    assert_eq!(env.token_balance(&k.vault), 10 * UNIT);

    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);
    env.draw(&k, line_id, &borrower);
    assert_eq!(env.token_balance(&k.asset_ata(&borrower.pubkey())), 4 * UNIT);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), 400_000);
    env.repay(&k, line_id, &borrower, 4 * UNIT + 400_000);
    assert_eq!(env.nav(&k), 10 * UNIT + 400_000);

    let out = env.redeem(&k, &a, minted);
    assert_eq!(out.assets, (minted as u128 * (10 * UNIT + 400_000) as u128 / (10 * UNIT) as u128) as u64);
    assert_eq!(env.token_balance(&k.asset_ata(&a.pubkey())), out.assets);
}

/// A 6-decimal Token-2022 mint (authority = admin) with the given extensions initialised by
/// `ixs(mint)` before `initialize_mint2`.
fn mint_2022_with(env: &mut Env, exts: &[ExtensionType], ixs: impl FnOnce(&Pubkey) -> Vec<Instruction>) -> Pubkey {
    let admin = env.admin.pubkey();
    env.create_mint_in(&TOKEN_2022_PROGRAM_ID, &admin, None, 6, exts, ixs)
}

fn expect_asset_rejected(env: &mut Env, mint: &Pubkey) {
    let admin = env.admin.insecure_clone();
    let (_, ix) = env.init_pool_plain(mint);
    env.expect_err(ix, &admin, &[], E_ASSET_MINT_NOT_ALLOWED);
}

fn init_accepting(env: &mut Env, mint: &Pubkey) -> (PoolKeys, PoolFlags) {
    let admin = env.admin.insecure_clone();
    let (k, ix) = env.init_pool_plain(mint);
    let meta = env.send(ix, &admin, &[]);
    (k, find_event(&meta).expect("PoolFlags"))
}

/// The asset mint may not carry an extension that takes or hides
/// value (transfer fee, permanent delegate, transfer hook, non-transferable, any confidential
/// extension, mint close authority) or that would freeze the vault at creation.
#[test]
fn init_pool_rejects_asset_mints_whose_extensions_take_or_hide_value() {
    let p = TOKEN_2022_PROGRAM_ID;
    let mut env = Env::new();
    let admin = env.admin.pubkey();

    let m = mint_2022_with(&mut env, &[ExtensionType::TransferFeeConfig], |mint| {
        vec![ext::transfer_fee::instruction::initialize_transfer_fee_config(&p, mint, Some(&admin), Some(&admin), 50, 1_000).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(&mut env, &[ExtensionType::PermanentDelegate], |mint| {
        vec![spl_token_2022::instruction::initialize_permanent_delegate(&p, mint, &admin).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(&mut env, &[ExtensionType::TransferHook], |mint| {
        vec![ext::transfer_hook::instruction::initialize(&p, mint, Some(admin), Some(Pubkey::new_unique())).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(&mut env, &[ExtensionType::NonTransferable], |mint| {
        vec![spl_token_2022::instruction::initialize_non_transferable_mint(&p, mint).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(&mut env, &[ExtensionType::MintCloseAuthority], |mint| {
        vec![spl_token_2022::instruction::initialize_mint_close_authority(&p, mint, Some(&admin)).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    // The three confidential extensions, whatever their flags: a mint that can hold confidential
    // balances makes `vault.amount` unreliable as the NAV's first term.
    let m = mint_2022_with(&mut env, &[ExtensionType::ConfidentialTransferMint], |mint| {
        vec![ext::confidential_transfer::instruction::initialize_mint(&p, mint, Some(admin), true, None).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(
        &mut env,
        &[ExtensionType::TransferFeeConfig, ExtensionType::ConfidentialTransferMint, ExtensionType::ConfidentialTransferFeeConfig],
        |mint| {
            vec![
                ext::transfer_fee::instruction::initialize_transfer_fee_config(&p, mint, Some(&admin), Some(&admin), 0, 0).unwrap(),
                ext::confidential_transfer::instruction::initialize_mint(&p, mint, Some(admin), true, None).unwrap(),
                ext::confidential_transfer_fee::instruction::initialize_confidential_transfer_fee_config(
                    &p,
                    mint,
                    Some(admin),
                    &Default::default(),
                )
                .unwrap(),
            ]
        },
    );
    expect_asset_rejected(&mut env, &m);

    let m = mint_2022_with(&mut env, &[ExtensionType::ConfidentialTransferMint, ExtensionType::ConfidentialMintBurn], |mint| {
        vec![
            ext::confidential_transfer::instruction::initialize_mint(&p, mint, Some(admin), true, None).unwrap(),
            ext::confidential_mint_burn::instruction::initialize_mint(&p, mint, &Default::default(), &Default::default()).unwrap(),
        ]
    });
    expect_asset_rejected(&mut env, &m);

    // Frozen by default: every vault would be born frozen. Initialized by default is harmless.
    // (Token-2022 only allows the extension on a mint with a freeze authority.)
    let m = env.create_mint_in(&p, &admin, Some(&admin), 6, &[ExtensionType::DefaultAccountState], |mint| {
        vec![ext::default_account_state::instruction::initialize_default_account_state(&p, mint, &AccountState::Frozen).unwrap()]
    });
    expect_asset_rejected(&mut env, &m);
    let m = env.create_mint_in(&p, &admin, Some(&admin), 6, &[ExtensionType::DefaultAccountState], |mint| {
        vec![ext::default_account_state::instruction::initialize_default_account_state(&p, mint, &AccountState::Initialized).unwrap()]
    });
    let (_, flags) = init_accepting(&mut env, &m);
    assert_eq!(flags.asset_controls, ASSET_CONTROL_FREEZE, "only the freeze authority it needed");
}

/// Decision 15 and the coordinator's step-2 answer: issuer controls that can stop the vault
/// without touching the program (a freeze authority, the Pausable extension) are accepted and
/// recorded in `asset_controls`; display-only extensions such as interest-bearing are accepted.
#[test]
fn init_pool_records_the_asset_issuer_controls_it_accepts() {
    let p = TOKEN_2022_PROGRAM_ID;
    let mut env = Env::new();
    let admin = env.admin.pubkey();

    // A classic mint with a freeze authority (MEXe's shape).
    let m = env.create_mint_in(&TOKEN_PROGRAM_ID, &admin, Some(&admin), 6, &[], |_| vec![]);
    let (k, flags) = init_accepting(&mut env, &m);
    assert_eq!((flags.pool, flags.asset_controls, flags.permissioned), (k.pool, ASSET_CONTROL_FREEZE, false));
    let pool = env.pool(&k);
    assert!(pool.asset_freezable() && !pool.asset_pausable());
    assert_eq!(pool.reserved[7], ASSET_CONTROL_FREEZE, "byte 7 of reserved");

    // A pausable Token-2022 mint without a freeze authority.
    let m = mint_2022_with(&mut env, &[ExtensionType::Pausable], |mint| {
        vec![ext::pausable::instruction::initialize(&p, mint, &admin).unwrap()]
    });
    let (k, flags) = init_accepting(&mut env, &m);
    assert_eq!(flags.asset_controls, ASSET_CONTROL_PAUSABLE);
    let pool = env.pool(&k);
    assert!(!pool.asset_freezable() && pool.asset_pausable());

    // Both controls at once.
    let m = env.create_mint_in(&p, &admin, Some(&admin), 6, &[ExtensionType::Pausable], |mint| {
        vec![ext::pausable::instruction::initialize(&p, mint, &admin).unwrap()]
    });
    let (_, flags) = init_accepting(&mut env, &m);
    assert_eq!(flags.asset_controls, ASSET_CONTROL_FREEZE | ASSET_CONTROL_PAUSABLE);

    // Interest-bearing is a display rate; neither a control nor a reason to refuse.
    let m = mint_2022_with(&mut env, &[ExtensionType::InterestBearingConfig], |mint| {
        vec![ext::interest_bearing_mint::instruction::initialize(&p, mint, Some(admin), 500).unwrap()]
    });
    let (_, flags) = init_accepting(&mut env, &m);
    assert_eq!(flags.asset_controls, 0);

    // The plain mints of the other tests carry nothing.
    let (k, meta) = env.setup_pool();
    let flags: PoolFlags = find_event(&meta).expect("PoolFlags");
    assert_eq!((flags.asset_controls, flags.permissioned, env.pool(&k).asset_controls()), (0, false, 0));
}

/// `attestor` and `collection` are appended after `bump`
/// so every earlier offset holds, the account grows by exactly 64 bytes and `version` is 2.
#[test]
fn init_pool_records_the_attestor_and_the_collection_as_version_2() {
    let mut env = Env::new();
    let (k, meta) = env.setup_pool();
    let pool = env.pool(&k);
    assert_eq!(pool.version, 2);
    assert_eq!(pool.attestor, env.attestor.pubkey());
    assert_eq!(pool.collection, k.collection);
    assert_eq!(elote_pool::state::Pool::INIT_SPACE, 227 + 64, "64 bytes more than the v1 layout");
    assert_eq!(env.account(&k.pool).data.len(), 8 + 227 + 64);
    // The two keys sit after `bump` at offsets 235 and 267 (discriminator included).
    let data = env.account(&k.pool).data;
    assert_eq!(&data[235..267], env.attestor.pubkey().as_ref());
    assert_eq!(&data[267..299], k.collection.as_ref());
    let ev: AttestorSet = find_event(&meta).expect("AttestorSet");
    assert_eq!((ev.pool, ev.attestor), (k.pool, env.attestor.pubkey()));
    let ev: PoolCollectionSet = find_event(&meta).expect("PoolCollectionSet");
    assert_eq!((ev.pool, ev.collection), (k.pool, k.collection));
    // The collection is a real Metaplex Core collection the pool controls, read back through the
    // crate's decoder against the pinned program binary.
    let c = env.collection_state(&k.collection);
    assert_eq!(c.key, mpl_core::types::Key::CollectionV1);
    assert_eq!(c.update_authority, k.pool);
    assert_eq!((c.num_minted, c.current_size), (0, 0));
    assert_eq!(env.account(&k.collection).owner, MPL_CORE_ID);

    // The zero key is not an attestor: nobody could ever co-sign.
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 6);
    let (k2, mut ix) = env.init_pool_plain(&mint);
    ix.data = elote_pool::instruction::InitPool { grace_period_seconds: 0, max_line_bps: BPS, attestor: Pubkey::default(), collection_period_seconds: COLLECTION_PERIOD, cancel_delay_seconds: CANCEL_DELAY }.data();
    env.expect_err(ix, &admin, &[], E_WRONG_ACCOUNT);
    assert!(env.svm.get_account(&k2.pool).is_none());
}

/// Decision 24: the collection the pool records must be a Metaplex Core collection whose update
/// authority is the pool PDA, so only the program mints into it. Anything else is refused.
#[test]
fn init_pool_rejects_a_collection_the_pool_does_not_control() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 6);
    let (mut k, _) = env.init_pool_plain(&mint);

    // A collection whose update authority is the admin, not the pool.
    k.collection = env.create_collection(&admin.pubkey());
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);
    // A Core asset, not a collection.
    k.collection = env.create_core_asset(&k.pool);
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);
    // An account the Core program does not own.
    k.collection = k.asset_mint;
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);
    // An account that does not exist.
    k.collection = Pubkey::new_unique();
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);
    assert!(env.svm.get_account(&k.pool).is_none());

    // The right one is accepted.
    k.collection = env.create_collection(&k.pool);
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.send(ix, &admin, &[]);
    assert_eq!(env.pool(&k).collection, k.collection);
}

/// `set_attestor` is the recovery when the agreements engine's key rotates or is lost: admin
/// only, co-signed by the new attestor, which proves control of the new key (condition C14),
/// announced by `AttestorSet`.
#[test]
fn set_attestor_is_admin_only_and_co_signed_by_the_new_attestor() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let admin = env.admin.insecure_clone();
    let new_attestor = env.funded(1);

    let stranger = env.funded(1);
    let ix = env.set_attestor_ix(&k, &stranger.pubkey(), &new_attestor.pubkey());
    env.expect_err(ix, &stranger, &[&new_attestor], E_UNAUTHORIZED);
    // The admin alone cannot name a key it does not control.
    let mut ix = env.set_attestor_ix(&k, &admin.pubkey(), &new_attestor.pubkey());
    let i = ix.accounts.iter().position(|m| m.pubkey == new_attestor.pubkey()).unwrap();
    ix.accounts[i].is_signer = false;
    assert!(env.try_send(ix, &admin, &[]).is_err());
    assert_eq!(env.pool(&k).attestor, env.attestor.pubkey(), "unchanged");

    let ix = env.set_attestor_ix(&k, &admin.pubkey(), &new_attestor.pubkey());
    let meta = env.send(ix, &admin, &[&new_attestor]);
    let ev: AttestorSet = find_event(&meta).expect("AttestorSet");
    assert_eq!((ev.pool, ev.attestor), (k.pool, new_attestor.pubkey()));
    assert_eq!(env.pool(&k).attestor, new_attestor.pubkey());
}

/// Series-shelf 2f: `NavUpdated { pool, total_assets, share_supply }` follows every instruction
/// that moves the NAV or the supply (deposit, redeem, repay, write_off), read back after the
/// CPIs, so indexers and the shelf page price shares without a simulation.
#[test]
fn nav_updated_follows_every_money_instruction() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, 10 * UNIT);
    let ix = env.deposit_ix(&k, &a.pubkey(), 10 * UNIT);
    let meta = env.send(ix, &a, &[]);
    let ev: NavUpdated = find_event(&meta).expect("NavUpdated after deposit");
    assert_eq!((ev.pool, ev.total_assets, ev.share_supply), (k.pool, 10 * UNIT, 10 * UNIT));

    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    let meta = env.send_with_attestor(ix, &borrower);
    assert!(find_event::<NavUpdated>(&meta).is_none(), "a draw moves no value: vault down, outstanding up");

    // Principal plus the 0.4 of interest: the NAV rises, the supply does not.
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), 400_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 4 * UNIT + 400_000);
    let meta = env.send(ix, &borrower, &[]);
    let ev: NavUpdated = find_event(&meta).expect("NavUpdated after repay");
    assert_eq!((ev.total_assets, ev.share_supply), (10 * UNIT + 400_000, 10 * UNIT));
    assert_eq!(ev.total_assets, env.nav(&k));

    let ix = env.redeem_ix(&k, &a.pubkey(), UNIT);
    let meta = env.send(ix, &a, &[]);
    let ev: NavUpdated = find_event(&meta).expect("NavUpdated after redeem");
    assert_eq!((ev.total_assets, ev.share_supply), (env.nav(&k), 9 * UNIT));

    // A second line, drawn and never repaid, is written off after its term: the NAV drops by the
    // unpaid principal and the supply stays.
    let second = env.open_line(&k, &borrower, 2 * UNIT, 0);
    env.draw(&k, second, &borrower);
    let before = env.nav(&k);
    env.warp(env.line(&k, second).term_end + 1);
    let ix = env.write_off_ix(&k, second, &admin.pubkey());
    let meta = env.send(ix, &admin, &[]);
    let ev: NavUpdated = find_event(&meta).expect("NavUpdated after write_off");
    assert_eq!((ev.total_assets, ev.share_supply), (before - 2 * UNIT, 9 * UNIT));
    assert_eq!(ev.total_assets, env.nav(&k));
    assert_eq!(env.pool(&k).outstanding_principal, 0);
}

/// A share-mint candidate for a pool on `asset`: Token-2022, 6 decimals, with `exts` initialised
/// by `ixs`, mint authority `authority` and freeze authority `gate`.
fn share_candidate(
    env: &mut Env,
    authority: &Pubkey,
    gate: Option<&Pubkey>,
    decimals: u8,
    exts: &[ExtensionType],
    ixs: impl FnOnce(&Pubkey) -> Vec<Instruction>,
) -> Pubkey {
    env.create_mint_in(&TOKEN_2022_PROGRAM_ID, authority, gate, decimals, exts, ixs)
}

fn expect_share_rejected(env: &mut Env, asset: &Pubkey, share_mint: &Pubkey) {
    let admin = env.admin.insecure_clone();
    let k = PoolKeys::new(&admin.pubkey(), asset, share_mint, TOKEN_PROGRAM_ID, env.mint_program(share_mint));
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_SHARE_MINT_NOT_ALLOWED);
}

/// Series-shelf 2c: the share mint the admin supplies must be one the pool can own and price.
/// Refused: a mint authority other than the pool PDA, a non-zero supply, decimals other than the
/// asset's, a transfer fee, a transfer hook, a close authority, a permanent delegate (decision
/// 15), an interest-bearing display rate, any confidential extension, and a frozen default state
/// without a freeze authority (nothing could ever be thawed).
#[test]
fn init_pool_rejects_share_mints_the_pool_cannot_own_or_price() {
    let p = TOKEN_2022_PROGRAM_ID;
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let asset = env.create_mint(&admin.pubkey(), 6);
    let pool = pool_pda(&admin.pubkey(), &asset).0;

    // Mint authority is the admin, not the pool.
    let m = share_candidate(&mut env, &admin.pubkey(), None, 6, &[], |_| vec![]);
    expect_share_rejected(&mut env, &asset, &m);

    // Right authority, but something was minted before the handover.
    let m = share_candidate(&mut env, &admin.pubkey(), None, 6, &[], |_| vec![]);
    let holder = env.funded(1);
    env.mint_to(&m, &admin, &holder.pubkey(), 1);
    env.set_mint_authority(&m, spl_token_2022::instruction::AuthorityType::MintTokens, &admin, Some(&pool));
    expect_share_rejected(&mut env, &asset, &m);

    // Wrong decimals.
    let m = share_candidate(&mut env, &pool, None, 2, &[], |_| vec![]);
    expect_share_rejected(&mut env, &asset, &m);

    // Extensions that take or hide value, or mislead wallets about the share price.
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::TransferFeeConfig], |mint| {
        vec![ext::transfer_fee::instruction::initialize_transfer_fee_config(&p, mint, Some(&admin.pubkey()), Some(&admin.pubkey()), 50, 1_000).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::TransferHook], |mint| {
        vec![ext::transfer_hook::instruction::initialize(&p, mint, Some(admin.pubkey()), Some(Pubkey::new_unique())).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::MintCloseAuthority], |mint| {
        vec![spl_token_2022::instruction::initialize_mint_close_authority(&p, mint, Some(&admin.pubkey())).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::PermanentDelegate], |mint| {
        vec![spl_token_2022::instruction::initialize_permanent_delegate(&p, mint, &admin.pubkey()).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::InterestBearingConfig], |mint| {
        vec![ext::interest_bearing_mint::instruction::initialize(&p, mint, Some(admin.pubkey()), 500).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    // Audit F-05: a display multiplier misleads wallets like a display rate, and a pause of the
    // share mint would stop every deposit and redemption outside `set_paused` and its event.
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::ScaledUiAmount], |mint| {
        vec![ext::scaled_ui_amount::instruction::initialize(&p, mint, Some(admin.pubkey()), 1.0).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::Pausable], |mint| {
        vec![ext::pausable::instruction::initialize(&p, mint, &admin.pubkey()).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::ConfidentialTransferMint], |mint| {
        vec![ext::confidential_transfer::instruction::initialize_mint(&p, mint, Some(admin.pubkey()), true, None).unwrap()]
    });
    expect_share_rejected(&mut env, &asset, &m);

    // Frozen by default, but the freeze authority was revoked: no holder could ever be thawed.
    let gate = env.funded(1);
    let m = env.create_share_mint(&pool, 6, Some(&gate.pubkey()), true);
    env.set_mint_authority(&m, spl_token_2022::instruction::AuthorityType::FreezeAccount, &gate, None);
    expect_share_rejected(&mut env, &asset, &m);

    // And the plain candidate the harness builds is accepted.
    let (k, ix) = env.init_pool_plain(&asset);
    env.send(ix, &admin, &[]);
    assert_eq!(env.pool(&k).share_mint, k.share_mint);
}

/// Non-transferable shares are an issuer choice (no Token ACL needed for that series): deposit
/// mints and redeem burns, neither transfers, so both still work.
#[test]
fn non_transferable_shares_still_deposit_and_redeem() {
    let p = TOKEN_2022_PROGRAM_ID;
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let asset = env.create_mint(&admin.pubkey(), 6);
    let pool = pool_pda(&admin.pubkey(), &asset).0;
    let m = share_candidate(&mut env, &pool, None, 6, &[ExtensionType::NonTransferable], |mint| {
        vec![spl_token_2022::instruction::initialize_non_transferable_mint(&p, mint).unwrap()]
    });
    let mut k = PoolKeys::new(&admin.pubkey(), &asset, &m, TOKEN_PROGRAM_ID, p);
    k.collection = env.create_collection(&pool);
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.send(ix, &admin, &[]);

    let a = env.investor(&k, 3 * UNIT);
    let minted = env.deposit(&k, &a, 3 * UNIT).shares;
    assert_eq!(minted, 3 * UNIT - DEAD_SHARES);
    let out = env.redeem(&k, &a, minted);
    assert_eq!(out.assets, minted);
}

/// With `DefaultAccountState = Frozen` the shares are
/// permissioned. Every share account is born frozen, the dead account included, so the first
/// deposit fails until the freeze authority (Token ACL's `MintConfig` in production, a plain
/// keypair here) has thawed both the dead account and the depositor's account. The thaw can
/// come in a later transaction or in the `init_pool` transaction itself.
#[test]
fn permissioned_shares_need_a_thaw_before_the_first_deposit() {
    let mut env = Env::new();
    let gate = env.funded(1);
    let (k, meta) = env.setup_permissioned_pool(&gate.pubkey());
    let flags: PoolFlags = find_event(&meta).expect("PoolFlags");
    assert!(flags.permissioned && env.pool(&k).permissioned());
    assert_eq!(env.pool(&k).reserved[6], 1, "byte 6 of reserved");
    assert_eq!(env.token_account(&k.dead_shares).state, AccountState::Frozen);

    // Nothing thawed: the mint into the depositor's frozen account fails inside Token-2022.
    let a = env.investor(&k, 10 * UNIT);
    assert_eq!(env.token_account(&k.share_ata(&a.pubkey())).state, AccountState::Frozen);
    let ix = env.deposit_ix(&k, &a.pubkey(), 10 * UNIT);
    env.expect_err(ix, &a, &[], SPL_TOKEN_ACCOUNT_FROZEN);

    // The depositor is admitted but the dead account is still frozen: the first deposit fails on it.
    env.thaw(&k.share_mint, &k.share_ata(&a.pubkey()), &gate);
    let ix = env.deposit_ix(&k, &a.pubkey(), 10 * UNIT);
    env.expect_err(ix, &a, &[], SPL_TOKEN_ACCOUNT_FROZEN);

    // Dead account thawed by the gate (path 1: the gate admits the pool PDA's account).
    env.thaw(&k.share_mint, &k.dead_shares, &gate);
    let minted = env.deposit(&k, &a, 10 * UNIT).shares;
    assert_eq!(minted, 10 * UNIT - DEAD_SHARES);
    assert_eq!(env.token_balance(&k.dead_shares), DEAD_SHARES);

    // A second investor who was never admitted cannot deposit; once thawed, can.
    let b = env.investor(&k, UNIT);
    let ix = env.deposit_ix(&k, &b.pubkey(), UNIT);
    env.expect_err(ix, &b, &[], SPL_TOKEN_ACCOUNT_FROZEN);
    env.thaw(&k.share_mint, &k.share_ata(&b.pubkey()), &gate);
    env.deposit(&k, &b, UNIT);

    // Path 2: the dead account is thawed in the init_pool transaction, so the pool is born ready.
    let admin = env.admin.insecure_clone();
    let asset = env.create_mint(&admin.pubkey(), 6);
    let pool = pool_pda(&admin.pubkey(), &asset).0;
    let m = env.create_share_mint(&pool, 6, Some(&gate.pubkey()), true);
    let mut k2 = PoolKeys::new(&admin.pubkey(), &asset, &m, TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID);
    k2.collection = env.create_collection(&pool);
    let init = env.init_pool_ix_for(&k2, 0, BPS);
    let thaw = env.thaw_ix(&m, &k2.dead_shares, &gate.pubkey());
    env.send_all(&[init, thaw], &admin, &[&gate]);
    assert_eq!(env.token_account(&k2.dead_shares).state, AccountState::Initialized);
    let c = env.investor(&k2, 2 * UNIT);
    env.thaw(&m, &k2.share_ata(&c.pubkey()), &gate);
    assert_eq!(env.deposit(&k2, &c, 2 * UNIT).shares, 2 * UNIT - DEAD_SHARES);
}

/// SPL Token accepts any `decimals` byte. Above 19, `10^decimals` does not fit a `u64`, so no
/// first deposit could ever meet the minimum; `init_pool` refuses such a mint instead of creating
/// a pool that can never take a deposit.
#[test]
fn init_pool_rejects_a_mint_whose_decimals_make_the_first_deposit_impossible() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 20);
    let (_, ix) = env.init_pool_plain(&mint);
    env.expect_err(ix, &admin, &[], E_WRONG_ACCOUNT);
    // Nineteen decimals is the largest the minimum can express.
    let mint = env.create_mint(&admin.pubkey(), 19);
    let (_, ix) = env.init_pool_plain(&mint);
    env.send(ix, &admin, &[]);
}
