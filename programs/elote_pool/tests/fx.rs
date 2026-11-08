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
    let (mut env, k, feeder) = feeder_pool(20261102);
    let council = env.council.insecure_clone();

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

/// A pool with a feeder and a council FIX of 20.0000 dated `fix_date` (Nov 2 at the latest),
/// published on Nov 2; the clock is then at `NOV_3_AFTERNOON`, the feeder's first day.
fn feeder_pool(fix_date: u32) -> (Env, PoolKeys, Keypair) {
    let mut env = Env::new();
    env.warp(NOV_3_AFTERNOON - DAY);
    let (k, _) = env.setup_pool();
    let feeder = env.funded(1);
    let council = env.council.insecure_clone();
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &feeder.pubkey());
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 200_000, fix_date);
    env.send(ix, &council, &[]);
    env.warp(NOV_3_AFTERNOON);
    (env, k, feeder)
}

#[test]
fn the_feeders_band_is_five_percent_a_day_since_the_fix_in_force_up_to_three_days() {
    // The FIX in force is dated Oct 31; today is Nov 3.
    for (rate, date, ok) in [
        (210_000u64, 20261101, true),  // one day: 5%
        (210_001, 20261101, false),
        (220_000, 20261102, true),     // two days: 10%
        (220_001, 20261102, false),
        (230_000, 20261103, true),     // three days: 15%
        (230_001, 20261103, false),
        (170_000, 20261103, true),     // either way
        (169_999, 20261103, false),
    ] {
        let (mut env, k, feeder) = feeder_pool(20261031);
        let ix = env.post_fix_ix(&k, &feeder.pubkey(), rate, date);
        if ok {
            env.send(ix, &feeder, &[]);
            assert_eq!((env.pool(&k).fix_rate, env.pool(&k).fix_date), (rate, date));
        } else {
            env.expect_err(ix, &feeder, &[], code(PoolError::FixJump));
        }
    }
}

#[test]
fn after_ten_days_without_a_fix_the_band_is_fifteen_percent() {
    let (mut env, k, feeder) = feeder_pool(20261102);
    env.warp(NOV_3_AFTERNOON + 10 * DAY);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 230_001, 20261113);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixJump));
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 300_000, 20261113);
    env.expect_err(ix, &feeder, &[], code(PoolError::FixJump));
    // Today's date alone: the days in between are not published.
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 230_000, 20261113);
    env.send(ix, &feeder, &[]);
    assert_eq!((env.pool(&k).fix_rate, env.pool(&k).fix_date), (230_000, 20261113));
}

#[test]
fn a_burst_of_dates_in_one_block_stops_at_the_second_publication() {
    // The FIX in force is dated Oct 31; today is Nov 3. Three publications in one transaction,
    // dated Nov 1, 2 and 3 and each within one day's band of the one before: the second is
    // refused, since the feeder publishes once per Mexico City day.
    let (mut env, k, feeder) = feeder_pool(20261031);
    let ixs = vec![
        env.post_fix_ix(&k, &feeder.pubkey(), 210_000, 20261101),
        env.post_fix_ix(&k, &feeder.pubkey(), 220_500, 20261102),
        env.post_fix_ix(&k, &feeder.pubkey(), 231_525, 20261103),
    ];
    match env.try_send_all(&ixs, &feeder, &[]) {
        Ok(_) => panic!("the second publication of the day"),
        Err(e) => assert_eq!(custom_error_code(&e.err), Some(E_FIX_ALREADY_POSTED_TODAY)),
    }
    assert_eq!((env.pool(&k).fix_rate, env.pool(&k).fix_date), (200_000, 20261031), "nothing landed");
    // Behind by three days, the feeder publishes today's date alone, within three days' band.
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 230_000, 20261103);
    env.send(ix, &feeder, &[]);
    assert_eq!((env.pool(&k).fix_rate, env.pool(&k).fix_date), (230_000, 20261103));
}

