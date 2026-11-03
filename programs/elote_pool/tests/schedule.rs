//! Credit lines repaid in installments: schedule, arrears, late interest and collection.

mod common;

use common::*;
use elote_pool::events::{
    LateInterestCollected, LineInCollection, LineRequested, LineRepaid,
    LineWrittenOff,
};
use elote_pool::state::{LineStatus, Schedule, ScheduleParams};
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

    // 6.0 at 12% a year, six installments spanning a year: 6.72 once drawn in full at once; 50%
    // a year late interest. Nothing is owed at the opening (interest is fixed per tranche).
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, None);
    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    assert_eq!((ev.line_id, ev.principal), (line_id, 6 * UNIT));
    assert_eq!(
        (ev.line_id, ev.first_due_at, ev.expediente_root),
        (line_id, p.first_due_at(), EXPEDIENTE_ROOT)
    );
    assert_eq!((ev.sponsor, ev.sponsor_role), (None, 0));
    assert_eq!(
        (ev.pool, ev.line_id, ev.interval_seconds, ev.installments, ev.late_bps, ev.max_missed),
        (k.pool, line_id, p.interval(), 6, 5_000, 3)
    );

    let line = env.line(&k, line_id);
    assert_eq!((line.first_due_at, line.availability_end), (p.first_due_at(), p.first_due_at()));
    assert_eq!((line.drawn, line.draws, line.interest_due), (0, 0, 0));
    assert_eq!(line.anchor(), Some(p.first_due_at() - p.interval()));
    assert_eq!(line.last_due(), Some(p.first_due_at() + 5 * p.interval()));
    let s = line.schedule().expect("schedule");
    assert_eq!(
        s,
        Schedule {
            interval_seconds: p.interval(),
            installments: 6,
            late_bps: 5_000,
            max_missed: 3,
        }
    );
    assert_eq!((line.late_collected, line.late_paid_until, line.pagares_paid), (0, 0, 0));

    // A bullet line: no installments, no schedule, its draw window open
    // until the term.
    let p1 = env.line_params(UNIT, 1_000);
    let (v1, meta) = env.open_line_with(&k, &borrower, &p1, None);
    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    assert_eq!((ev.installments, ev.interval_seconds), (0, 0));
    let line = env.line(&k, v1);
    assert_eq!((line.installments, line.interval_seconds, line.late_bps, line.max_missed), (0, 0, 0, 0));
    assert_eq!(line.schedule(), None);
    assert_eq!((line.first_due_at, line.availability_end), (0, p1.term_end));
    assert_eq!((line.anchor(), line.last_due()), (None, Some(p1.term_end)));
}

#[test]
fn open_rejects_invalid_schedules() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
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
        ("negative interval", with(&|s| s.interval_seconds = -good.interval()), E_INVALID_SCHEDULE),
        ("late rate above 100%", with(&|s| s.late_bps = 10_001), E_INVALID_SCHEDULE),
        ("zero max missed", with(&|s| s.max_missed = 0), E_MAX_MISSED_TOO_LOW),
        ("max missed below the three-month policy", with(&|s| s.max_missed = 2), E_MAX_MISSED_TOO_LOW),
        ("max missed above installments", with(&|s| s.max_missed = 7), E_INVALID_SCHEDULE),
        ("one second past the term", with(&|s| s.interval_seconds = good.interval() + 1), E_SCHEDULE_ENDS_AFTER_TERM),
        ("first due date not after the opening", with(&|s| s.first_due_at = good.first_due_at() - good.interval()), E_INVALID_SCHEDULE),
        ("span overflow", with(&|s| s.interval_seconds = i64::MAX), E_OVERFLOW),
    ];
    for (name, p, code) in cases {
        let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
        match env.try_send(ix, &council, &[&borrower]) {
            Ok(_) => panic!("{name}: accepted"),
            Err(e) => assert_eq!(custom_error_code(&e.err), Some(code), "{name}"),
        }
    }

    // Installments smaller than one unit: 3 base units in 4 installments.
    let mut p = with(&|s| s.installments = 4);
    p.principal = 3;
    p.rate_bps = 0;
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &[&borrower], E_INVALID_SCHEDULE);
    assert_eq!(env.pool(&k).line_count, 0);

    // The edges are accepted: three installments with max_missed == installments, 100% late
    // rate, a wider interval whose last due date stays within the term; and the last due date
    // exactly at the term (six installments, first due one interval in, last at the term).
    let p = with(&|s| {
        s.installments = 3;
        s.max_missed = 3;
        s.late_bps = 10_000;
        s.interval_seconds = 2 * good.interval();
    });
    open(&mut env, &k, &borrower, &p);
    let p = with(&|s| s.max_missed = 6);
    assert_eq!(p.first_due_at() + 5 * p.interval(), p.term_end, "last due date at the term");
    open(&mut env, &k, &borrower, &p);
    assert_eq!(env.pool(&k).line_count, 2);
}

