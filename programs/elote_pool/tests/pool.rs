//! Pool, deposits and redemptions at net asset value.

mod common;

use anchor_lang::AccountDeserialize;
use common::*;
use elote_pool::events::{PoolConfigured, PoolInitialized, PoolPaused};
use solana_signer::Signer;

/// Value of one whole share in asset units, at `nav` and `supply`, rounded down.
fn per_share(nav: u64, supply: u64) -> u128 {
    (nav as u128) * (UNIT as u128) / (supply as u128)
}

#[test]
fn init_pool_creates_share_mint_and_vault() {
    let mut env = Env::new();
    let (k, meta) = env.setup_pool();

    let pool = env.pool(&k);
    assert_eq!(pool.version, 1);
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

    let share_mint = env.anchor_account::<anchor_spl::token::Mint>(&k.share_mint);
    assert_eq!(share_mint.decimals, 6, "share decimals follow the asset");
    assert_eq!(share_mint.supply, 0);
    assert_eq!(Option::<solana_pubkey::Pubkey>::from(share_mint.mint_authority), Some(k.pool));
    assert_eq!(Option::<solana_pubkey::Pubkey>::from(share_mint.freeze_authority), None);

    let vault = anchor_spl::token::TokenAccount::try_deserialize(&mut env.account(&k.vault).data.as_slice()).unwrap();
    assert_eq!(vault.mint, k.asset_mint);
    assert_eq!(vault.owner, k.pool);
    assert_eq!(vault.amount, 0);
    assert_eq!(k.vault, vault_pda(&k.pool).0, "the vault is a PDA token account, not an ATA");
    assert_eq!(pool.dead_shares, k.dead_shares);
    let dead = anchor_spl::token::TokenAccount::try_deserialize(&mut env.account(&k.dead_shares).data.as_slice()).unwrap();
    assert_eq!(dead.mint, k.share_mint);
    assert_eq!(dead.owner, k.pool);
    assert_eq!(dead.amount, 0);

    let ev: PoolInitialized = find_event(&meta).expect("PoolInitialized");
    assert_eq!(ev.pool, k.pool);
    assert_eq!(ev.asset_mint, k.asset_mint);
    assert_eq!(ev.share_mint, k.share_mint);
    assert_eq!(ev.admin, env.admin.pubkey());

    // A second init for the same admin and asset is refused (the pool account already exists).
    let admin = env.admin.insecure_clone();
    let ix = env.init_pool_ix(&admin.pubkey(), &k.asset_mint);
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
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.share_mint)), UNIT - DEAD_SHARES);
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
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.share_mint)), 10 * UNIT - DEAD_SHARES);
    assert_eq!(env.supply(&k), 15 * UNIT);

    // What fits is paid pro rata.
    let ev = env.redeem(&k, &a, 3 * UNIT);
    assert_eq!(ev.holder, a.pubkey());
    assert_eq!(ev.shares, 3 * UNIT);
    assert_eq!(ev.assets, 3_120_000);
    assert_eq!(ev.nav_after, 12_480_000);
    assert_eq!(ev.supply_after, 12 * UNIT);
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.asset_mint)), 3_120_000);
    assert_eq!(env.token_balance(&ata(&a.pubkey(), &k.share_mint)), 7 * UNIT - DEAD_SHARES);
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
    let ix = env.deposit_ix_with(&k, &a.pubkey(), ata(&a.pubkey(), &k.asset_mint), ata(&other.pubkey(), &k.share_mint), UNIT);
    env.expect_err(ix, &a, &[], E_WRONG_ACCOUNT);
    // Asset and share accounts swapped: each has the wrong mint.
    let ix = env.deposit_ix_with(&k, &a.pubkey(), ata(&a.pubkey(), &k.share_mint), ata(&a.pubkey(), &k.asset_mint), UNIT);
    env.expect_err(ix, &a, &[], E_WRONG_ACCOUNT);

    // A vault that is not the pool's.
    let mut ix = env.deposit_ix(&k, &a.pubkey(), UNIT);
    ix.accounts[3].pubkey = ata(&other.pubkey(), &k.asset_mint);
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
    assert_eq!(pool.version, 1, "pools stay at version 1");
    assert_eq!(pool.grace_period_seconds(), grace);
    assert_eq!(&pool.reserved[0..4], &grace.to_le_bytes(), "bytes 0..4 of reserved, little endian");
    assert_eq!(&pool.reserved[4..], &[0u8; 28], "the rest of reserved stays zero");

    let ev: PoolInitialized = find_event(&meta).expect("PoolInitialized");
    assert_eq!(ev.pool, k.pool);
    let ev: PoolConfigured = find_event(&meta).expect("PoolConfigured");
    assert_eq!((ev.pool, ev.grace_period_seconds), (k.pool, grace));

    // The default pool of the other tests has no grace.
    let (k0, meta0) = env.setup_pool();
    assert_eq!(env.pool(&k0).grace_period_seconds(), 0);
    assert_eq!(env.pool(&k0).reserved, [0u8; 32]);
    let ev: PoolConfigured = find_event(&meta0).expect("PoolConfigured");
    assert_eq!(ev.grace_period_seconds, 0);
}