#[test]
fn the_feeder_publishes_once_per_mexico_city_day() {
    let (mut env, k, feeder) = feeder_pool(20261102);
    let council = env.council.insecure_clone();
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 201_000, 20261103);
    env.send(ix, &feeder, &[]);
    // A second publication the same day, even a correction of today's FIX, is the council's.
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 201_500, 20261103);
    env.expect_err(ix, &feeder, &[], E_FIX_ALREADY_POSTED_TODAY);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 201_500, 20261103);
    env.send(ix, &council, &[]);
    assert_eq!(env.pool(&k).fix_rate, 201_500);

    // 2026-11-04T05:59:59Z is still Nov 3 in Mexico City; 06:00Z is Nov 4.
    env.warp(1_793_707_200 + 18 * 3_600 - 1);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 202_000, 20261103);
    env.expect_err(ix, &feeder, &[], E_FIX_ALREADY_POSTED_TODAY);
    env.warp(1_793_707_200 + 18 * 3_600);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 202_000, 20261104);
    env.send(ix, &feeder, &[]);

    // A day the council publishes first is spent for the feeder too.
    env.warp(NOV_3_AFTERNOON + 2 * DAY);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 202_500, 20261105);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 202_600, 20261105);
    env.expect_err(ix, &feeder, &[], E_FIX_ALREADY_POSTED_TODAY);
    env.warp(NOV_3_AFTERNOON + 3 * DAY);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 202_600, 20261106);
    env.send(ix, &feeder, &[]);
}

#[test]
fn the_council_publishes_a_fix_at_most_five_days_old() {
    let (mut env, k, _) = setup();
    let council = env.council.insecure_clone();
    // Nov 3: Oct 4 is 30 days old, Oct 28 six, Oct 29 five.
    for date in [20261004u32, 20261028] {
        let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, date);
        env.expect_err(ix, &council, &[], code(PoolError::FixDateInvalid));
    }
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, 20261029);
    env.send(ix, &council, &[]);
}

#[test]
fn the_fix_bounds_hold_for_the_feeder_and_the_council() {
    use elote_pool::events::FixBoundsSet;
    let mut env = Env::new();
    env.warp(NOV_3_AFTERNOON - DAY);
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let multisig = env.multisig;
    for (min, max) in [(250_001u64, 250_000u64), (150_000, 0)] {
        let mut p = env.init_params(BPS, COLLECTION_PERIOD);
        (p.min_fix_rate, p.max_fix_rate) = (min, max);
        let ix = env.init_pool_ix_params(&k, &multisig, p);
        env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);
    }
    let mut p = env.init_params(BPS, COLLECTION_PERIOD);
    (p.min_fix_rate, p.max_fix_rate) = (150_000, 250_000);
    let ix = env.init_pool_ix_params(&k, &multisig, p);
    env.send(ix, &council, &[]);
    assert_eq!((env.pool(&k).min_fix_rate, env.pool(&k).max_fix_rate), (150_000, 250_000));
    let feeder = env.funded(1);
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &feeder.pubkey());
    env.send(ix, &council, &[]);

    for rate in [250_001u64, 149_999] {
        let ix = env.post_fix_ix(&k, &council.pubkey(), rate, 20261102);
        env.expect_err(ix, &council, &[], E_FIX_OUT_OF_BOUNDS);
    }
    let ix = env.post_fix_ix(&k, &council.pubkey(), 240_000, 20261102);
    env.send(ix, &council, &[]);
    env.warp(NOV_3_AFTERNOON);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 250_001, 20261103);
    env.expect_err(ix, &feeder, &[], E_FIX_OUT_OF_BOUNDS);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 250_000, 20261103);
    env.send(ix, &feeder, &[]);

    // The council moves the bounds; only the council, and never min above max.
    let ix = env.set_fix_bounds_ix(&k, &feeder.pubkey(), 100_000, 240_000);
    env.expect_err(ix, &feeder, &[], E_UNAUTHORIZED);
    for (min, max) in [(240_001u64, 240_000u64), (100_000, 0)] {
        let ix = env.set_fix_bounds_ix(&k, &council.pubkey(), min, max);
        env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);
    }
    let ix = env.set_fix_bounds_ix(&k, &council.pubkey(), 100_000, 240_000);
    let ev: FixBoundsSet = find_event(&env.send(ix, &council, &[])).expect("FixBoundsSet");
    assert_eq!((ev.pool, ev.min_fix_rate, ev.max_fix_rate), (k.pool, 100_000, 240_000));
    let ix = env.post_fix_ix(&k, &council.pubkey(), 245_000, 20261103);
    env.expect_err(ix, &council, &[], E_FIX_OUT_OF_BOUNDS);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 239_000, 20261103);
    env.send(ix, &council, &[]);

    // Zero and zero: no bounds.
    let ix = env.set_fix_bounds_ix(&k, &council.pubkey(), 0, 0);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 400_000, 20261103);
    env.send(ix, &council, &[]);
}