/// The draw window closes at the first due date, strictly, because an
/// installment counts as due at the second it falls due. The dates are fixed by the contract, so
/// nothing about them is re-checked at the draw.
#[test]
fn the_draw_window_closes_at_the_first_due_date_exactly() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    // One second before the first due date a tranche is accepted.
    env.warp(p.first_due_at() - 1);
    let ev = env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!((ev.drawn, ev.available, ev.availability_end), (3 * UNIT, 3 * UNIT, p.first_due_at()));
    // Interest runs from the draw to the last due date: 3.0 at 12% for 5 intervals plus one second.
    let seconds = (p.first_due_at() + 5 * p.interval() - (p.first_due_at() - 1)) as u128;
    assert_eq!(ev.interest_due as u128, 3_000_000u128 * 1_200 * seconds / (10_000 * YEAR as u128));

    // At the first due date the window is closed, whatever is still available.
    env.warp(p.first_due_at());
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err_with_attestor(ix, &borrower, E_AVAILABILITY_ENDED);
    let line = env.line(&k, line_id);
    assert_eq!((line.drawn, line.status), (3 * UNIT, LineStatus::Active));
    assert_eq!(env.arrears(&k, line_id).due, 1, "and the first installment is due");
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
        let council = env.council.insecure_clone();
        env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), extra);
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
        env.warp(t0 + i64::from(i) * p.interval());
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
    assert_eq!(line.late_collected, 0);
    assert_eq!(borrower_balance(&env, &k, &b), 0);
}

#[test]
fn missed_installments_grow_over_time() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 0);
    let (_, line_id, t0) = drawn(&mut env, &k, &p, 0);
    let mut seen = vec![];
    for t in [t0, t0 + p.interval() - 1, t0 + p.interval(), t0 + 2 * p.interval() + 5, t0 + 3 * p.interval(), t0 + 10 * p.interval()] {
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
    env.warp(t0 + p.interval() + i64::from(grace));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue, a.late_due), (1, 1_120_000, 0));

    // 1% of a year after grace, on the principal share of the 1.12 installment (1.0;
    // no interest on overdue interest): 1.0 * 100% * 0.01 = 0.01.
    let year = elote_pool::state::SECONDS_PER_YEAR as i64;
    env.warp(t0 + p.interval() + i64::from(grace) + year / 100);
    assert_eq!(env.arrears(&k, line_id).overdue_principal, UNIT);
    assert_eq!(env.arrears(&k, line_id).late_due, 10_000);

    // A repayment must cover the late interest.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 9_999);
    env.expect_err(ix, &b, &[], E_LATE_INTEREST_UNPAID);

    // Late interest, then the whole interest, then principal.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1_120_000 + 10_000);
    let meta = env.send(ix, &b, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("LateInterestCollected");
    assert_eq!((ev.pool, ev.line_id, ev.amount), (k.pool, line_id, 10_000));
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (720_000, 400_000, 5_600_000));
    assert_eq!(ev.nav_after, nav0 + 10_000 + 720_000, "late interest raises the NAV like interest");
    assert_eq!(env.nav(&k), nav0 + 10_000 + 720_000);
    let pool = env.pool(&k);
    assert_eq!(pool.interest_collected, 720_000, "late interest is not ordinary interest");
    assert_eq!(pool.outstanding_principal, 5_600_000);
    let l = env.line(&k, line_id);
    assert_eq!((l.late_collected, l.late_paid_until), (10_000, env.now()));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.late_due), (0, 0));

    // The settled period is never charged again: a day later nothing is overdue or late.
    env.warp(env.now() + 24 * 3600);
    let ev = env.repay(&k, line_id, &b, 1);
    assert_eq!((ev.interest_part, ev.principal_part), (0, 1));
    assert_eq!(env.line(&k, line_id).late_collected, 10_000);
}

#[test]
fn late_interest_is_capped_and_overpayment_stays_with_the_payer() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(UNIT, 1_000, 3, 10_000);
    let total = 1_100_000;
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 5 * UNIT);

    // Twenty years late at 100% a year: each part stops at its own principal share (1.1 overdue
    // in all, of which 0.999998 is principal: three shares each rounded down), which binds
    // before the lifetime cap of 1.0.
    let late = 999_998;
    env.warp(t0 + 3 * p.interval() + 20 * elote_pool::state::SECONDS_PER_YEAR as i64);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.overdue, a.overdue_principal, a.late_due), (total, late, late));

    // Paying far more than is due debits only what is due.
    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 6 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).unwrap().amount, late);
    let ev: LineRepaid = find_event(&meta).unwrap();
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, UNIT, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), late + total);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!(line.late_collected, late);
}

