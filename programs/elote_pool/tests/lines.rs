//! Credit lines approved from the pool, with an optional sponsor that co-signs the opening.

mod common;

use common::*;
use elote_pool::events::{LineOpened, LineWrittenOff};
use elote_pool::state::LineStatus;
use solana_keypair::Keypair;
use solana_signer::Signer;

/// A pool with one investor who deposited `assets`.
fn pool_with(env: &mut Env, assets: u64) -> (PoolKeys, Keypair) {
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    (k, a)
}

#[test]
fn open_draw_repay_full_cycle_raises_share_value() {
    let mut env = Env::new();
    let (k, a) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);

    // Open 4.0 at 10%: interest due 0.4.
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, None);
    assert_eq!(line_id, 0);
    let ev: LineOpened = find_event(&meta).expect("LineOpened");
    assert_eq!(ev.pool, k.pool);
    assert_eq!(ev.line_id, 0);
    assert_eq!(ev.borrower, borrower.pubkey());
    assert_eq!(ev.sponsor, None);
    assert_eq!(ev.agreement_hash, AGREEMENT_HASH);
    assert_eq!(ev.principal, 4 * UNIT);
    assert_eq!(ev.rate_bps, 1_000);
    assert_eq!(ev.interest_due, 400_000);
    assert_eq!(ev.term_end, p.term_end);

    let line = env.line(&k, 0);
    assert_eq!(line.version, 1);
    assert_eq!(line.pool, k.pool);
    assert_eq!(line.borrower, borrower.pubkey());
    assert_eq!(line.status, LineStatus::Approved);
    assert_eq!(line.interest_due, 400_000);
    assert_eq!(line.agreement_hash, AGREEMENT_HASH);
    assert_eq!(line.opened_at, env.now());
    assert_eq!(line.bump, line_pda(&k.pool, 0).1);
    assert_eq!(env.pool(&k).line_count, 1);
    assert_eq!(env.nav(&k), 10 * UNIT, "approval moves no money");

    let ev = env.draw(&k, 0, &borrower);
    assert_eq!((ev.pool, ev.line_id, ev.amount, ev.outstanding_after), (k.pool, 0, 4 * UNIT, 4 * UNIT));
    assert_eq!(env.token_balance(&k.vault), 6 * UNIT);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
    assert_eq!(env.nav(&k), 10 * UNIT);
    assert_eq!(env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), 4 * UNIT);
    let line = env.line(&k, 0);
    assert_eq!(line.status, LineStatus::Active);
    assert_eq!(line.drawn_at, env.now());

    // The borrower earns the interest elsewhere.
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), 400_000);

    let ev = env.repay(&k, 0, &borrower, 2 * UNIT);
    assert_eq!(ev.payer, borrower.pubkey());
    assert_eq!((ev.principal_part, ev.interest_part), (2 * UNIT, 0));
    assert_eq!(ev.remaining, 2_400_000);
    assert_eq!(ev.nav_after, 10 * UNIT);
    assert_eq!(env.line(&k, 0).status, LineStatus::Active);

    let ev = env.repay(&k, 0, &borrower, 2_400_000);
    assert_eq!((ev.principal_part, ev.interest_part), (2 * UNIT, 400_000));
    assert_eq!(ev.remaining, 0);
    assert_eq!(ev.nav_after, 10_400_000);
    let line = env.line(&k, 0);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!((line.principal_repaid, line.interest_repaid), (4 * UNIT, 400_000));
    assert_eq!(line.closed_at, env.now());
    let pool = env.pool(&k);
    assert_eq!(pool.outstanding_principal, 0);
    assert_eq!(pool.interest_collected, 400_000);
    assert_eq!(env.nav(&k), 10_400_000);

    // Every share is now worth 1.04.
    let ev = env.redeem(&k, &a, UNIT);
    assert_eq!(ev.assets, 1_040_000);
}

#[test]
fn sponsor_must_cosign_when_named() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let impostor = env.funded(1);
    let admin = env.admin.insecure_clone();
    let mut p = env.line_params(4 * UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());

    // Named but absent.
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_SPONSOR_REQUIRED);
    // Someone else signs in the sponsor's place.
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, Some(impostor.pubkey()));
    env.expect_err(ix, &admin, &[&borrower, &impostor], E_SPONSOR_REQUIRED);
    // The sponsor's account is passed without its signature.
    let mut ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, Some(sponsor.pubkey()));
    let meta = ix.accounts.iter_mut().find(|m| m.pubkey == sponsor.pubkey()).unwrap();
    meta.is_signer = false;
    assert!(env.try_send(ix, &admin, &[&borrower]).is_err());
    assert_eq!(env.pool(&k).line_count, 0);

    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, Some(&sponsor));
    let ev: LineOpened = find_event(&meta).expect("LineOpened");
    assert_eq!(ev.sponsor, Some(sponsor.pubkey()));
    assert_eq!(env.line(&k, line_id).sponsor, Some(sponsor.pubkey()));
}

