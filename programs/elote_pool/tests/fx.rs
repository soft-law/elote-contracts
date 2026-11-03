//! The FIX on chain: `set_fx_feeder` and `post_fix`.

mod common;

use common::*;
use elote_pool::errors::PoolError;
use elote_pool::events::{FixPosted, FxFeederSet};
use solana_signer::Signer;

fn code(e: PoolError) -> u32 {
    ERR_BASE + e as u32
}

/// 2026-11-03T20:00Z: 14:00 in Mexico City, after the day's FIX.
const NOV_3_AFTERNOON: i64 = 1_793_707_200 + 8 * 3_600;
const DAY: i64 = 86_400;

fn setup() -> (Env, PoolKeys, Keypair) {
    let mut env = Env::new();
    env.warp(NOV_3_AFTERNOON);
    let (k, _) = env.setup_pool();
    let feeder = env.funded(1);
    (env, k, feeder)
}

#[test]
fn the_council_publishes_the_first_fix_and_names_the_feeder() {
    let (mut env, k, feeder) = setup();
    let council = env.council.insecure_clone();

    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 183_740, 20261103);
    env.expect_err(ix, &feeder, &[], E_UNAUTHORIZED);
    let ix = env.set_fx_feeder_ix(&k, &feeder.pubkey(), &feeder.pubkey());
    env.expect_err(ix, &feeder, &[], E_UNAUTHORIZED);
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &feeder.pubkey());
    let ev: FxFeederSet = find_event(&env.send(ix, &council, &[])).expect("FxFeederSet");
    assert_eq!(ev.fx_feeder, feeder.pubkey());

    // The feeder cannot set the first FIX: there is nothing to bound it by.
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 183_740, 20261103);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixJump));

    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, 20261103);
    let meta = env.send(ix, &council, &[]);
    let ev: FixPosted = find_event(&meta).expect("FixPosted");
    assert_eq!((ev.fix_rate, ev.fix_date, ev.previous_rate, ev.by, ev.posted_at), (183_740, 20261103, 0, council.pubkey(), NOV_3_AFTERNOON));
    let pool = env.pool(&k);
    assert_eq!((pool.fix_rate, pool.fix_date, pool.fix_posted_at), (183_740, 20261103, NOV_3_AFTERNOON));
}

#[test]
fn the_feeder_moves_forward_within_five_percent() {
    let (mut env, k, feeder) = setup();
    let council = env.council.insecure_clone();
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &feeder.pubkey());
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 200_000, 20261102);
    env.send(ix, &council, &[]);

    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_100, 20261102);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixDateInvalid));
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_100, 20261101);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixDateInvalid));
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 210_001, 20261103);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixJump));
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_100, 20261104);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixDateInvalid));
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_100, 20261131);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixDateInvalid));

    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 210_000, 20261103);
    let ev: FixPosted = find_event(&env.send(ix, &feeder, &[])).expect("FixPosted");
    assert_eq!((ev.previous_rate, ev.by), (200_000, feeder.pubkey()));

    // A move beyond the band, and a same-day correction, are the council's.
    env.warp(NOV_3_AFTERNOON + DAY);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 240_000, 20261104);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 239_000, 20261104);
    env.send(ix, &council, &[]);
    assert_eq!((env.pool(&k).fix_rate, env.pool(&k).fix_date), (239_000, 20261104));
    let ix = env.post_fix_ix(&k, &council.pubkey(), 239_000, 20261103);
    env.expect_err(ix, &council, &[], code(PoolError::FixDateInvalid));
    let ix = env.post_fix_ix(&k, &council.pubkey(), 0, 20261104);
    env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);

    // The zero key removes the feeder.
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &solana_pubkey::Pubkey::default());
    env.send(ix, &council, &[]);
    env.warp(NOV_3_AFTERNOON + 2 * DAY);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 239_100, 20261105);
    env.expect_err(ix, &feeder, &[], E_UNAUTHORIZED);
}

#[test]
fn today_in_mexico_city_starts_six_hours_after_midnight_utc() {
    let (mut env, k, _) = setup();
    let council = env.council.insecure_clone();
    // 2026-11-04T03:00Z is still Nov 3 in Mexico City.
    env.warp(1_793_707_200 + 15 * 3_600);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, 20261104);
    env.expect_err(ix, &council, &[], code(PoolError::FixDateInvalid));
    env.warp(1_793_707_200 + 18 * 3_600);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, 20261104);
    env.send(ix, &council, &[]);
}