#[test]
fn the_feeder_stays_within_two_percent_of_the_market_reference() {
    // FIX in force 20.0000 dated Nov 2; the reference reads 20.0000 (0.05 USD per MXNe).
    let (mut env, k, feeder) = feeder_pool(20261102);
    env.put_reference_rate(200_000, 3_600);
    for (rate, ok) in [(204_001u64, false), (195_999, false), (204_000, true)] {
        let ix = env.post_fix_ix(&k, &feeder.pubkey(), rate, 20261103);
        if ok {
            let ev: FixPosted = find_event(&env.send(ix, &feeder, &[])).expect("FixPosted");
            assert_eq!((ev.fix_rate, ev.reference_rate, ev.reference_at), (204_000, 200_000, NOV_3_AFTERNOON - 3_600));
        } else {
            env.expect_err(ix, &feeder, &[], E_FIX_OFF_REFERENCE);
        }
    }
}

#[test]
fn the_feeder_needs_a_reference_at_most_26_hours_old() {
    let max_age = elote_pool::fx::MAX_REFERENCE_AGE_SECONDS;
    assert_eq!(max_age, 26 * 3_600);
    let (mut env, k, feeder) = feeder_pool(20261102);
    env.put_reference_rate(200_000, max_age + 1);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_000, 20261103);
    env.expect_err(ix, &feeder, &[], E_FX_REFERENCE_STALE);
    env.put_reference_rate(200_000, max_age);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_000, 20261103);
    env.send(ix, &feeder, &[]);
}

#[test]
fn a_reference_with_another_owner_or_layout_is_refused_to_the_feeder() {
    let now_ms = NOV_3_AFTERNOON as u64 * 1_000;
    let mut wrong_feed = reference_data(5_000_000, now_ms);
    wrong_feed[8..12].copy_from_slice(b"MXNt");
    let mut wrong_disc = reference_data(5_000_000, now_ms);
    wrong_disc[0] ^= 1;
    let mut wrong_decimals = reference_data(5_000_000, now_ms);
    wrong_decimals[97] = 6;
    let mut huge = reference_data(5_000_000, now_ms);
    huge[40] = 1;
    for (data, owner) in [
        (reference_data(5_000_000, now_ms), solana_pubkey::Pubkey::new_unique()),
        (reference_data(5_000_000, now_ms), SYSTEM_PROGRAM_ID),
        (wrong_feed, elote_pool::fx::REFERENCE_PROGRAM),
        (wrong_disc, elote_pool::fx::REFERENCE_PROGRAM),
        (wrong_decimals, elote_pool::fx::REFERENCE_PROGRAM),
        (huge, elote_pool::fx::REFERENCE_PROGRAM),
        (reference_data(0, now_ms), elote_pool::fx::REFERENCE_PROGRAM),
        (reference_data(5_000_000, now_ms)[..79].to_vec(), elote_pool::fx::REFERENCE_PROGRAM),
    ] {
        let (mut env, k, feeder) = feeder_pool(20261102);
        env.put_reference(data, owner);
        let ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_000, 20261103);
        env.expect_err(ix, &feeder, &[], E_FX_REFERENCE_INVALID);
    }
}