#[test]
fn sponsor_pays_interest_on_behalf() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &sponsor.pubkey(), 400_000);
    let mut p = env.line_params(4 * UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, Some(&sponsor));
    env.draw(&k, line_id, &borrower);

    let ev = env.repay(&k, line_id, &borrower, 4 * UNIT);
    assert_eq!((ev.principal_part, ev.interest_part, ev.remaining), (4 * UNIT, 0, 400_000));

    let ev = env.repay(&k, line_id, &sponsor, 400_000);
    assert_eq!(ev.payer, sponsor.pubkey());
    assert_eq!((ev.principal_part, ev.interest_part, ev.remaining), (0, 400_000, 0));
    assert_eq!(ev.nav_after, 10_400_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.token_balance(&ata(&sponsor.pubkey(), &k.asset_mint)), 0);
}

#[test]
fn repay_rejects_overpay_and_wrong_status() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), 2 * UNIT);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);

    // Not drawn yet.
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);

    env.draw(&k, line_id, &borrower);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 0);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    let vault_before = env.token_balance(&k.vault);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 4_400_001);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    assert_eq!(env.token_balance(&k.vault), vault_before, "an overpayment never reaches the vault");

    // Paused: a borrower can still repay.
    env.set_paused(&k, true);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.principal_part, ev.remaining), (UNIT, 3_400_000));
    env.set_paused(&k, false);

    env.repay(&k, line_id, &borrower, 3_400_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
    assert_eq!(env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), 1_600_000);
}

#[test]
fn draw_rejects_insufficient_liquidity_and_double_draw() {
    let mut env = Env::new();
    let (k, a) = pool_with(&mut env, 3 * UNIT);
    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);

    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_INSUFFICIENT_LIQUIDITY);
    assert_eq!(env.token_balance(&k.vault), 3 * UNIT);

    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &a.pubkey(), UNIT);
    env.deposit(&k, &a, UNIT);

    // Only the borrower draws.
    let other = env.borrower(&k);
    let ix = env.draw_ix(&k, line_id, &other.pubkey());
    env.expect_err(ix, &other, &[], E_UNAUTHORIZED);

    // Paused: no draw.
    env.set_paused(&k, true);
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_PAUSED);
    env.set_paused(&k, false);

    env.draw(&k, line_id, &borrower);
    assert_eq!(env.token_balance(&k.vault), 0);
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT, "drawn once");
}

#[test]
fn write_off_after_term_lowers_nav_once() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let admin = env.admin.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);

    // An approved line is not written off.
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_LINE_STATUS);

    env.draw(&k, line_id, &borrower);
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.nav(&k), 10 * UNIT);

    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_TERM_NOT_ENDED);
    env.warp(p.term_end);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_TERM_NOT_ENDED);

    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);

    // Pause does not block recognising a loss.
    env.set_paused(&k, true);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    let meta = env.send(ix, &admin, &[]);
    let ev: LineWrittenOff = find_event(&meta).expect("LineWrittenOff");
    assert_eq!((ev.pool, ev.line_id, ev.loss, ev.nav_after), (k.pool, line_id, 3 * UNIT, 7 * UNIT));
    let pool = env.pool(&k);
    assert_eq!(pool.outstanding_principal, 0);
    assert_eq!(pool.written_off, 3 * UNIT);
    assert_eq!(env.nav(&k), 7 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::WrittenOff);
    assert_eq!(line.closed_at, p.term_end + 1);
    env.set_paused(&k, false);

    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_LINE_STATUS);
    assert_eq!(env.nav(&k), 7 * UNIT, "the loss counts once");
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

#[test]
fn open_rejects_bad_params() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let admin = env.admin.insecure_clone();
    let good = env.line_params(4 * UNIT, 1_000);

    let mut p = good;
    p.principal = 0;
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_WRONG_AMOUNT);

    let mut p = good;
    p.rate_bps = 10_001;
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_WRONG_AMOUNT);

    let mut p = good;
    p.term_end = env.now();
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_WRONG_AMOUNT);

    let mut p = good;
    p.agreement_hash = [0; 32];
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_WRONG_AMOUNT);

    // Out of sequence: the next id is 0.
    let ix = env.open_line_ix(&k, &admin.pubkey(), 1, &borrower.pubkey(), &good, None);
    env.expect_err(ix, &admin, &[&borrower], E_WRONG_ACCOUNT);

    // Only the admin approves.
    let ix = env.open_line_ix(&k, &borrower.pubkey(), 0, &borrower.pubkey(), &good, None);
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);

    // Paused: no new lines.
    env.set_paused(&k, true);
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &good, None);
    env.expect_err(ix, &admin, &[&borrower], E_PAUSED);
    env.set_paused(&k, false);

    // The full rate is accepted and the next ids follow in sequence.
    let mut p = good;
    p.rate_bps = 10_000;
    let (id0, _) = env.open_line_with(&k, &borrower, &p, None);
    let (id1, _) = env.open_line_with(&k, &borrower, &good, None);
    assert_eq!((id0, id1), (0, 1));
    assert_eq!(env.line(&k, 0).interest_due, 4 * UNIT);
    assert_eq!(env.pool(&k).line_count, 2);
}

#[test]
fn draw_requires_the_term_not_ended() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (late, _) = env.open_line_with(&k, &borrower, &p, None);
    let (on_time, _) = env.open_line_with(&k, &borrower, &p, None);

    // At the last second of the term a draw is still allowed.
    env.warp(p.term_end);
    env.draw(&k, on_time, &borrower);

    env.warp(p.term_end + 1);
    let ix = env.draw_ix(&k, late, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_TERM_ENDED);
    assert_eq!(env.line(&k, late).status, LineStatus::Approved);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
}
