//! Credit lines repaid in installments: schedule, arrears, late interest and collection.

mod common;

use common::*;
use elote_pool::events::{
    LateInterestCollected, LineInCollection, LineOpened, LineRepaid, LineScheduled, LineWrittenOff,
};
use elote_pool::state::{LineStatus, Schedule, ScheduleParams};
use solana_keypair::Keypair;
use solana_signer::Signer;

/// A pool with `grace` seconds of grace and one investor who deposited `assets`.
fn pool_with(env: &mut Env, assets: u64, grace: u32) -> PoolKeys {
    let (k, _) = env.setup_pool_with_grace(grace);
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

fn open(env: &mut Env, k: &PoolKeys, borrower: &Keypair, p: &LineParams) -> u64 {
    env.open_line_with(k, borrower, p, None).0
}

#[test]
fn open_scheduled_line_records_the_schedule() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);

    // 6.0 at 12%: 6.72 in six monthly installments, 50% a year late interest.
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, None);
    let ev: LineOpened = find_event(&meta).expect("LineOpened");
    assert_eq!((ev.line_id, ev.principal, ev.interest_due), (line_id, 6 * UNIT, 720_000));
    let ev: LineScheduled = find_event(&meta).expect("LineScheduled");
    assert_eq!(
        (ev.pool, ev.line_id, ev.interval_seconds, ev.installments, ev.late_bps, ev.max_missed),
        (k.pool, line_id, MONTH, 6, 5_000, 3)
    );

    let line = env.line(&k, line_id);
    assert_eq!(line.version, 2, "scheduled lines are version 2");
    let s = line.schedule().expect("schedule");
    assert_eq!(
        s,
        Schedule {
            interval_seconds: MONTH,
            installments: 6,
            late_bps: 5_000,
            max_missed: 3,
            flags: 0,
            late_collected: 0,
            late_paid_until: 0,
        }
    );
    // The byte layout of `reserved`.
    let r = line.reserved;
    assert_eq!(&r[0..8], &MONTH.to_le_bytes());
    assert_eq!(&r[8..10], &6u16.to_le_bytes());
    assert_eq!(&r[10..12], &5_000u16.to_le_bytes());
    assert_eq!((r[12], r[13]), (3, 0));
    assert_eq!(&r[14..32], &[0u8; 18]);

    // A line without a schedule keeps version 1, zero reserved bytes and no LineScheduled event.
    let p1 = env.line_params(UNIT, 1_000);
    let (v1, meta) = env.open_line_with(&k, &borrower, &p1, None);
    assert!(find_event::<LineScheduled>(&meta).is_none());
    let line = env.line(&k, v1);
    assert_eq!(line.version, 1);
    assert_eq!(line.reserved, [0u8; 32]);
    assert_eq!(line.schedule(), None);
}

#[test]
fn open_rejects_invalid_schedules() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let admin = env.admin.insecure_clone();
    let good = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let with = |f: &dyn Fn(&mut ScheduleParams)| {
        let mut p = good;
        let mut s = p.schedule.unwrap();
        f(&mut s);
        p.schedule = Some(s);
        p
    };

    let cases: Vec<(&str, LineParams, u32)> = vec![
        ("zero installments", with(&|s| s.installments = 0), E_INVALID_SCHEDULE),
        ("above the maximum", with(&|s| s.installments = 121), E_INVALID_SCHEDULE),
        ("zero interval", with(&|s| s.interval_seconds = 0), E_INVALID_SCHEDULE),
        ("negative interval", with(&|s| s.interval_seconds = -MONTH), E_INVALID_SCHEDULE),
        ("late rate above 100%", with(&|s| s.late_bps = 10_001), E_INVALID_SCHEDULE),
        ("zero max missed", with(&|s| s.max_missed = 0), E_INVALID_SCHEDULE),
        ("max missed below the three-month policy", with(&|s| s.max_missed = 2), E_INVALID_SCHEDULE),
        ("max missed above installments", with(&|s| s.max_missed = 7), E_INVALID_SCHEDULE),
        ("one second past the term", with(&|s| s.interval_seconds = MONTH + 1), E_SCHEDULE_ENDS_AFTER_TERM),
        ("span overflow", with(&|s| s.interval_seconds = i64::MAX), E_OVERFLOW),
    ];
    for (name, p, code) in cases {
        let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
        match env.try_send(ix, &admin, &[&borrower]) {
            Ok(_) => panic!("{name}: accepted"),
            Err(e) => assert_eq!(custom_error_code(&e.err), Some(code), "{name}"),
        }
    }

    // Installments smaller than one unit: 3 base units in 4 installments.
    let mut p = with(&|s| s.installments = 4);
    p.principal = 3;
    p.rate_bps = 0;
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_INVALID_SCHEDULE);
    assert_eq!(env.pool(&k).line_count, 0);

    // The edges are accepted: three installments with max_missed == installments, 100% late
    // rate, last due date exactly at the term.
    let p = with(&|s| {
        s.installments = 3;
        s.max_missed = 3;
        s.late_bps = 10_000;
        s.interval_seconds = 2 * MONTH;
    });
    open(&mut env, &k, &borrower, &p);
    let p = with(&|s| s.max_missed = 6);
    open(&mut env, &k, &borrower, &p);
    assert_eq!(env.pool(&k).line_count, 2);
}