#[test]
fn the_council_publishes_without_the_reference_check_and_records_the_reference() {
    let (mut env, k, _) = feeder_pool(20261102);
    let council = env.council.insecure_clone();
    // A reference 10% away and two days old: the council's FIX lands and carries it.
    env.put_reference_rate(220_000, 2 * DAY);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 200_000, 20261103);
    let ev: FixPosted = find_event(&env.send(ix, &council, &[])).expect("FixPosted");
    assert_eq!((ev.reference_rate, ev.reference_at), (220_000, NOV_3_AFTERNOON - 2 * DAY));
    // An unreadable reference records zero.
    env.put_reference(vec![7u8; 176], solana_pubkey::Pubkey::new_unique());
    let ix = env.post_fix_ix(&k, &council.pubkey(), 200_100, 20261103);
    let ev: FixPosted = find_event(&env.send(ix, &council, &[])).expect("FixPosted");
    assert_eq!((ev.fix_rate, ev.reference_rate, ev.reference_at), (200_100, 0, 0));
}

#[test]
fn on_localnet_a_missing_reference_skips_the_check() {
    // RedStone's account does not exist on localnet or devnet: the feeder publishes on the band
    // alone and the event records no reference. A mainnet build refuses it (FxReferenceInvalid).
    let (mut env, k, feeder) = feeder_pool(20261102);
    assert!(env.svm.get_account(&elote_pool::fx::REFERENCE_ACCOUNT).is_none());
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 210_000, 20261103);
    let ev: FixPosted = find_event(&env.send(ix, &feeder, &[])).expect("FixPosted");
    assert_eq!((ev.fix_rate, ev.reference_rate, ev.reference_at), (210_000, 0, 0));
}

#[test]
fn post_fix_takes_only_the_reference_account() {
    let (mut env, k, feeder) = feeder_pool(20261102);
    let mut ix = env.post_fix_ix(&k, &feeder.pubkey(), 200_000, 20261103);
    let at = ix.accounts.iter().position(|m| m.pubkey == elote_pool::fx::REFERENCE_ACCOUNT).expect("the reference account");
    ix.accounts[at].pubkey = solana_pubkey::Pubkey::new_unique();
    env.expect_err(ix, &feeder, &[], E_FX_REFERENCE_INVALID);
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
    real_fix_pool_at(env, max_outstanding, 0)
}

/// As `real_fix_pool`, the line at `rate_bps` a year.
fn real_fix_pool_at(env: &mut Env, max_outstanding: u64, rate_bps: u16) -> (PoolKeys, Keypair, u64) {
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let mut p = env.init_params(BPS, COLLECTION_PERIOD);
    p.max_outstanding_principal = max_outstanding;
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), p);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, env.today());
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 5_000 * UNIT);
    env.deposit(&k, &investor, 5_000 * UNIT);
    let borrower = env.borrower(&k);
    let terms = env.line_params(5_000_000, rate_bps); // MXN 50,000.00
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
    assert_eq!((ev.amount_mxn, ev.fix_rate, ev.fix_date, ev.usdc_delivered), (5_000_000, 183_740, env.today(), usdc));
    let record = env.pagare(&k, id, 0);
    assert_eq!((record.amount_mxn, record.fix_rate, record.fix_date, record.usdc_delivered), (5_000_000, 183_740, env.today(), usdc));
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
    assert_eq!(env.pagare(&k, id, 0).fix_rate, 183_750);
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

