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