#[test]
fn draw_rejects_a_schedule_that_would_end_after_the_term() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    // The term ends exactly at the last due date if drawn now.
    let p = env.scheduled_params(UNIT, 1_200, 6, 5_000);
    let late = open(&mut env, &k, &borrower, &p);
    let on_time = open(&mut env, &k, &borrower, &p);

    env.draw(&k, on_time, &borrower);
    env.warp(env.now() + 1);
    let ix = env.draw_ix(&k, late, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_SCHEDULE_ENDS_AFTER_TERM);
    assert_eq!(env.line(&k, late).status, LineStatus::Approved);
}

// -----------------------------------------------------------------------------------------------
// Arrears, late interest and collection
// -----------------------------------------------------------------------------------------------

/// A drawn scheduled line, with `extra` more of the asset in the borrower's account.
fn drawn(env: &mut Env, k: &PoolKeys, p: &LineParams, extra: u64) -> (Keypair, u64, i64) {
    let borrower = env.borrower(k);
    let line_id = open(env, k, &borrower, p);
    env.draw(k, line_id, &borrower);
    if extra > 0 {
        let admin = env.admin.insecure_clone();
        env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), extra);
    }
    let drawn_at = env.line(k, line_id).drawn_at;
    (borrower, line_id, drawn_at)
}

fn borrower_balance(env: &Env, k: &PoolKeys, b: &Keypair) -> u64 {
    env.token_balance(&ata(&b.pubkey(), &k.asset_mint))
}

#[test]
fn installments_paid_on_their_due_dates_add_up_and_close_the_line() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    // 1_000_003 at 0%: five installments of 166_667 and a last one of 166_668.
    let p = env.scheduled_params(1_000_003, 0, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 0);
    for i in 1..=6u16 {
        env.warp(t0 + i64::from(i) * MONTH);
        let a = env.arrears(&k, line_id);
        assert_eq!((a.due, a.missed, a.late_due), (i, 1, 0), "due at its date, no late interest yet");
        let amount = if i == 6 { 166_668 } else { 166_667 };
        let ix = env.repay_ix(&k, line_id, &b.pubkey(), amount);
        let meta = env.send(ix, &b, &[]);
        assert!(find_event::<LateInterestCollected>(&meta).is_none());
        assert_eq!(env.arrears(&k, line_id).missed, 0);
    }
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!(line.principal_repaid, 1_000_003);
    assert_eq!(line.schedule().unwrap().late_collected, 0);
    assert_eq!(borrower_balance(&env, &k, &b), 0);
}

#[test]
fn missed_installments_grow_over_time() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 0);
    let (_, line_id, t0) = drawn(&mut env, &k, &p, 0);
    let mut seen = vec![];
    for t in [t0, t0 + MONTH - 1, t0 + MONTH, t0 + 2 * MONTH + 5, t0 + 3 * MONTH, t0 + 10 * MONTH] {
        env.warp(t);
        seen.push(env.arrears(&k, line_id).missed);
    }
    assert_eq!(seen, vec![0, 0, 1, 2, 3, 6]);
}