/// Decision 42: bounds with `min == max` pin the FIX — a peso series on a peso asset (MEXe),
/// where parity is the issuer's promise, not an oracle's reading — and a pinned FIX never goes
/// stale: approvals and draws run on it months later with no re-posting, no feeder and no
/// council cadence. Any post off the pin is refused by the bounds, so the exemption is safe.
#[test]
fn a_pinned_fix_never_goes_stale() {
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let mut p = env.init_params(BPS, COLLECTION_PERIOD);
    p.min_fix_rate = 10_000;
    p.max_fix_rate = 10_000;
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), p);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 10_000, env.today());
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 5_000 * UNIT);
    env.deposit(&k, &investor, 5_000 * UNIT);
    let borrower = env.borrower(&k);
    let terms = env.line_params(300_000, 0); // MXN 3,000.00 at parity
    let (id, _) = env.request_line_with(&k, &borrower, &terms, None);
    env.approve_line(&k, id, &borrower.pubkey(), &terms);

    // A month later, with no re-posting (the harness stops refreshing this pool): the draw and
    // the approval of a fresh request both run on the month-old pinned FIX.
    env.release_fix(&k);
    env.warp(env.now() + 30 * DAY);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 100_000);
    env.send(ix, &borrower, &[]);
    let pagare = env.pagare(&k, id, 0);
    assert_eq!((pagare.fix_rate, pagare.amount_mxn, pagare.usdc_delivered), (10_000, 100_000, 1_000_000_000), "MXN 1,000.00 delivered as exactly 1,000.000000 of the peso asset");
    let (id2, _) = env.request_line_with(&k, &borrower, &terms, None);
    env.approve_line(&k, id2, &borrower.pubkey(), &terms);

    // The pin holds: any other rate is refused, so the stale exemption can never price wrong.
    let ix = env.post_fix_ix(&k, &council.pubkey(), 10_001, env.today());
    env.expect_err(ix, &council, &[], code(PoolError::FixOutOfBounds));
}

/// The crisis path of a pinned peso pool (decision 42): the council pauses, widens the bounds
/// (the pool is no longer pinned, so the month-old FIX counts as stale again), re-prices while
/// paused, and resumes. Draws then run at the new rate only.
#[test]
fn unpinning_restores_staleness_and_the_council_re_prices_under_pause() {
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let mut p = env.init_params(BPS, COLLECTION_PERIOD);
    p.min_fix_rate = 10_000;
    p.max_fix_rate = 10_000;
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), p);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 10_000, env.today());
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 5_000 * UNIT);
    env.deposit(&k, &investor, 5_000 * UNIT);
    let borrower = env.borrower(&k);
    let terms = env.line_params(300_000, 0);
    let (id, _) = env.request_line_with(&k, &borrower, &terms, None);
    env.approve_line(&k, id, &borrower.pubkey(), &terms);
    env.release_fix(&k);
    env.warp(env.now() + 30 * DAY);

    // The depeg response: pause, widen the bounds, re-price, resume — all by council vote.
    env.set_paused(&k, true);
    let ix = env.set_fix_bounds_ix(&k, &council.pubkey(), 9_000, 10_000);
    env.send(ix, &council, &[]);
    // Unpinned, the month-old FIX is stale again: a draw is refused even after resume.
    env.set_paused(&k, false);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 100_000);
    env.expect_err(ix, &borrower, &[], code(PoolError::FixStale));
    // The re-priced FIX goes through the widened bounds and draws run at the new rate.
    let ix = env.post_fix_ix(&k, &council.pubkey(), 9_500, env.today());
    env.send(ix, &council, &[]);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 50_000);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pagare(&k, id, 0).fix_rate, 9_500);
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
    let (k, ix) = env.init_pool_full(&asset, 1_000); // 10% of the NAV per line
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

/// A fully drawn MXN 50,000 line at no interest, drawn at 18.3740.
fn drawn_line(env: &mut Env) -> (PoolKeys, Keypair, u64, u64) {
    let (k, borrower, id) = real_fix_pool(env, u64::MAX);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 5_000_000);
    env.send(ix, &borrower, &[]);
    let delivered = env.pagare(&k, id, 0).usdc_delivered;
    (k, borrower, id, delivered)
}

fn post(env: &mut Env, k: &PoolKeys, fix_rate: u64) {
    let council = env.council.insecure_clone();
    let ix = env.post_fix_ix(k, &council.pubkey(), fix_rate, env.today());
    env.send(ix, &council, &[]);
}