/// One repayment order for every line: late interest, then interest, then principal, and never
/// more than is due.
#[test]
fn repayment_order_is_interest_first_and_clamps_overpayment_on_every_line() {
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
    assert_eq!((ev.interest_part, ev.principal_part), (400_000, 100_000), "a bullet line too");

    // Both take what is due and leave the surplus with the payer.
    let before = borrower_balance(&env, &k, &b);
    let ev = env.repay(&k, v1, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_900_000, 0));
    let ev = env.repay(&k, v2, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_900_000, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), 2 * 3_900_000);
    assert_eq!(env.line(&k, v1).status, LineStatus::Repaid);
    assert_eq!(env.line(&k, v2).status, LineStatus::Repaid);
}

#[test]
fn mark_in_collection_after_three_missed_installments() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (_, line_id, t0) = drawn(&mut env, &k, &p, 0);
    let keeper = env.funded(1);

    env.warp(t0 + 3 * p.interval() - 1);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
    // At the third due date it is missed, but collection waits until that second has passed.
    env.warp(t0 + 3 * p.interval());
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);

    // Anyone may call it after that, also while the pool is paused: it only records facts.
    env.warp(t0 + 3 * p.interval() + 1);
    env.set_paused(&k, true);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let meta = env.send(ix, &keeper, &[]);
    env.set_paused(&k, false);
    let ev: LineInCollection = find_event(&meta).expect("LineInCollection");
    assert_eq!((ev.pool, ev.line_id, ev.missed, ev.remaining, ev.interest_forgiven), (k.pool, line_id, 3, 6_360_000, 360_000));
    assert_eq!(env.line(&k, line_id).interest_due, 360_000, "the interest of the three installments not yet due is forgiven");
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
    env.warp(t0 + 3 * p.interval());
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
    env.warp(t0 + 3 * p.interval() + 10);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    // The mark accelerated the balance and forgave the interest of the three installments not yet
    // due: 6.36 is overdue. The borrower pays half of it with the late interest: the line stays in
    // collection with the other half still missed.
    let a = env.arrears(&k, line_id);
    assert!(a.late_due > 0);
    assert_eq!(a.overdue, 6_360_000);
    let ev = env.repay(&k, line_id, &b, 3_180_000 + a.late_due);
    assert_eq!(ev.remaining, 3_180_000);
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // Paying the rest closes it.
    let ev = env.repay(&k, line_id, &b, 6_360_000);
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
    p.schedule.as_mut().unwrap().first_due_at = env.now() + day;
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

/// A line in collection is written off only once the pool's collection period has run from the
/// mark (audit F-03): the grace of the third missed installment implies the grace of the first, so
/// without the period the write-off was available the instant the keeper marked the line.
#[test]
fn write_off_in_collection_waits_for_the_collection_period_from_the_mark() {
    let mut env = Env::new();
    let grace: u32 = 15 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    let nav0 = env.nav(&k);

    // An active scheduled line before its term is not written off, even with missed installments.
    env.warp(t0 + 3 * p.interval());
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);

    // Collection once the third missed installment's grace has run. The oldest missed installment
    // fell due at t0 + p.interval(), so its grace has run too; the write-off still waits for the
    // pool's collection period, counted from the mark.
    env.warp(t0 + 3 * p.interval() + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    let marked_at = env.now();
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD));
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &b.pubkey());
    env.expect_err(ix, &b, &[], E_UNAUTHORIZED);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
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
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);

    // One installment paid: the next three fall due at t0 + 2, 3 and 4 months.
    env.warp(t0 + p.interval());
    env.repay(&k, line_id, &b, 1_120_000);
    env.warp(t0 + 4 * p.interval() + i64::from(grace));
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &b, &[], E_NOT_ENOUGH_MISSED);
    env.warp(t0 + 4 * p.interval() + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // The oldest missed installment (t0 + 2 months) is past its grace; the write-off is allowed
    // once the collection period has run from the mark.
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

/// After the mark there is no cure: the installments not yet due became exigible at the mark, so
/// paying the pre-mark arrears leaves them missed, and the write-off waits for the grace and for
/// the pool's collection period, both counted from the mark.
#[test]
fn after_the_mark_there_is_no_cure_and_the_write_off_waits_for_the_collection_period_from_the_mark() {
    let mut env = Env::new();
    let grace: u32 = 10 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + 3 * p.interval() + i64::from(grace) + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    let marked_at = env.now();

    // The borrower pays the three installments that were missed before the mark, with the late
    // interest: the other three are still missed, exigible since the mark.
    let a = env.arrears(&k, line_id);
    env.repay(&k, line_id, &b, 3_180_000 + a.late_due);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.first_missed_due), (3, Some(marked_at)));
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);

    // The grace (10 days) and the collection period (30 days) both run from the mark.
    env.warp(marked_at + i64::from(grace) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD));
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