#[test]
fn late_interest_accrues_after_grace_and_is_collected_first() {
    let mut env = Env::new();
    let grace = 5 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    // 6.0 at 12%: 6.72 in six installments of 1.12; 100% a year late interest.
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    let nav0 = env.nav(&k);

    // Within the grace period the installment is missed but accrues nothing.
    env.warp(t0 + MONTH + i64::from(grace));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue, a.late_due), (1, 1_120_000, 0));

    // 1% of a year after grace: 1.12 * 100% * 0.01 = 0.0112.
    let year = elote_pool::state::SECONDS_PER_YEAR as i64;
    env.warp(t0 + MONTH + i64::from(grace) + year / 100);
    assert_eq!(env.arrears(&k, line_id).late_due, 11_200);

    // A repayment must cover the late interest.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 11_199);
    env.expect_err(ix, &b, &[], E_LATE_INTEREST_UNPAID);

    // Late interest, then the whole interest, then principal.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1_120_000 + 11_200);
    let meta = env.send(ix, &b, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("LateInterestCollected");
    assert_eq!((ev.pool, ev.line_id, ev.amount), (k.pool, line_id, 11_200));
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (720_000, 400_000, 5_600_000));
    assert_eq!(ev.nav_after, nav0 + 11_200 + 720_000, "late interest raises the NAV like interest");
    assert_eq!(env.nav(&k), nav0 + 11_200 + 720_000);
    let pool = env.pool(&k);
    assert_eq!(pool.interest_collected, 720_000, "late interest is not ordinary interest");
    assert_eq!(pool.outstanding_principal, 5_600_000);
    let s = env.line(&k, line_id).schedule().unwrap();
    assert_eq!((s.late_collected, s.late_paid_until), (11_200, env.now()));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.late_due), (0, 0));

    // The settled period is never charged again: a day later nothing is overdue or late.
    env.warp(env.now() + 24 * 3600);
    let ev = env.repay(&k, line_id, &b, 1);
    assert_eq!((ev.interest_part, ev.principal_part), (0, 1));
    assert_eq!(env.line(&k, line_id).schedule().unwrap().late_collected, 11_200);
}

#[test]
fn late_interest_is_capped_and_overpayment_stays_with_the_payer() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(UNIT, 1_000, 3, 10_000);
    let total = 1_100_000;
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 5 * UNIT);

    // Twenty years late at 100% a year: each part stops at its own amount (1.1 overdue in all),
    // and the lifetime cap holds late interest to the principal, 1.0.
    env.warp(t0 + 3 * MONTH + 20 * elote_pool::state::SECONDS_PER_YEAR as i64);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.overdue, a.late_due), (total, UNIT));

    // Paying far more than is due debits only what is due.
    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 6 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).unwrap().amount, UNIT);
    let ev: LineRepaid = find_event(&meta).unwrap();
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, UNIT, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), UNIT + total);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!(line.schedule().unwrap().late_collected, UNIT);
}

#[test]
fn repayment_order_scheduled_is_interest_first_single_maturity_is_principal_first() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p2 = env.scheduled_params(4 * UNIT, 1_000, 4, 10_000);
    let (b, v2, _) = drawn(&mut env, &k, &p2, UNIT);
    let p1 = env.line_params(4 * UNIT, 1_000);
    let v1 = open(&mut env, &k, &b, &p1);
    env.draw(&k, v1, &b);

    let ev = env.repay(&k, v2, &b, 500_000);
    assert_eq!((ev.interest_part, ev.principal_part), (400_000, 100_000));
    let ev = env.repay(&k, v1, &b, 500_000);
    assert_eq!((ev.interest_part, ev.principal_part), (0, 500_000));

    // A single-maturity line still rejects more than remains; a scheduled one takes what is due.
    let ix = env.repay_ix(&k, v1, &b.pubkey(), 3_900_001);
    env.expect_err(ix, &b, &[], E_WRONG_AMOUNT);
    let before = borrower_balance(&env, &k, &b);
    let ev = env.repay(&k, v2, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_900_000, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), 3_900_000);
    assert_eq!(env.line(&k, v2).status, LineStatus::Repaid);
}

#[test]
fn mark_in_collection_after_three_missed_installments() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (_, line_id, t0) = drawn(&mut env, &k, &p, 0);
    let keeper = env.funded(1);

    env.warp(t0 + 3 * MONTH - 1);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
    // At the third due date it is missed, but collection waits until that second has passed.
    env.warp(t0 + 3 * MONTH);
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);

    // Anyone may call it after that, also while the pool is paused: it only records facts.
    env.warp(t0 + 3 * MONTH + 1);
    env.set_paused(&k, true);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let meta = env.send(ix, &keeper, &[]);
    env.set_paused(&k, false);
    let ev: LineInCollection = find_event(&meta).expect("LineInCollection");
    assert_eq!((ev.pool, ev.line_id, ev.missed, ev.remaining), (k.pool, line_id, 3, 6_720_000));
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_LINE_STATUS);

    // A single-maturity line has no installments to miss.
    let b = env.borrower(&k);
    let p1 = env.line_params(UNIT, 1_000);
    let v1 = open(&mut env, &k, &b, &p1);
    env.draw(&k, v1, &b);
    env.warp(env.now() + 40 * 24 * 3600);
    let ix = env.mark_in_collection_ix(&k, v1);
    env.expect_err(ix, &keeper, &[], E_NOT_SCHEDULED);
}