#[test]
fn a_repayment_in_usdc_is_credited_in_mxn_at_the_fix_of_the_day_and_realizes_the_difference() {
    use elote_pool::events::{FxRealized, LineRepaid, NavUpdated, PagarePayment};
    use anchor_lang::Discriminator;
    let mut env = Env::new();
    let (k, borrower, id, delivered) = drawn_line(&mut env);
    let nav_before = env.nav(&k);
    // The peso falls to 20.0000: MXN 50,000 is now 2,500 USDC.
    env.warp(env.now() + DAY);
    post(&mut env, &k, 200_000);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 3_000 * UNIT);
    let before = env.token_balance(&k.asset_ata(&borrower.pubkey()));
    let ix = env.repay_ix(&k, id, &borrower.pubkey(), 3_000 * UNIT);
    let meta = env.send(ix, &borrower, &[]);
    assert_eq!(before - env.token_balance(&k.asset_ata(&borrower.pubkey())), 2_500 * UNIT, "only what MXN 50,000 costs today");

    let line = env.line(&k, id);
    assert_eq!((line.status, line.principal_repaid, line.usdc_cost_repaid), (elote_pool::state::LineStatus::Repaid, 5_000_000, delivered));
    assert_eq!(env.pagare(&k, id, 0).status, elote_pool::state::PAGARE_PAID, "the PAGADO mark at the exact MXN amount");
    let fx: FxRealized = find_event(&meta).expect("FxRealized");
    let repaid: LineRepaid = find_event(&meta).expect("LineRepaid");
    let loss = delivered as i64 - (2_500 * UNIT) as i64;
    assert_eq!((fx.line_id, fx.cost, fx.received, fx.delta), (id, delivered, 2_500 * UNIT, -loss));
    assert_eq!((fx.receipt_hash, repaid.receipt_hash, repaid.usdc_received, repaid.fix_rate), (RECEIPT_HASH, RECEIPT_HASH, 2_500 * UNIT, 200_000));
    assert_eq!((fx.nav_after, repaid.nav_after), (nav_before - loss as u64, nav_before - loss as u64));
    let pool = env.pool(&k);
    assert_eq!((pool.outstanding_principal, pool.fx_realized), (0, -loss));
    assert_eq!(env.nav(&k), nav_before - loss as u64, "the investors carry the peso's fall");

    // One order, for the app to group one payment: PagarePayment, FxRealized, LineRepaid, NavUpdated.
    let order: Vec<Vec<u8>> = event_order(&meta)
        .into_iter()
        .filter(|d| [PagarePayment::DISCRIMINATOR, FxRealized::DISCRIMINATOR, LineRepaid::DISCRIMINATOR, NavUpdated::DISCRIMINATOR].contains(&d.as_slice()))
        .collect();
    assert_eq!(order, vec![PagarePayment::DISCRIMINATOR.to_vec(), FxRealized::DISCRIMINATOR.to_vec(), LineRepaid::DISCRIMINATOR.to_vec(), NavUpdated::DISCRIMINATOR.to_vec()]);
}

#[test]
fn a_stronger_peso_is_a_gain_and_partial_payments_cost_exactly_the_delivery() {
    let mut env = Env::new();
    let (k, borrower, id, delivered) = drawn_line(&mut env);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 4_000 * UNIT);
    let mut received = 0;
    for fix in [170_000u64, 171_000] {
        env.warp(env.now() + DAY);
        post(&mut env, &k, fix);
        let pay = elote_pool::fx::usdc_ceil_for_mxn(2_500_000, fix).unwrap();
        let before = env.token_balance(&k.asset_ata(&borrower.pubkey()));
        let ix = env.repay_ix(&k, id, &borrower.pubkey(), pay);
        env.send(ix, &borrower, &[]);
        received += before - env.token_balance(&k.asset_ata(&borrower.pubkey()));
    }
    let line = env.line(&k, id);
    assert_eq!((line.principal_repaid, line.usdc_cost_repaid), (5_000_000, delivered), "the two halves cost the whole delivery");
    assert_eq!(env.pool(&k).fx_realized, received as i64 - delivered as i64);
    assert!(env.pool(&k).fx_realized > 0);
}