#[test]
fn write_off_after_the_term_waits_for_the_pool_grace() {
    let mut env = Env::new();
    let grace: u32 = 7 * 24 * 3600;
    let k = pool_with(&mut env, 10 * UNIT, grace);
    let council = env.council.insecure_clone();
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
        let ix = env.write_off_ix(&k, line_id, &council.pubkey());
        env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);
        env.warp(term_end + i64::from(grace) + 1);
        let ix = env.write_off_ix(&k, line_id, &b.pubkey());
        env.expect_err(ix, &b, &[], E_UNAUTHORIZED);
        let ix = env.write_off_ix(&k, line_id, &council.pubkey());
        env.send(ix, &council, &[]);
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
    env.warp(t0 + 3 * p.interval());

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
        assert!(env.line(&k, line_id).late_collected <= UNIT);
    }
    // About 0.55 a year on 1.1 overdue: the first year is collected in full, the second only up
    // to the principal (1.0 in all), and nothing after that.
    assert_eq!(settlements, 2);
    assert_eq!(env.line(&k, line_id).late_collected, UNIT);
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
    p.schedule.as_mut().unwrap().first_due_at = env.now() + 2 * year;
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

/// A repayment settles the late interest accrued until its own time and moves the
/// watermark. When the accrued amount rounds down to zero, the watermark must not move: otherwise
/// a borrower paying one unit every few seconds would keep the accrual below one unit forever
/// and never owe late interest at all.
#[test]
fn a_repayment_that_settles_no_late_interest_leaves_the_watermark_where_it_was() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    // 6.0 at 12%: six installments of 1.12, of which 1.0 is principal; 50% a year late interest.
    // The overdue principal share accrues 1.0 * 0.5 / 31_536_000 units a second: about 0.016, so
    // 30 s round to 0 and 70 s to 1.
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);

    env.warp(t0 + p.interval() + 30);
    assert_eq!(env.arrears(&k, line_id).late_due, 0, "30 s of accrual round to zero");
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1);
    let meta = env.send(ix, &b, &[]);
    assert!(find_event::<LateInterestCollected>(&meta).is_none());

    env.warp(t0 + p.interval() + 70);
    assert_eq!(env.arrears(&k, line_id).late_due, 1, "70 s since the due date, not 40 s since the last payment");
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1);
    let meta = env.send(ix, &b, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("the second repayment settles one unit");
    assert_eq!(ev.amount, 1);
    let l = env.line(&k, line_id);
    assert_eq!((l.late_collected, l.late_paid_until), (1, env.now()));
}

/// Decision 28 (vencimiento anticipado at the third missed installment): the mark records the
/// acceleration, from which every installment is due and late interest runs on all remaining
/// principal; paying the installments that were missed before the mark cures nothing, and only a
/// full repayment leaves collection.
#[test]
fn the_mark_accelerates_the_balance() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    assert_eq!(env.line(&k, line_id).accelerated_at, 0);
    env.warp(t0 + 3 * p.interval() + 1);
    let before = env.arrears(&k, line_id);
    assert_eq!((before.due, before.missed, before.overdue), (3, 3, 3_360_000));
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    let marked_at = env.now();
    assert_eq!(env.line(&k, line_id).accelerated_at, marked_at);

    // Everything is due now: the whole balance, 6.0 plus the 0.36 of interest kept, is overdue;
    // late interest accrues on all 6.0 of principal, the first three installments from their own
    // due dates, the other three from the mark.
    assert_eq!(env.line(&k, line_id).interest_due, 360_000);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.due, a.covered, a.missed), (6, 0, 6));
    assert_eq!((a.overdue, a.overdue_principal), (6_360_000, 6_000_000));
    env.warp(marked_at + 10 * 24 * 3600);
    let a = env.arrears(&k, line_id);
    let part_principal = 1_000_000u128; // 1.06 per installment after the mark, 1.0 of it principal
    let seconds = |due: i64| (env.now() - due) as u128;
    let weighted = part_principal * (seconds(t0 + p.interval()) + seconds(t0 + 2 * p.interval()) + seconds(t0 + 3 * p.interval()))
        + 3 * part_principal * seconds(marked_at);
    assert_eq!(a.late_due, (weighted * 10_000 / (10_000 * YEAR as u128)) as u64);

    // Paying what was missed before the mark leaves three installments missed and the line in collection.
    env.repay(&k, line_id, &b, 3_180_000 + a.late_due);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.covered, a.missed, a.overdue), (3, 3, 3_180_000));
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    // Paying the rest closes it.
    let a = env.arrears(&k, line_id);
    env.repay(&k, line_id, &b, 3_180_000 + a.late_due);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.line(&k, line_id).accelerated_at, marked_at, "the record stays");
}