#[test]
fn a_cure_confirmed_first_wins_over_collection() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 0);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    env.warp(t0 + 3 * MONTH);
    env.repay(&k, line_id, &b, 1_120_000);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Active);
}

#[test]
fn a_line_in_collection_still_accepts_repayments_and_closes_when_repaid() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + 3 * MONTH + 10);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    // The borrower pays the arrears with their late interest: the line stays in collection.
    let a = env.arrears(&k, line_id);
    assert!(a.late_due > 0);
    let ev = env.repay(&k, line_id, &b, a.overdue + a.late_due);
    assert_eq!(ev.remaining, 6_720_000 - a.overdue);
    assert_eq!(env.arrears(&k, line_id).missed, 0);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // Paying the rest closes it.
    let ev = env.repay(&k, line_id, &b, 6_720_000);
    assert_eq!(ev.remaining, 0);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.pool(&k).outstanding_principal, 0);
}

#[test]
fn late_interest_on_the_maximum_installments_stays_within_the_compute_budget() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let day = 24 * 3600;
    let mut p = env.scheduled_params(UNIT, 0, 120, 10_000);
    p.schedule.as_mut().unwrap().interval_seconds = day;
    p.term_end = env.now() + 120 * day;
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    env.warp(t0 + 200 * day);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue), (120, UNIT));
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 2 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).unwrap().amount, a.late_due);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert!(meta.compute_units_consumed < 100_000, "repay used {} CU", meta.compute_units_consumed);
}

// -----------------------------------------------------------------------------------------------
// Write-off policy
// -----------------------------------------------------------------------------------------------

#[test]
fn write_off_in_collection_only_after_the_oldest_missed_due_date_plus_grace() {
    let mut env = Env::new();
    let grace: u32 = 15 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let admin = env.admin.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    let nav0 = env.nav(&k);

    // An active scheduled line before its term is not written off, even with missed installments.
    env.warp(t0 + 3 * MONTH);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_TERM_NOT_ENDED);

    // Collection once the third missed installment's grace has run. The oldest missed installment
    // fell due at t0 + MONTH, so its grace has run too and the admin may write off now.
    env.warp(t0 + 3 * MONTH + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    let ix = env.write_off_ix(&k, line_id, &b.pubkey());
    env.expect_err(ix, &b, &[], E_UNAUTHORIZED);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    let meta = env.send(ix, &admin, &[]);
    let ev: LineWrittenOff = find_event(&meta).expect("LineWrittenOff");
    assert_eq!((ev.line_id, ev.loss, ev.nav_after), (line_id, 6 * UNIT, nav0 - 6 * UNIT));
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    assert_eq!(env.pool(&k).written_off, 6 * UNIT);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), UNIT);
    env.expect_err(ix, &b, &[], E_LINE_STATUS);
}