#[test]
fn a_repayment_is_never_refused_for_an_old_fix_but_must_name_the_one_in_force() {
    let mut env = Env::new();
    let (k, borrower, id, _) = drawn_line(&mut env);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    env.warp(env.now() + 30 * DAY);
    let ix = env.repay_ix(&k, id, &borrower.pubkey(), UNIT);
    env.send(ix, &borrower, &[]);
    let stale = env.repay_ix(&k, id, &borrower.pubkey(), UNIT / 2);
    post(&mut env, &k, 190_000);
    env.expect_err(stale, &borrower, &[], code(PoolError::FixMismatch));
}

#[test]
fn a_write_off_removes_the_lines_cost_in_usdc() {
    use elote_pool::events::LineWrittenOff;
    let mut env = Env::new();
    let (k, borrower, id, delivered) = drawn_line(&mut env);
    let term_end = env.line(&k, id).term_end;
    let nav_before = env.nav(&k);
    env.warp(term_end + 1);
    let _ = borrower;
    let council = env.council.insecure_clone();
    let ix = env.write_off_ix(&k, id, &council.pubkey());
    let ev: LineWrittenOff = find_event(&env.send(ix, &council, &[])).expect("LineWrittenOff");
    assert_eq!((ev.loss, ev.loss_usdc, ev.nav_after), (5_000_000, delivered, nav_before - delivered));
    assert_eq!(env.pool(&k).outstanding_principal, 0);
}

/// The principal's USDC is what its centavos cost at the FIX of the day, rounded up; the rounding of
/// the USDC taken stays with the charges. A payment that applies no principal realizes nothing, and
/// every payment that realizes something says so: after any sequence of payments, the `FxRealized`
/// deltas add up to `Pool.fx_realized`.
#[test]
fn fx_realized_events_add_up_to_the_pools_figure() {
    use elote_pool::events::{FxRealized, LineRepaid};
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_pool_at(&mut env, u64::MAX, 1_200);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 5_000_000);
    env.send(ix, &borrower, &[]);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 4_000 * UNIT);
    let mut events = 0i64;
    // Interest only, at an amount whose MXN does not convert back exactly; then interest and
    // principal; then principal at other FIXes; then the payoff.
    for (fix, max_usdc) in [(183_740u64, 1_000_001u64), (183_740, 777_777_777), (191_234, 333_333_333), (176_543, 1_234_567_891), (180_001, 4_000 * UNIT)] {
        if fix != env.pool(&k).fix_rate {
            env.warp(env.now() + DAY);
            post(&mut env, &k, fix);
        }
        let line = env.line(&k, id);
        let interest_left = line.interest_due - line.interest_repaid;
        let ix = env.repay_ix(&k, id, &borrower.pubkey(), max_usdc.min(env.token_balance(&k.asset_ata(&borrower.pubkey()))));
        let meta = env.send(ix, &borrower, &[]);
        let repaid: LineRepaid = find_event(&meta).expect("LineRepaid");
        if repaid.principal_part == 0 {
            assert!(repaid.interest_part > 0 && repaid.interest_part <= interest_left);
        }
        match find_event::<FxRealized>(&meta) {
            Some(fx) => {
                assert_eq!(fx.received, elote_pool::fx::usdc_ceil_for_mxn(repaid.principal_part, fix).unwrap());
                events += fx.delta;
            }
            None => assert_eq!(repaid.principal_part, 0, "a payment that applies principal emits FxRealized"),
        }
        assert_eq!(events, env.pool(&k).fx_realized, "after the payment at {fix}");
    }
    assert_eq!(env.line(&k, id).status, elote_pool::state::LineStatus::Repaid);
}