/// A pool of 5,000 USDC at the real FIX of 18.3740 MXN per USD, with a MXN 50,000 line approved.
fn real_fix_pool(env: &mut Env, max_outstanding: u64) -> (PoolKeys, Keypair, u64) {
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let mut p = env.init_params(0, BPS, env.attestor.pubkey(), COLLECTION_PERIOD, CANCEL_DELAY);
    p.max_outstanding_principal = max_outstanding;
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), p);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, env.today());
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 5_000 * UNIT);
    env.deposit(&k, &investor, 5_000 * UNIT);
    let borrower = env.borrower(&k);
    let terms = env.line_params(5_000_000, 1_200); // MXN 50,000.00
    let (id, _) = env.request_line_with(&k, &borrower, &terms, None);
    env.release_fix(&k);
    env.approve_line(&k, id, &borrower.pubkey(), &terms);
    (k, borrower, id)
}

#[test]
fn a_draw_delivers_usdc_at_the_fix_the_borrower_signed() {
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_pool(&mut env, u64::MAX);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 5_000_000);
    let meta = env.send(ix, &borrower, &[]);
    let usdc = 2_721_236_529; // floor(5_000_000 * 10^8 / 183_740)
    assert_eq!(env.token_balance(&k.asset_ata(&borrower.pubkey())), usdc);
    let ev: elote_pool::events::LineDrawn = find_event(&meta).expect("LineDrawn");
    assert_eq!((ev.amount_mxn, ev.fx_rate, ev.fx_date, ev.usdc_delivered), (5_000_000, 183_740, env.today(), usdc));
    let record = env.pagare(&k, id, 0);
    assert_eq!((record.amount_mxn, record.fx_rate, record.fx_date, record.usdc_delivered), (5_000_000, 183_740, env.today(), usdc));
    let line = env.line(&k, id);
    assert_eq!((line.drawn, line.usdc_drawn), (5_000_000, usdc), "the line counts MXN; its cost is in USDC");
    assert_eq!(env.pool(&k).outstanding_principal, usdc, "the pool's outstanding is the USDC cost");
    assert_eq!(env.nav(&k), 5_000 * UNIT, "a draw moves no value");
}

#[test]
fn a_changed_fix_fails_the_draw_and_the_borrower_confirms_again() {
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_pool(&mut env, u64::MAX);
    let council = env.council.insecure_clone();
    let shown = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    // The council corrects today's FIX between the screen and the signature.
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_750, env.today());
    env.send(ix, &council, &[]);
    env.expect_err(shown, &borrower, &[], code(PoolError::FixMismatch));
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pagare(&k, id, 0).fx_rate, 183_750);
}

#[test]
fn a_fix_older_than_five_days_stops_draws() {
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_pool(&mut env, u64::MAX);
    env.warp(env.now() + 5 * DAY);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.send(ix, &borrower, &[]);
    env.warp(env.now() + DAY);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], code(PoolError::FixStale));
}

#[test]
fn the_pool_cap_on_outstanding_cost_holds_at_every_draw() {
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_pool(&mut env, 1_000 * UNIT);
    // MXN 18,374.00 is exactly 1,000 USDC.
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 1_837_401);
    env.expect_err(ix, &borrower, &[], code(PoolError::OutstandingCapExceeded));
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 1_837_400);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pool(&k).outstanding_principal, 1_000 * UNIT);
}

#[test]
fn the_line_cap_is_measured_in_mxn_at_the_fix() {
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let (k, ix) = env.init_pool_full(&asset, 0, 1_000); // 10% of the NAV per line
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 200_000, env.today()); // 20 MXN per USD
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 1_000 * UNIT);
    env.deposit(&k, &investor, 1_000 * UNIT);
    let borrower = env.borrower(&k);
    // 10% of 1,000 USDC is 100 USDC, MXN 2,000.00.
    for (principal, ok) in [(200_001u64, false), (200_000, true)] {
        let p = env.line_params(principal, 0);
        let (id, _) = env.request_line_with(&k, &borrower, &p, None);
        env.release_fix(&k);
        let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
        let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
        if ok {
            env.send(ix, &council, &[]);
        } else {
            env.expect_err(ix, &council, &[], E_LINE_TOO_LARGE);
        }
    }
}