#[test]
fn collection_waits_for_the_grace_of_the_third_missed_installment() {
    let mut env = Env::new();
    let grace: u32 = 10 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let admin = env.admin.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);

    // One installment paid: the next three fall due at t0 + 2, 3 and 4 months.
    env.warp(t0 + MONTH);
    env.repay(&k, line_id, &b, 1_120_000);
    env.warp(t0 + 4 * MONTH + i64::from(grace));
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &b, &[], E_NOT_ENOUGH_MISSED);
    env.warp(t0 + 4 * MONTH + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // The oldest missed installment (t0 + 2 months) is past its grace: the write-off is allowed.
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

#[test]
fn a_cured_line_in_collection_is_not_written_off_until_it_misses_again() {
    let mut env = Env::new();
    let grace: u32 = 10 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let admin = env.admin.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + 3 * MONTH + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    // The borrower pays all arrears and late interest: nothing is missed.
    let a = env.arrears(&k, line_id);
    env.repay(&k, line_id, &b, a.overdue + a.late_due);
    assert_eq!(env.arrears(&k, line_id).missed, 0);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_WRITE_OFF_TOO_EARLY);

    // The fourth installment falls due at t0 + 4 months and is missed: grace runs 10 more days.
    env.warp(t0 + 4 * MONTH + i64::from(grace));
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(t0 + 4 * MONTH + i64::from(grace) + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

#[test]
fn write_off_after_the_term_waits_for_the_pool_grace() {
    let mut env = Env::new();
    let grace: u32 = 7 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let admin = env.admin.insecure_clone();
    let b = env.borrower(&k);

    // A single-maturity line in a pool with grace.
    let p1 = env.line_params(4 * UNIT, 1_000);
    let v1 = open(&mut env, &k, &b, &p1);
    env.draw(&k, v1, &b);
    // A scheduled line that never reaches collection (max_missed 6 of 6).
    let mut p2 = env.scheduled_params(UNIT, 1_000, 6, 10_000);
    p2.schedule.as_mut().unwrap().max_missed = 6;
    p2.term_end = p1.term_end.max(p2.term_end);
    let v2 = open(&mut env, &k, &b, &p2);
    env.draw(&k, v2, &b);

    for (line_id, term_end) in [(v1, p1.term_end), (v2, p2.term_end)] {
        env.warp(term_end + i64::from(grace));
        let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
        env.expect_err(ix, &admin, &[], E_TERM_NOT_ENDED);
        env.warp(term_end + i64::from(grace) + 1);
        let ix = env.write_off_ix(&k, line_id, &b.pubkey());
        env.expect_err(ix, &b, &[], E_UNAUTHORIZED);
        let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
        env.send(ix, &admin, &[]);
        assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    }
}

#[test]
fn late_interest_over_the_life_of_a_line_never_exceeds_its_principal() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    // 1.0 at 10%: 1.1 in three installments; 50% a year late interest.
    let p = env.scheduled_params(UNIT, 1_000, 3, 5_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 3 * UNIT);
    env.warp(t0 + 3 * MONTH);

    // The borrower pays only the late interest, once a year: the arrears never shrink, the
    // watermark moves, and each settlement charges up to the overdue amount again.
    let year = elote_pool::state::SECONDS_PER_YEAR as i64;
    let mut settlements = 0;
    loop {
        env.warp(env.now() + year);
        let a = env.arrears(&k, line_id);
        if a.late_due == 0 {
            break;
        }
        let ev = env.repay(&k, line_id, &b, a.late_due);
        assert_eq!((ev.interest_part, ev.principal_part), (0, 0), "only late interest");
        settlements += 1;
        assert!(env.line(&k, line_id).schedule().unwrap().late_collected <= UNIT);
    }
    // About 0.55 a year on 1.1 overdue: the first year is collected in full, the second only up
    // to the principal (1.0 in all), and nothing after that.
    assert_eq!(settlements, 2);
    assert_eq!(env.line(&k, line_id).schedule().unwrap().late_collected, UNIT);
    assert_eq!(env.arrears(&k, line_id).missed, 3);

    // From then on a payment goes to interest and principal.
    let ev = env.repay(&k, line_id, &b, 1);
    assert_eq!(ev.interest_part, 1);
}

#[test]
fn late_interest_caps_each_overdue_part_separately() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let year = elote_pool::state::SECONDS_PER_YEAR as i64;
    let part: u64 = 3 * UNIT;
    // 9.0 at 0% in three installments every two years; 33.33% a year late interest.
    let mut p = env.scheduled_params(3 * part, 0, 3, 3_333);
    p.schedule.as_mut().unwrap().interval_seconds = 2 * year;
    p.term_end = env.now() + 6 * year;
    let pool_cash = 20 * UNIT;
    let a = env.investor(&k, pool_cash);
    env.deposit(&k, &a, pool_cash);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 3 * UNIT);

    // 5.5 years in: part 1 is 3.5 years overdue, part 2 1.5 years. At 33.33% the per-part cap is
    // reached after ceil(10_000 * YEAR / 3_333) seconds, about 3.0003 years: part 1 is capped at
    // its own amount, part 2 is not.
    env.warp(t0 + 5 * year + year / 2);
    let got = env.arrears(&k, line_id);
    assert_eq!((got.missed, got.overdue), (2, 2 * part));
    let y = year as u128;
    let cap = (10_000 * y).div_ceil(3_333);
    let s1 = (3 * y + y / 2).min(cap);
    let s2 = y + y / 2;
    let expected = ((part as u128 * (s1 + s2)) * 3_333 / (10_000 * y)) as u64;
    assert_eq!(got.late_due, expected);
    // Distinguishable: without the per-part cap it would be larger, and the overall caps
    // (overdue 6.0, lifetime 9.0) do not bind.
    let uncapped = ((part as u128 * (3 * y + y / 2 + s2)) * 3_333 / (10_000 * y)) as u64;
    assert!(expected < uncapped && uncapped < 2 * part);
    let part1_alone = (part as u128 * s1 * 3_333 / (10_000 * y)) as u64;
    assert!(part1_alone >= part - 1 && part1_alone <= part, "part 1 is at its own cap");

    let ix = env.repay_ix(&k, line_id, &b.pubkey(), expected);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).unwrap().amount, expected);
}
