//! Credit lines repaid in installments: schedule, arrears, late interest and collection.

mod common;
mod reference;

use common::*;
use elote_pool::events::{LateInterestCollected, LineInCollection, LineRequested, LineRepaid, LineWrittenOff, PagarePayment};
use elote_pool::state::{LineStatus, Schedule, ScheduleParams};
use solana_signer::Signer;

/// A pool with one investor who deposited `assets`. The schedule tests lend one line most of
/// the NAV (6 of 10), so the line cap is the whole NAV.
fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool_capped(BPS);
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
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);

    // 6.0 at 12% a year, six installments of principal spanning a year; 50% a year late interest.
    // Nothing is owed at the opening: interest accrues on what is drawn (decision 48).
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
    assert_eq!((line.drawn, line.draws, line.principal_seconds, line.accrued_at), (0, 0, 0, 0));
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
    let k = pool_with(&mut env, 10 * UNIT);
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
        ("late rate above 100%", { let mut p = good; p.late_bps = 10_001; p }, E_WRONG_AMOUNT),
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
    let mut p = with(&|s| {
        s.installments = 3;
        s.max_missed = 3;
        s.interval_seconds = 2 * good.interval();
    });
    p.late_bps = 10_000;
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
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    // One second before the first due date a tranche is accepted.
    env.warp(p.first_due_at() - 1);
    let ev = env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!((ev.drawn, ev.available, ev.availability_end), (3 * UNIT, 3 * UNIT, p.first_due_at()));
    // Interest runs from the draw to the last due date: 3.0 at 12% for 5 intervals plus one second.
    let last_due = p.first_due_at() + 5 * p.interval();
    let seconds = (last_due - (p.first_due_at() - 1)) as u128;
    let expected = (3_000_000u128 * 1_200 * seconds / (10_000 * YEAR as u128)) as u64;
    assert_eq!(env.line(&k, line_id).interest_accrued(last_due + YEAR), Some(expected));

    // At the first due date the window is closed, whatever is still available.
    env.warp(p.first_due_at());
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_AVAILABILITY_ENDED);
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
    let k = pool_with(&mut env, 10 * UNIT);
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

/// 3.0 of a 6.0 scheduled line drawn at the opening, at 12% a year: 30_000 a month while all of
/// it is outstanding. The window stays open until the first due date, two months in, with 3.0
/// still available; the borrower holds one unit more.
fn half_drawn(env: &mut Env, k: &PoolKeys, p: &LineParams) -> (Keypair, u64, i64) {
    let borrower = env.borrower(k);
    let line_id = open(env, k, &borrower, p);
    let t0 = env.now();
    env.draw_amount(k, line_id, &borrower, 3_000_000);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    (borrower, line_id, t0)
}

/// While the line can still draw, a payment on account takes the interest accrued to that moment,
/// then principal (decision 48), and leaves nothing of the interest owed.
#[test]
fn a_payment_on_account_while_the_line_can_draw_takes_the_accrued_interest_then_principal() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1_000_000);
    let meta = env.send(ix, &b, &[]);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part), (30_000, 970_000), "one month accrued, then principal");
    assert_eq!(ev.remaining, 2_030_000, "the principal left; no interest is left unpaid");
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.interest_part, ev.principal_part), (30_000, 970_000));
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Active);
    assert_eq!((line.interest_repaid, line.principal_repaid), (30_000, 970_000));
}

/// Paying on account early lowers the interest that follows (decision 48). Two equal lines settle
/// at the first due date, when neither can draw; the one that repaid 970_000 of principal a month
/// earlier pays a month of 12% on it less, 9_700: 3_050_300 against 3_060_000.
#[test]
fn paying_principal_on_account_early_lowers_the_interest_that_follows() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (early, a, t0) = half_drawn(&mut env, &k, &p);
    let (late, b, _) = half_drawn(&mut env, &k, &p);
    let before = (borrower_balance(&env, &k, &early), borrower_balance(&env, &k, &late));
    env.warp(t0 + p.interval() / 2);
    env.repay(&k, a, &early, 1_000_000);
    // At the first due date the window closes; no late interest has run yet.
    env.warp(p.first_due_at());
    // A month of 12% on 2_030_000, 20_300; and two months on 3_000_000, 60_000.
    for (borrower, line_id, interest) in [(&early, a, 20_300), (&late, b, 60_000)] {
        let ev = env.repay(&k, line_id, borrower, 4 * UNIT);
        assert_eq!(ev.interest_part, interest);
        assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    }
    assert_eq!(before.0 - borrower_balance(&env, &k, &early), 3_050_300);
    assert_eq!(before.1 - borrower_balance(&env, &k, &late), 3_060_000);
}

/// While the line can still draw, a payment of more than is owed takes the principal and the
/// interest accrued, debits nothing more, and does not close the line.
#[test]
fn a_full_payment_while_the_line_can_draw_leaves_it_open() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    let before = borrower_balance(&env, &k, &b);
    let ev = env.repay(&k, line_id, &b, 3_000_000 + 360_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (30_000, 3_000_000, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), 3_030_000, "what is owed and no more");
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Active, "the window is open and half the line can still draw");
}

/// While the line can draw, a payment when nothing is payable (no principal left, the interest
/// accrued already repaid) is refused, never a payment that moves nothing.
#[test]
fn a_payment_when_nothing_is_payable_while_the_line_can_draw_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    env.repay(&k, line_id, &b, 4 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.principal_repaid, line.interest_repaid), (3_000_000, 30_000));
    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1);
    env.expect_err(ix, &b, &[], E_WRONG_AMOUNT);
    assert_eq!(borrower_balance(&env, &k, &b), before);
    assert_eq!(env.line(&k, line_id).interest_repaid, 30_000);
}

/// Decision 48 reverses decision 47's known consequence: once the principal is repaid nothing
/// accrues, whether or not the line can still draw. A borrower who repays everything a month in
/// owes nothing a fortnight later (a payment is refused) nor at the first due date, when the
/// window closes and a payment of one unit closes the line and moves nothing. The título is the
/// debtor's as soon as the line owes nothing, open window or not (decision 49).
#[test]
fn no_interest_accrues_once_the_principal_is_repaid() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    let before = borrower_balance(&env, &k, &b);
    env.warp(t0 + p.interval() / 2);
    let ev = env.repay(&k, line_id, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (30_000, 3_000_000, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.pagares_paid, line.draws, line.status), (1, 1, LineStatus::Active));
    env.deliver_pagare(&k, line_id, 0);

    env.warp(t0 + 3 * p.interval() / 4);
    assert_eq!(env.line(&k, line_id).owed(env.now()), Some(0));
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), UNIT);
    env.expect_err(ix, &b, &[], E_WRONG_AMOUNT);
    env.warp(p.first_due_at());
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), UNIT);
    let meta = env.send(ix, &b, &[]);
    assert!(find_event::<PagarePayment>(&meta).is_none(), "no record left to take an entry");
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining, ev.usdc_received), (0, 0, 0, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.interest_repaid), (LineStatus::Repaid, 30_000));
    assert_eq!(before - borrower_balance(&env, &k, &b), 3_030_000);
}

#[test]
fn missed_installments_grow_over_time() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
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
fn late_interest_accrues_from_the_due_date_and_is_collected_first() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    // 6.0 at 12% in six installments of 1.0 of principal (decision 48); 100% a year late interest.
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    let nav0 = env.nav(&k);

    // On its due date the installment is missed and accrues nothing yet; there is no grace period,
    // so a day later a day of late interest has run: 1.0 * 100% / 365 = 0.002739.
    env.warp(t0 + p.interval());
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue, a.late_due), (1, UNIT, 0));
    env.warp(t0 + p.interval() + 86_400);
    assert_eq!(env.arrears(&k, line_id).late_due, 2_739);

    // 1% of a year after the due date, on the 1.0 of principal overdue (no interest on interest):
    // 1.0 * 100% * 0.01 = 0.01.
    let year = elote_pool::state::SECONDS_PER_YEAR as i64;
    env.warp(t0 + p.interval() + year / 100);
    assert_eq!(env.arrears(&k, line_id).overdue, UNIT);
    assert_eq!(env.arrears(&k, line_id).late_due, 10_000);

    // A repayment must cover the late interest.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 9_999);
    env.expect_err(ix, &b, &[], E_LATE_INTEREST_UNPAID);

    // Late interest, then the interest accrued (6.0 * 12% * (1/6 + 1/100) = 127_200), then the
    // installment's principal.
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 10_000 + 127_200 + UNIT);
    let meta = env.send(ix, &b, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("LateInterestCollected");
    assert_eq!((ev.pool, ev.line_id, ev.amount), (k.pool, line_id, 10_000));
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (127_200, UNIT, 5 * UNIT));
    assert_eq!(ev.nav_after, nav0 + 10_000 + 127_200, "late interest raises the NAV like interest");
    assert_eq!(env.nav(&k), nav0 + 10_000 + 127_200);
    let pool = env.pool(&k);
    assert_eq!(pool.interest_collected, 127_200, "late interest is not ordinary interest");
    assert_eq!(pool.outstanding_principal, 5 * UNIT);
    let l = env.line(&k, line_id);
    assert_eq!((l.late_collected, l.late_paid_until), (10_000, env.now()));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.late_due), (0, 0));

    // The settled period is never charged again: a day later nothing is overdue or late, and
    // the unit goes to the day's ordinary interest.
    env.warp(env.now() + 24 * 3600);
    let ev = env.repay(&k, line_id, &b, 1);
    assert_eq!((ev.interest_part, ev.principal_part), (1, 0));
    assert_eq!(env.line(&k, line_id).late_collected, 10_000);
}

/// Principal paid ahead at the draw (5.0 of 6.0) covers five installments; the last goes unpaid,
/// and after the last due date late interest runs on its 1.0 of principal alone while the ordinary
/// interest, a year of 12% on the 1.0 outstanding, has stopped (decision 48).
#[test]
fn principal_paid_ahead_leaves_late_interest_on_the_last_installment_only() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    let ev = env.repay(&k, line_id, &b, 5 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (0, 5 * UNIT), "nothing accrued yet");

    env.warp(t0 + 6 * p.interval() + YEAR / 100);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.covered, a.missed, a.overdue), (5, 1, UNIT));
    // 1% of a year at 100% on 1.0.
    assert_eq!(a.late_due, 10_000);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 2 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("LateInterestCollected").amount, 10_000);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (120_000, UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

#[test]
fn late_interest_is_capped_and_overpayment_stays_with_the_payer() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(UNIT, 1_000, 3, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 5 * UNIT);

    // Twenty years late at 100% a year: each part of principal stops at its own amount, and so the
    // whole at the 1.0 overdue, which is also the lifetime cap.
    let late = UNIT;
    env.warp(t0 + 3 * p.interval() + 20 * elote_pool::state::SECONDS_PER_YEAR as i64);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.overdue, a.late_due), (UNIT, late));

    // Paying far more than is due debits only what is due.
    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 6 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).unwrap().amount, late);
    // The ordinary interest stopped at the last due date: a year of 10% on 1.0.
    let ev: LineRepaid = find_event(&meta).unwrap();
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, UNIT, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), late + 100_000 + UNIT);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!(line.late_collected, late);
}

/// One repayment order for every line: late interest, then the interest accrued, then principal,
/// and never more than is due (decision 48).
#[test]
fn repayment_order_is_accrued_interest_first_and_clamps_overpayment_on_every_line() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p2 = env.scheduled_params(4 * UNIT, 1_000, 4, 10_000);
    let (b, v2, t0) = drawn(&mut env, &k, &p2, UNIT);
    let p1 = env.line_params(4 * UNIT, 1_000);
    let v1 = open(&mut env, &k, &b, &p1);
    env.draw(&k, v1, &b);

    // A quarter of the year in, a quarter of 10% on 4.0 has accrued on each: 0.1.
    env.warp(t0 + p2.interval());
    let ev = env.repay(&k, v2, &b, 500_000);
    assert_eq!((ev.interest_part, ev.principal_part), (100_000, 400_000));
    let ev = env.repay(&k, v1, &b, 500_000);
    assert_eq!((ev.interest_part, ev.principal_part), (100_000, 400_000), "a bullet line too");

    // Both take what is due and leave the surplus with the payer.
    let before = borrower_balance(&env, &k, &b);
    let ev = env.repay(&k, v1, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_600_000, 0));
    let ev = env.repay(&k, v2, &b, 4 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_600_000, 0));
    assert_eq!(before - borrower_balance(&env, &k, &b), 2 * 3_600_000);
    assert_eq!(env.line(&k, v1).status, LineStatus::Repaid);
    assert_eq!(env.line(&k, v2).status, LineStatus::Repaid);
}

#[test]
fn mark_in_collection_after_three_missed_installments() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
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
    // Half a year and a second of 12% on 6.0 accrued to the mark (0.36), and nothing after it.
    assert_eq!((ev.pool, ev.line_id, ev.missed, ev.remaining), (k.pool, line_id, 3, 6_360_000));
    assert_eq!(env.line(&k, line_id).interest_accrued(t0 + 2 * YEAR), Some(360_000), "the accrual stopped at the mark");
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
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 0);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    env.warp(t0 + 3 * p.interval());
    // Half a year of 12% on 6.0 and one installment of principal.
    env.repay(&k, line_id, &b, 360_000 + UNIT);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Active);
}

#[test]
fn a_line_in_collection_still_accepts_repayments_and_closes_when_repaid() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + 3 * p.interval() + 10);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    // The mark accelerated the balance: all 6.0 of principal is overdue, and the interest accrued
    // to the mark, half a year and ten seconds of 12% on 6.0, is 0.36. The borrower pays it, the
    // late interest and half the principal: the line stays in collection with the other half missed.
    let a = env.arrears(&k, line_id);
    assert!(a.late_due > 0);
    assert_eq!(a.overdue, 6 * UNIT);
    let ev = env.repay(&k, line_id, &b, a.late_due + 360_000 + 3 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (360_000, 3 * UNIT, 3 * UNIT));
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // Paying the rest closes it.
    let late = env.arrears(&k, line_id).late_due;
    let ev = env.repay(&k, line_id, &b, late + 3 * UNIT);
    assert_eq!(ev.remaining, 0);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.pool(&k).outstanding_principal, 0);
}

#[test]
fn late_interest_on_the_maximum_installments_stays_within_the_compute_budget() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
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
/// mark (audit F-03), on top of the oldest missed due date, which is always past at the mark.
#[test]
fn write_off_in_collection_waits_for_the_collection_period_from_the_mark() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);
    let nav0 = env.nav(&k);

    // An active scheduled line before its term is not written off, even with missed installments.
    env.warp(t0 + 3 * p.interval());
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);

    // Collection once the third missed installment's due date has passed. The oldest missed
    // installment fell due at t0 + p.interval(); the write-off still waits for the pool's
    // collection period, counted from the mark.
    env.warp(t0 + 3 * p.interval() + 1);
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
    // Decision 51: the written-off line still owes, and takes a payment as a recovery.
    let late = env.arrears(&k, line_id).late_due;
    env.repay(&k, line_id, &b, late + env.interest_unpaid(&k, line_id) + UNIT);
    assert_eq!(env.pool(&k).written_off, 5 * UNIT);
}

#[test]
fn collection_waits_for_the_due_date_of_the_third_missed_installment_to_pass() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, UNIT);

    // One installment paid on its date, with its interest (6.0 at 12% for a sixth of a year,
    // 120_000): the next three fall due at t0 + 2, 3 and 4 months.
    env.warp(t0 + p.interval());
    env.repay(&k, line_id, &b, 120_000 + UNIT);
    // At the third one's due date, to the second, it is missed but the line is not marked yet.
    env.warp(t0 + 4 * p.interval());
    assert_eq!(env.arrears(&k, line_id).missed, 3);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &b, &[], E_NOT_ENOUGH_MISSED);
    env.warp(t0 + 4 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);

    // The oldest missed installment (t0 + 2 months) is past due; the write-off is allowed once the
    // collection period has run from the mark.
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

/// After the mark there is no cure: the installments not yet due became exigible at the mark, so
/// paying the pre-mark arrears leaves them missed, and the write-off waits for the pool's
/// collection period, counted from the mark.
#[test]
fn after_the_mark_there_is_no_cure_and_the_write_off_waits_for_the_collection_period_from_the_mark() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + 3 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);

    let marked_at = env.now();

    // The borrower pays the three installments that were missed before the mark, with the interest
    // accrued to it (half a year of 12% on 6.0) and the late interest: the other three are still
    // missed, exigible since the mark.
    let a = env.arrears(&k, line_id);
    env.repay(&k, line_id, &b, a.late_due + 360_000 + 3 * UNIT);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.first_missed_due), (3, Some(marked_at)));
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WRITE_OFF_TOO_EARLY);

    // The collection period (30 days) runs from the mark; the installments exigible at the mark
    // are past due a second later, which is not enough.
    env.warp(marked_at + 1);
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
fn write_off_after_the_term_waits_for_the_term_to_pass() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let b = env.borrower(&k);

    // A single-maturity line.
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
        env.warp(term_end);
        let ix = env.write_off_ix(&k, line_id, &council.pubkey());
        env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);
        env.warp(term_end + 1);
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
    let k = pool_with(&mut env, 10 * UNIT);
    // 1.0 at 10% in three installments of principal; 50% a year late interest.
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
    // 0.5 a year on the 1.0 overdue, more the first year for the parts due before the last: the
    // first year is collected in full, the second only up to the principal (1.0 in all), and
    // nothing after that.
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
    let k = pool_with(&mut env, 10 * UNIT);
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
    let k = pool_with(&mut env, 10 * UNIT);
    // 6.0 at 12%: six installments of 1.0 of principal; 50% a year late interest. The overdue
    // principal accrues 1.0 * 0.5 / 31_536_000 units a second: about 0.016, so 30 s round to 0
    // and 70 s to 1.
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
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    assert_eq!(env.line(&k, line_id).accelerated_at, 0);
    env.warp(t0 + 3 * p.interval() + 1);
    let before = env.arrears(&k, line_id);
    assert_eq!((before.due, before.missed, before.overdue), (3, 3, 3 * UNIT));
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &b, &[]);
    let marked_at = env.now();
    assert_eq!(env.line(&k, line_id).accelerated_at, marked_at);

    // Everything is due now: all 6.0 of principal is overdue, with the 0.36 of interest accrued to
    // the mark; late interest accrues on the principal, the first three installments from their
    // own due dates, the other three from the mark.
    let a = env.arrears(&k, line_id);
    assert_eq!((a.due, a.covered, a.missed), (6, 0, 6));
    assert_eq!(a.overdue, 6 * UNIT);
    env.warp(marked_at + 10 * 24 * 3600);
    let a = env.arrears(&k, line_id);
    let part_principal = 1_000_000u128;
    let seconds = |due: i64| (env.now() - due) as u128;
    let weighted = part_principal * (seconds(t0 + p.interval()) + seconds(t0 + 2 * p.interval()) + seconds(t0 + 3 * p.interval()))
        + 3 * part_principal * seconds(marked_at);
    assert_eq!(a.late_due, (weighted * 10_000 / (10_000 * YEAR as u128)) as u64);

    // Paying what was missed before the mark, with the interest, leaves three installments missed
    // and the line in collection.
    env.repay(&k, line_id, &b, a.late_due + 360_000 + 3 * UNIT);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.covered, a.missed, a.overdue), (3, 3, 3 * UNIT));
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    // Paying the rest closes it.
    let a = env.arrears(&k, line_id);
    env.repay(&k, line_id, &b, a.late_due + 3 * UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.line(&k, line_id).accelerated_at, marked_at, "the record stays");
}

const E_WINDOW_CLOSED: u32 = ERR_BASE + elote_pool::errors::PoolError::WindowClosed as u32;
const E_DRAWS_NOT_SUSPENDED: u32 = ERR_BASE + elote_pool::errors::PoolError::DrawsNotSuspended as u32;

/// Decision 45: the borrower gives up what is left of the commitment, so the line can draw nothing
/// more and closes at his will with the payment that leaves it owing nothing, instead of when the
/// calendar closes the window. 3.0 of 6.0 drawn, one month in, a month before the window ends.
#[test]
fn the_borrower_renounces_the_window_and_closes_the_line_at_will() {
    use elote_pool::events::AvailabilityRenounced;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    let before = borrower_balance(&env, &k, &b);

    let ix = env.renounce_availability_ix(&k, line_id, &b.pubkey());
    let meta = env.send(ix, &b, &[]);
    let now = env.now();
    let ev: AvailabilityRenounced = find_event(&meta).expect("AvailabilityRenounced");
    assert_eq!((ev.pool, ev.line_id, ev.borrower, ev.available_forfeited, ev.renounced_at), (k.pool, line_id, b.pubkey(), 3_000_000, now));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.availability_end, line.availability_end_saved), (LineStatus::Active, now, 0));

    let ev = env.repay(&k, line_id, &b, 3_500_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (30_000, 3 * UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(before - borrower_balance(&env, &k, &b), 3_030_000, "the principal and one month of interest");
}

/// The renunciation is the borrower's alone and cannot be undone: nobody else may make it, the
/// pause does not block it, and afterwards nothing draws, the council can neither resume nor
/// suspend, and it is not made twice.
#[test]
fn the_renunciation_is_the_borrowers_alone_and_irrevocable() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, _) = half_drawn(&mut env, &k, &p);
    let council = env.council.insecure_clone();
    let stranger = env.funded(1);
    for other in [&stranger, &council] {
        let ix = env.renounce_availability_ix(&k, line_id, &other.pubkey());
        env.expect_err(ix, other, &[], E_UNAUTHORIZED);
    }

    env.set_paused(&k, true);
    let ix = env.renounce_availability_ix(&k, line_id, &b.pubkey());
    env.send(ix, &b, &[]);
    env.set_paused(&k, false);

    let ix = env.draw_ix_amount(&k, line_id, &b.pubkey(), UNIT);
    env.expect_err(ix, &b, &[], E_AVAILABILITY_ENDED);
    let resume = anchor_lang::InstructionData::data(&elote_pool::instruction::ResumeDraws { valid_until: env.valid_until() });
    let ix = env.council_line_ix(&k, line_id, &council.pubkey(), resume);
    env.expect_err(ix, &council, &[], E_DRAWS_NOT_SUSPENDED);
    let ix = env.suspend_draws_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_WINDOW_CLOSED);
    env.warp(env.now() + 1);
    let ix = env.renounce_availability_ix(&k, line_id, &b.pubkey());
    env.expect_err(ix, &b, &[], E_WINDOW_CLOSED);
}

/// Nothing to renounce: a line never drawn, one drawn in full, one the council suspended, and one
/// whose window the calendar closed.
#[test]
fn a_renunciation_needs_an_open_window_with_something_left_to_draw() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let council = env.council.insecure_clone();

    let b = env.borrower(&k);
    let approved = open(&mut env, &k, &b, &p);
    let ix = env.renounce_availability_ix(&k, approved, &b.pubkey());
    env.expect_err(ix, &b, &[], E_LINE_STATUS);

    let (b, full, _) = drawn(&mut env, &k, &p, 0);
    let ix = env.renounce_availability_ix(&k, full, &b.pubkey());
    env.expect_err(ix, &b, &[], E_WINDOW_CLOSED);

    let (b, suspended, _) = half_drawn(&mut env, &k, &p);
    let ix = env.suspend_draws_ix(&k, suspended, &council.pubkey());
    env.send(ix, &council, &[]);
    let ix = env.renounce_availability_ix(&k, suspended, &b.pubkey());
    env.expect_err(ix, &b, &[], E_WINDOW_CLOSED);

    let (b, lapsed, _) = half_drawn(&mut env, &k, &p);
    env.warp(env.line(&k, lapsed).availability_end);
    let ix = env.renounce_availability_ix(&k, lapsed, &b.pubkey());
    env.expect_err(ix, &b, &[], E_WINDOW_CLOSED);
}

const DOCUMENT_HASH: [u8; 32] = [0xd0; 32];
const E_DOCUMENT_HASH_REQUIRED: u32 = ERR_BASE + elote_pool::errors::PoolError::DocumentHashRequired as u32;

fn condone_ix(env: &Env, k: &PoolKeys, line_id: u64, signer: &solana_pubkey::Pubkey, interest: u64, document_hash: [u8; 32]) -> solana_instruction::Instruction {
    env.condone_ix(k, line_id, signer, interest, 0, document_hash)
}

/// Decision 50: the council condones the late interest a line has accrued, against the hash of
/// the written record. Nothing is collected, the watermark moves, and the next payment owes none.
#[test]
fn the_council_condones_the_late_interest_accrued() {
    use elote_pool::events::LineCondoned;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 2 * UNIT);
    env.warp(t0 + p.interval() + YEAR / 100);
    assert_eq!(env.arrears(&k, line_id).late_due, 10_000);
    let nav = env.nav(&k);

    let council = env.council.insecure_clone();
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 0, DOCUMENT_HASH);
    let meta = env.send(ix, &council, &[]);
    let now = env.now();
    let ev: LineCondoned = find_event(&meta).expect("LineCondoned");
    assert_eq!(
        (ev.pool, ev.line_id, ev.late_interest, ev.interest, ev.document_hash, ev.remaining, ev.condoned_at),
        (k.pool, line_id, 10_000, 0, DOCUMENT_HASH, 6 * UNIT + 127_200, now),
        "6.0 of principal and 6.0 * 12% * (1/6 + 1/100) of interest accrued"
    );
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.late_collected, line.late_paid_until, line.interest_repaid), (LineStatus::Active, 0, now, 0));
    assert_eq!(env.arrears(&k, line_id).late_due, 0);
    assert_eq!(env.nav(&k), nav, "nothing moves");

    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1_120_000);
    let meta = env.send(ix, &b, &[]);
    assert!(find_event::<LateInterestCollected>(&meta).is_none());
}

/// A settlement negotiated in collection: the council first condones the late interest and the
/// ordinary interest, then the debtor pays the principal; the line owes nothing and only then do
/// the títulos leave (decision 49).
#[test]
fn a_quita_then_the_principal_settles_the_line_and_the_titulo_leaves() {
    use elote_pool::events::LineCondoned;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 0);
    env.warp(t0 + 6 * p.interval() + YEAR / 100);
    assert!(env.arrears(&k, line_id).late_due > 0);

    let council = env.council.insecure_clone();
    // A year of 12% on 6.0, stopped at the last due date: 0.72.
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 720_000, DOCUMENT_HASH);
    let meta = env.send(ix, &council, &[]);
    let ev: LineCondoned = find_event(&meta).expect("LineCondoned");
    assert_eq!((ev.interest, ev.remaining), (720_000, 6 * UNIT));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.interest_unpaid(env.now())), (LineStatus::Active, Some(0)), "the principal is still owed");
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &b.pubkey(), &b.pubkey());
    env.expect_err(ix, &b, &[], E_PAGARE_NOT_COVERED);

    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 7 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert!(find_event::<LateInterestCollected>(&meta).is_none());
    assert_eq!(before - borrower_balance(&env, &k, &b), 6 * UNIT, "the principal and nothing else");
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
}

/// Under decision 48 a quita cannot be what leaves a line owing nothing: a payment that repays
/// principal takes the interest accrued first, and nothing accrues on principal repaid, so once the
/// principal is gone there is no interest left to condone (`WrongAmount`), and the título leaves
/// with the payment. (The quita's closing branch is reached by condoning principal, decision 51:
/// `tests/quita.rs`.)
#[test]
fn a_quita_has_nothing_to_condone_once_the_principal_is_repaid() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    env.repay(&k, line_id, &b, 4 * UNIT);
    env.warp(p.first_due_at());
    assert_eq!(env.line(&k, line_id).interest_unpaid(env.now()), Some(0));
    let council = env.council.insecure_clone();
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 1, DOCUMENT_HASH);
    env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);
    env.deliver_pagare(&k, line_id, 0);
}

/// A partial quita takes off the interest accrued and unpaid: two months in, 240_000 accrued;
/// 240_000 condoned leaves no interest payable today. It pays no installment: those are principal
/// (decision 48).
#[test]
fn a_partial_quita_lowers_what_is_payable_now() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, t0) = drawn(&mut env, &k, &p, 0);
    env.warp(t0 + 2 * p.interval());
    let council = env.council.insecure_clone();
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 240_000, DOCUMENT_HASH);
    let meta = env.send(ix, &council, &[]);
    let ev: elote_pool::events::LineCondoned = find_event(&meta).expect("LineCondoned");
    assert_eq!((ev.interest, ev.remaining), (240_000, 6 * UNIT));
    assert_eq!(env.arrears(&k, line_id).missed, 2, "a quita pays no installment");
    let ev = env.repay(&k, line_id, &b, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, UNIT, 5 * UNIT));
    // A month later a month of 12% on 5.0 has accrued, 50_000; late interest has run on the second
    // installment's 1.0 for that month, at 100%: 1.0 / 12 = 83_333.3.
    env.warp(t0 + 5 * p.interval() / 2);
    let late = env.arrears(&k, line_id).late_due;
    assert_eq!(late, 83_333);
    let ev = env.repay(&k, line_id, &b, late + 70_000);
    assert_eq!((ev.interest_part, ev.principal_part), (50_000, 20_000));
}

/// A quita on a line that can still draw does not reach the interest accrued later, on a later
/// draw or on what was drawn before.
#[test]
fn a_quita_on_an_open_line_leaves_the_next_draw_its_interest() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let p = env.scheduled_params(6_000_000, 1_200, 6, 10_000);
    let (b, line_id, t0) = half_drawn(&mut env, &k, &p);
    env.warp(t0 + p.interval() / 2);
    let council = env.council.insecure_clone();
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 30_000, DOCUMENT_HASH);
    env.send(ix, &council, &[]);
    env.draw_amount(&k, line_id, &b, UNIT);
    assert_eq!(env.line(&k, line_id).interest_repaid, 30_000);
    let ev = env.repay(&k, line_id, &b, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (0, UNIT), "the month condoned is not charged again");
    // A month later: a month of 12% on the 3.0 outstanding.
    env.warp(t0 + p.interval());
    let ev = env.repay(&k, line_id, &b, 100_000);
    assert_eq!((ev.interest_part, ev.principal_part), (30_000, 70_000));
}

/// The quita is the council's, against a written record, and never more than is owed.
#[test]
fn a_quita_is_the_councils_needs_its_record_and_something_to_condone() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (b, line_id, _) = drawn(&mut env, &k, &p, 0);
    let council = env.council.insecure_clone();

    let ix = condone_ix(&env, &k, line_id, &b.pubkey(), 1, DOCUMENT_HASH);
    env.expect_err(ix, &b, &[], E_UNAUTHORIZED);
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 1, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    // Drawn this second: nothing accrued, so any interest is more than is owed.
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 1, DOCUMENT_HASH);
    env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);
    let ix = condone_ix(&env, &k, line_id, &council.pubkey(), 0, DOCUMENT_HASH);
    env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);

    let borrower = env.borrower(&k);
    let approved = open(&mut env, &k, &borrower, &p);
    let ix = condone_ix(&env, &k, approved, &council.pubkey(), 1, DOCUMENT_HASH);
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
}

// -----------------------------------------------------------------------------------------------
// Decision 48: ordinary interest on the outstanding principal, against `reference::RefLine`
// -----------------------------------------------------------------------------------------------

use reference::RefLine;

/// A twelfth of a year: 2,628,000 s.
const MONTH12: i64 = YEAR / 12;

/// `principal` at 12% in six monthly installments (a month is `YEAR / 12`), the first due a month
/// from now and the last six months from now, at the term; late interest `late_bps` a year.
fn monthly(env: &Env, principal: u64, late_bps: u16) -> LineParams {
    let mut p = env.scheduled_params(principal, 1_200, 6, late_bps);
    p.schedule = Some(ScheduleParams { first_due_at: env.now() + MONTH12, interval_seconds: MONTH12, installments: 6, max_missed: 3 });
    p.term_end = env.now() + 6 * MONTH12;
    p
}

/// A monthly line of `principal` drawn in full now, its reference, and the borrower holding
/// `extra` more.
fn monthly_drawn(env: &mut Env, k: &PoolKeys, principal: u64, late_bps: u16, extra: u64) -> (Keypair, u64, i64, RefLine) {
    let p = monthly(env, principal, late_bps);
    let (b, line_id, t0) = drawn(env, k, &p, extra);
    assert_eq!(t0, p.first_due_at() - MONTH12);
    let mut r = RefLine::new(1_200, t0 + 6 * MONTH12);
    r.draw(t0, principal);
    (b, line_id, t0, r)
}

/// Decision 48 (d): 60,000.00 at 12%, six monthly principal parts of 10,000.00 paid on their
/// dates. Each payment takes the month's interest on the principal outstanding: 600 + 500 + 400 +
/// 300 + 200 + 100 = 2,100.00. The same line untouched to the last date accrues 6 x 600 = 3,600.00.
#[test]
fn repaying_principal_on_its_dates_lowers_the_interest_that_follows() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    let before = borrower_balance(&env, &k, &b);
    let mut parts = vec![];
    for i in 1..=6i64 {
        env.warp(t0 + i * MONTH12);
        // 10,000.00 of principal and a month on what is outstanding: (7 - i) * 10,000.00 * 1%.
        let interest = (7 - i as u64) * 10_000;
        let ev = env.repay(&k, line_id, &b, 1_000_000 + interest);
        parts.push((ev.interest_part, ev.principal_part));
        assert_eq!(env.arrears(&k, line_id).missed, 0, "installment {i} met on its date");
    }
    assert_eq!(
        parts,
        vec![(60_000, 1_000_000), (50_000, 1_000_000), (40_000, 1_000_000), (30_000, 1_000_000), (20_000, 1_000_000), (10_000, 1_000_000)]
    );
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(before - borrower_balance(&env, &k, &b), 6_000_000 + 210_000, "2,100.00 of interest");

    // No late interest on the twin, so the payment shows the ordinary interest alone.
    let (b, untouched, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 0, UNIT);
    env.warp(t0 + 6 * MONTH12);
    let ev = env.repay(&k, untouched, &b, 7 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (360_000, 6_000_000, 0), "3,600.00");
}

/// Decision 48 (d): an early payoff, at any second, costs the principal and the interest accrued
/// to that second, and nothing more.
#[test]
fn an_early_payoff_costs_the_principal_and_the_interest_to_that_second() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    for offset in [1, 86_400 + 17, MONTH12 - 1, MONTH12, 45 * 86_400 + 3_601] {
        let (b, line_id, t0, r) = monthly_drawn(&mut env, &k, 6_000_000, 0, UNIT);
        env.warp(t0 + offset);
        let before = borrower_balance(&env, &k, &b);
        let ev = env.repay(&k, line_id, &b, 7 * UNIT);
        let interest = r.interest_accrued(t0 + offset) as u64;
        assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (interest, 6_000_000, 0), "{offset} s in");
        assert_eq!(before - borrower_balance(&env, &k, &b), 6_000_000 + interest);
        assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    }
}

/// Decision 48 (a): over a line with several draws and several principal repayments, the interest
/// paid is `rate x ∫ balance dt`, payment by payment, as the reference computes it.
#[test]
fn interest_paid_over_the_life_of_a_line_is_the_rate_times_the_balance_integral() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let p = monthly(&env, 6_000_000, 10_000);
    let b = env.borrower(&k);
    let line_id = open(&mut env, &k, &b, &p);
    let t0 = p.first_due_at() - MONTH12;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &b.pubkey(), UNIT);
    let mut r = RefLine::new(1_200, t0 + 6 * MONTH12);

    let draw = |env: &mut Env, r: &mut RefLine, at: i64, amount: u64| {
        env.warp(at);
        env.draw_amount(&k, line_id, &b, amount);
        r.draw(at, amount);
    };
    let pay = |env: &mut Env, r: &mut RefLine, at: i64, amount: u64| {
        env.warp(at);
        let ev = env.repay(&k, line_id, &b, amount);
        assert_eq!((ev.interest_part, ev.principal_part), { let (_, i, p) = r.pay(at, amount, 0); (i, p) }, "the payment at {at}");
    };
    draw(&mut env, &mut r, t0, 2_000_000);
    draw(&mut env, &mut r, t0 + 10 * 86_400 + 7, 3_000_000);
    pay(&mut env, &mut r, t0 + 15 * 86_400, 1_500_000);
    draw(&mut env, &mut r, t0 + MONTH12 - 1, 1_000_000);
    // On each due date the principal reaches the schedule (k x 1,000,000.00 of 6,000,000.00).
    pay(&mut env, &mut r, t0 + MONTH12, 1_000_000);
    let amount = r.interest_unpaid(t0 + 2 * MONTH12) as u64 + 123_457;
    pay(&mut env, &mut r, t0 + 2 * MONTH12, amount);
    let amount = r.interest_unpaid(t0 + 3 * MONTH12) as u64 + 3_000_000 - r.principal_repaid as u64;
    pay(&mut env, &mut r, t0 + 3 * MONTH12, amount);
    pay(&mut env, &mut r, t0 + 3 * MONTH12 + 86_400 * 11 + 5, 777);
    // Before the fourth due date: nothing is late.
    let end = t0 + 3 * MONTH12 + 20 * 86_400;
    pay(&mut env, &mut r, end, 7 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!((line.principal_repaid, u128::from(line.interest_repaid)), (6_000_000, r.interest_accrued(end)));
    assert_eq!(r.interest_paid, r.interest_accrued(end));
}

/// Decision 48 (e): the schedule is on principal and interest is taken first, so paying exactly
/// the principal part on the due date leaves the installment short by the month's interest; it is
/// missed, and late interest runs on that shortfall from the due date: 600.00 at 100% for ten
/// days, 60,000 x 864,000 / 31,536,000 = 1,643.8, so 1,643 centavos. A payment smaller than the
/// interest pays interest only, and the whole principal part stays missed.
#[test]
fn paying_only_the_principal_part_on_the_due_date_misses_the_installment_by_the_interest() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    env.warp(t0 + MONTH12);
    let ev = env.repay(&k, line_id, &b, 1_000_000);
    assert_eq!((ev.interest_part, ev.principal_part), (60_000, 940_000));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue), (1, 60_000));
    env.warp(t0 + MONTH12 + 10 * 86_400);
    assert_eq!(env.arrears(&k, line_id).late_due, 1_643);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 1_643 + 60_000);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("late").amount, 1_643);
    // Ten days of interest on 5,060,000: 5,060,000 x 12% x 864,000 / 31,536,000 = 16,635.6.
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part), (16_635, 60_000 - 16_635));

    // A payment below the month's interest pays interest only.
    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    env.warp(t0 + MONTH12);
    let ev = env.repay(&k, line_id, &b, 30_000);
    assert_eq!((ev.interest_part, ev.principal_part), (30_000, 0));
    env.warp(t0 + MONTH12 + 10 * 86_400);
    // Late interest on the whole principal part, never on the interest: 1,000,000 x 864,000 /
    // 31,536,000 = 27,397.2.
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue, a.late_due), (1, 1_000_000, 27_397));
}

/// Decision 48 (e): principal paid three installments ahead and then nothing: no installment is
/// missed until the principal schedule catches up, the interest left unpaid meanwhile carries no
/// late interest, and the next payment takes it first.
#[test]
fn principal_paid_ahead_holds_off_the_misses_until_the_schedule_catches_up() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, line_id, t0, mut r) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    let at = t0 + 86_400;
    env.warp(at);
    // A day on 6,000,000.00: 6,000,000 x 12% x 86,400 / 31,536,000 = 1,972.6.
    let ev = env.repay(&k, line_id, &b, 3_001_972);
    assert_eq!((ev.interest_part, ev.principal_part), (1_972, 3_000_000));
    r.pay(at, 3_001_972, 0);
    for (when, missed) in [(t0 + MONTH12, 0), (t0 + 3 * MONTH12, 0), (t0 + 4 * MONTH12 - 1, 0), (t0 + 4 * MONTH12, 1)] {
        env.warp(when);
        assert_eq!(env.arrears(&k, line_id).missed, missed, "at {} s", when - t0);
    }
    // Ten days past the fourth due date: late interest on the 1,000,000 of principal short only,
    // 1,000,000 x 864,000 / 31,536,000 = 27,397.2.
    let at = t0 + 4 * MONTH12 + 10 * 86_400;
    env.warp(at);
    assert_eq!(env.arrears(&k, line_id).late_due, 27_397);
    let interest = r.interest_unpaid(at) as u64;
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 27_397 + interest + 1_000_000);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("late").amount, 27_397);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part), (interest, 1_000_000));
    assert_eq!(env.arrears(&k, line_id).missed, 0);
}

/// Decision 48 (e, f): before the last due date overdue principal accrues ordinary and late
/// interest both; after it only late interest runs. Nothing paid on a monthly line of 60,000.00:
/// - two months and ten days in: ordinary 6,000,000 x 12% x 6,120,000 / 31,536,000 = 139,726.0;
///   late on installment 1 for 3,492,000 s and installment 2 for 864,000 s, 1,000,000 x 4,356,000
///   / 31,536,000 = 138,127.8;
/// - thirty days past the last due date: ordinary stops at 6 x 60,000 = 360,000; late on the six
///   installments for 15 months and 180 days in all, 1,000,000 x 54,972,000 / 31,536,000 =
///   1,743,150.6.
#[test]
fn overdue_principal_accrues_both_interests_until_the_last_due_date_then_only_late_interest() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, 2 * UNIT);
    env.warp(t0 + 2 * MONTH12 + 10 * 86_400);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 7 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("late").amount, 138_127);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (139_726, 6_000_000, 0));

    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, 3 * UNIT);
    env.warp(t0 + 6 * MONTH12 + 30 * 86_400);
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 9 * UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("late").amount, 1_743_150);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (360_000, 6_000_000, 0));
}

/// Decision 48 (b), dust invariance. 394,200,000 at 12% accrues 1.5 centavos a second. A third
/// party pays one centavo every second for an hour: every centavo is interest, and the line has
/// accrued exactly what an untouched twin accrues, 1.5 x 3,600 = 5,400 (a payer rounding per payment
/// would have erased 1,800). Both pay off for the same total.
#[test]
fn one_centavo_a_second_for_an_hour_erases_no_interest() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 800 * UNIT);
    let principal = 394_200_000;
    let p = env.line_params(principal, 1_200);
    let b = env.borrower(&k);
    let dusted = open(&mut env, &k, &b, &p);
    let twin = open(&mut env, &k, &b, &p);
    env.draw(&k, dusted, &b);
    env.draw(&k, twin, &b);
    let t0 = env.now();
    let stranger = env.funded(1);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), 3_600);
    env.mint_to(&k.asset_mint, &council, &b.pubkey(), 2 * UNIT);
    for s in 1..=3_600 {
        env.warp(t0 + s);
        let ev = env.repay(&k, dusted, &stranger, 1);
        assert_eq!((ev.interest_part, ev.principal_part), (1, 0), "second {s}");
    }
    let line = env.line(&k, dusted);
    assert_eq!((line.interest_repaid, line.principal_repaid), (3_600, 0));
    assert_eq!(line.interest_accrued(env.now()), Some(5_400));
    assert_eq!(env.line(&k, twin).interest_accrued(env.now()), Some(5_400));
    let a = env.repay(&k, dusted, &b, principal + UNIT);
    let t = env.repay(&k, twin, &b, principal + UNIT);
    assert_eq!((a.interest_part, a.principal_part), (1_800, principal));
    assert_eq!((t.interest_part, t.principal_part), (5_400, principal));
}

/// Decision 48 (b): one centavo a second on 6,000,000 at 12% (0.0228 centavos of interest a
/// second) mostly reaches principal. Each payment splits as the reference splits it; a month later
/// the payoff brings the interest paid to exactly `rate x ∫ balance dt`. A twin paying the same
/// 3,600 centavos at once at the end of the hour pays more interest, by exactly the time-value of
/// the principal the dust repaid earlier, as the reference counts it.
#[test]
fn dust_payments_differ_from_one_payment_only_by_the_time_value_they_save() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, dusted, t0, mut r) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    let p = monthly(&env, 6_000_000, 10_000);
    let twin = open(&mut env, &k, &b, &p);
    env.draw(&k, twin, &b);
    let mut rt = RefLine::new(1_200, t0 + 6 * MONTH12);
    rt.draw(t0, 6_000_000);
    let stranger = env.funded(1);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), 2 * 3_600);
    for s in 1..=3_600 {
        env.warp(t0 + s);
        let ev = env.repay(&k, dusted, &stranger, 1);
        let (_, i, pr) = r.pay(t0 + s, 1, 0);
        assert_eq!((ev.interest_part, ev.principal_part), (i, pr), "second {s}");
    }
    let ev = env.repay(&k, twin, &stranger, 3_600);
    let (_, i, pr) = rt.pay(t0 + 3_600, 3_600, 0);
    assert_eq!((ev.interest_part, ev.principal_part), (i, pr));
    assert!(r.principal_repaid > 3_000, "the dust reached principal: {}", r.principal_repaid);

    let end = t0 + 30 * 86_400;
    env.warp(end);
    let mut paid = [0u64; 2];
    for (n, (line_id, rf)) in [(dusted, &mut r), (twin, &mut rt)].into_iter().enumerate() {
        let ev = env.repay(&k, line_id, &b, 7 * UNIT);
        assert_eq!((ev.interest_part, ev.principal_part), { let (_, i, p) = rf.pay(end, 7 * UNIT, 0); (i, p) });
        let line = env.line(&k, line_id);
        assert_eq!(u128::from(line.interest_repaid), rf.interest_accrued(end), "rate x ∫ balance dt");
        paid[n] = line.interest_repaid;
    }
    let saved = rt.interest_accrued(end) - r.interest_accrued(end);
    assert_eq!(u128::from(paid[1] - paid[0]), saved);
    // The bound by hand: 3,600 centavos for at most an hour at 12% is below one centavo; with the
    // two roundings, the difference is at most one.
    assert!(saved <= 1, "saved {saved}");
}

/// Decision 48 (c), split invariance: a payoff in five transactions in the same second, the first
/// covering the late interest, costs what one transaction costs on a twin line, split the same way
/// between late interest, interest and principal.
#[test]
fn a_payoff_split_in_one_second_costs_what_one_payment_costs() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 30 * UNIT);
    let (b, split, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, 4 * UNIT);
    let p = monthly(&env, 6_000_000, 10_000);
    let one = open(&mut env, &k, &b, &p);
    env.draw(&k, one, &b);
    env.warp(t0 + 2 * MONTH12 + 10 * 86_400);
    let late = env.arrears(&k, split).late_due;
    assert_eq!(late, 138_127, "as overdue_principal_accrues_both_interests... computes it");

    let before = borrower_balance(&env, &k, &b);
    let ix = env.repay_ix(&k, one, &b.pubkey(), 9 * UNIT);
    let meta = env.send(ix, &b, &[]);
    let whole: LineRepaid = find_event(&meta).expect("LineRepaid");
    let one_cost = before - borrower_balance(&env, &k, &b);

    let before = borrower_balance(&env, &k, &b);
    let (mut interest, mut principal, mut collected) = (0, 0, 0);
    for amount in [late + 1_000, 50_000, 1_234_567, 2_000_000, 9 * UNIT] {
        let ix = env.repay_ix(&k, split, &b.pubkey(), amount);
        let meta = env.send(ix, &b, &[]);
        let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
        interest += ev.interest_part;
        principal += ev.principal_part;
        collected += find_event::<LateInterestCollected>(&meta).map_or(0, |e| e.amount);
    }
    assert_eq!(before - borrower_balance(&env, &k, &b), one_cost);
    assert_eq!((collected, interest, principal), (late, whole.interest_part, whole.principal_part));
    assert_eq!((interest, principal), (139_726, 6_000_000));
    assert_eq!((env.line(&k, split).status, env.line(&k, one).status), (LineStatus::Repaid, LineStatus::Repaid));
}

/// Decision 48 (g), the invariant counsel relies on to drop the interest reservation: after any
/// payment that applies principal, the line has no ordinary interest accrued and unpaid — whoever
/// pays (borrower, aval, third party), on an active line or one in collection, after a quita.
#[test]
fn after_any_payment_that_touches_principal_no_interest_is_left_unpaid() {
    use elote_pool::state::SPONSOR_GUARANTOR;
    let mut env = Env::new();
    let k = pool_with(&mut env, 40 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let aval = env.funded(1);
    env.create_ata(&aval.pubkey(), &k.asset_mint);
    let stranger = env.funded(1);
    for who in [&aval, &stranger] {
        env.mint_to(&k.asset_mint, &council, &who.pubkey(), 10 * UNIT);
    }
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 10 * UNIT);
    let mut p = monthly(&env, 6_000_000, 10_000);
    p.sponsor = Some(aval.pubkey());
    p.sponsor_role = SPONSOR_GUARANTOR;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, Some(&aval));
    let t0 = env.now();
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let check = |env: &mut Env, payer: &Keypair, at: i64, amount: u64| {
        env.warp(at);
        let ev = env.repay(&k, line_id, payer, amount);
        assert!(ev.principal_part > 0, "the payment at {} touches principal", at - t0);
        assert_eq!(env.interest_unpaid(&k, line_id), 0, "the payment at {}", at - t0);
    };
    check(&mut env, &borrower, t0 + 86_400 + 7, 300_001);
    env.warp(t0 + 10 * 86_400);
    env.draw_amount(&k, line_id, &borrower, 4 * UNIT);
    check(&mut env, &aval, t0 + 20 * 86_400 + 3, 1_000_000);
    check(&mut env, &stranger, t0 + MONTH12 + 11, 777_777);
    // A quita of part of the interest, then a payment.
    env.warp(t0 + 3 * MONTH12);
    let ix = env.condone_ix(&k, line_id, &council.pubkey(), 1_000, 0, DOCUMENT_HASH);
    env.send(ix, &council, &[]);
    let late = env.arrears(&k, line_id).late_due;
    check(&mut env, &borrower, t0 + 3 * MONTH12, late + 500_000);
    // In collection: three installments missed, the third past its due date.
    env.warp(t0 + 5 * MONTH12 + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &stranger, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    let late = env.arrears(&k, line_id).late_due;
    check(&mut env, &aval, t0 + 5 * MONTH12 + 1, late + 1_000_000);
    env.warp(t0 + 7 * MONTH12);
    let late = env.arrears(&k, line_id).late_due;
    check(&mut env, &stranger, t0 + 7 * MONTH12, late + 2);
    let payoff = env.payoff(&k, line_id);
    check(&mut env, &borrower, t0 + 7 * MONTH12, payoff);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

mod state_machine {
    //! Decision 48 (h): random sequences of draws, payments of any size and time, and marks, on a
    //! scheduled or a bullet line, against the reference and the invariants.
    use super::*;
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    pub enum Op {
        /// A tranche of this many thousandths of what is available.
        Draw(u16),
        /// A payment: dust, a random amount, or the payoff.
        Pay(u8, u64),
        Warp(i64),
        Mark,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (1u16..=1_000).prop_map(Op::Draw),
            (0u8..3, 1u64..=8 * UNIT).prop_map(|(kind, amount)| Op::Pay(kind, amount)),
            prop_oneof![0i64..=60, 0i64..=86_400, 0i64..=90 * 86_400].prop_map(Op::Warp),
            Just(Op::Mark),
        ]
    }

    const E_OVERFLOW_CODE: u32 = E_OVERFLOW;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        #[test]
        fn random_sequences_keep_the_invariants(
            scheduled in any::<bool>(),
            rate in prop_oneof![Just(0u16), Just(1_200u16), Just(3_650u16), 1u16..=10_000],
            ops in proptest::collection::vec(op(), 1..24),
        ) {
            let mut env = Env::new();
            let k = pool_with(&mut env, 40 * UNIT);
            let b = env.borrower(&k);
            let council = env.council.insecure_clone();
            env.mint_to(&k.asset_mint, &council, &b.pubkey(), 30 * UNIT);
            let mut p = if scheduled { monthly(&env, 6 * UNIT, 10_000) } else { env.line_params(6 * UNIT, rate) };
            p.rate_bps = rate;
            p.late_bps = 10_000;
            let line_id = open(&mut env, &k, &b, &p);
            let last_due = env.line(&k, line_id).last_due().unwrap();
            let mut r = RefLine::new(rate, last_due);
            let keeper = env.funded(1);
            // The signed message of every pagaré, as its draw sent it (decision 55).
            let mut signed: Vec<PagareSignature> = Vec::new();
            let line_key = line_pda(&k.pool, line_id).0;
            for op in ops {
                let before = env.line(&k, line_id);
                match op {
                    Op::Draw(permille) => {
                        let available = before.principal - before.drawn;
                        let amount = (u128::from(available) * u128::from(permille) / 1_000) as u64;
                        let amount = amount.max(100_000).min(available);
                        if amount == 0 { continue; }
                        let ix = env.draw_ix_amount(&k, line_id, &b.pubkey(), amount);
                        let message = env.default_pagare_signature(&line_key, before.draws);
                        match env.try_send(ix, &b, &[]) {
                            Ok(_) => {
                                r.draw(env.now(), amount);
                                signed.push(message);
                            }
                            Err(e) => {
                                let code = custom_error_code(&e.err);
                                prop_assert!(
                                    [E_AVAILABILITY_ENDED, E_LINE_STATUS, E_WRONG_AMOUNT, E_INVALID_SCHEDULE].map(Some).contains(&code),
                                    "draw refused with {code:?}"
                                );
                            }
                        }
                    }
                    Op::Pay(kind, amount) => {
                        if !matches!(before.status, LineStatus::Active | LineStatus::InCollection) { continue; }
                        let now = env.now();
                        let late = env.arrears(&k, line_id).late_due;
                        let amount = match kind { 0 => amount % 100 + 1, 1 => amount, _ => env.payoff(&k, line_id).max(1) };
                        let can_draw = before.can_draw(now).unwrap();
                        let ix = env.repay_ix(&k, line_id, &b.pubkey(), amount);
                        match env.try_send(ix, &b, &[]) {
                            Ok(meta) => {
                                let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
                                let collected = find_event::<LateInterestCollected>(&meta).map_or(0, |e| e.amount);
                                prop_assert_eq!(collected, late);
                                let (_, i, pr) = r.pay(now, amount, late);
                                prop_assert_eq!((ev.interest_part, ev.principal_part), (i, pr), "against the reference");
                                let line = env.line(&k, line_id);
                                if ev.principal_part > 0 {
                                    prop_assert_eq!(line.interest_unpaid(now), Some(0), "nothing accrued is left unpaid");
                                }
                                let owed = line.owed(now).unwrap();
                                prop_assert_eq!(ev.remaining, owed);
                                prop_assert_eq!(line.status == LineStatus::Repaid, owed == 0 && !can_draw, "closes iff it owes nothing and cannot draw");
                            }
                            Err(e) => {
                                let code = custom_error_code(&e.err);
                                prop_assert!(code != Some(E_OVERFLOW_CODE), "repay failed on arithmetic");
                                // A payment touching more than three records is split by the client.
                                if code == Some(E_TOO_MANY_SIDECARS) {
                                    prop_assert!(env.payment_sidecars(&k, line_id, amount).len() > elote_pool::state::MAX_SIDECARS_PER_REPAY);
                                    continue;
                                }
                                let expected = if amount < late { Some(E_LATE_INTEREST_UNPAID) } else { Some(E_WRONG_AMOUNT) };
                                prop_assert_eq!(code, expected, "repay of {} refused", amount);
                                if code == Some(E_WRONG_AMOUNT) {
                                    prop_assert!(can_draw && before.owed(now) == Some(0) && late == 0);
                                }
                            }
                        }
                    }
                    Op::Warp(seconds) => env.warp(env.now() + seconds),
                    Op::Mark => {
                        let ix = env.mark_in_collection_ix(&k, line_id);
                        match env.try_send(ix, &keeper, &[]) {
                            Ok(_) => r.end = r.end.min(env.now()),
                            Err(e) => {
                                let code = custom_error_code(&e.err);
                                prop_assert!([E_NOT_ENOUGH_MISSED, E_LINE_STATUS, E_NOT_SCHEDULED].map(Some).contains(&code), "mark refused with {code:?}");
                            }
                        }
                    }
                }
                let line = env.line(&k, line_id);
                let now = env.now();
                prop_assert!(line.principal_repaid <= line.drawn);
                prop_assert!(line.interest_repaid <= line.interest_accrued(now).unwrap());
                prop_assert_eq!(u128::from(line.interest_accrued(now).unwrap()), r.interest_accrued(now), "the accrual against the reference");
                prop_assert_eq!(u128::from(line.interest_repaid), r.interest_paid);
                for (i, s) in signed.iter().enumerate() {
                    let p = env.pagare(&k, line_id, i as u16);
                    prop_assert_eq!((p.signed_pagare_hash, p.pagare_signature, p.signed_on), (s.hash, s.signature, s.signed_on), "no instruction rewrites the signed message");
                }
            }
        }
    }
}

/// Decision 48 (e): one day late. The first installment of the monthly line paid a day after its
/// date: a day of late interest on its 1,000,000 of principal, 1,000,000 x 86,400 / 31,536,000 =
/// 2,739.7; a month and a day of 12% on 6,000,000, 6,000,000 x 1,200 x 2,714,400 /
/// 315,360,000,000 = 61,972.6; and the principal. The installment is then met.
#[test]
fn an_installment_paid_a_day_late_pays_a_day_of_late_interest_on_its_principal() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let (b, line_id, t0, _) = monthly_drawn(&mut env, &k, 6_000_000, 10_000, UNIT);
    env.warp(t0 + MONTH12 + 86_400);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.missed, a.overdue, a.late_due), (1, UNIT, 2_739));
    let ix = env.repay_ix(&k, line_id, &b.pubkey(), 2_739 + 61_972 + UNIT);
    let meta = env.send(ix, &b, &[]);
    assert_eq!(find_event::<LateInterestCollected>(&meta).expect("late").amount, 2_739);
    let ev: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (61_972, UNIT, 5 * UNIT));
    assert_eq!(env.arrears(&k, line_id).missed, 0);
}

/// The audit's dust line: one centavo of principal per installment is the least `draw` accepts
/// (decision 48), at 6 and at 120 installments; such a line misses, is marked, and takes payments
/// of one centavo and its payoff without an arithmetic failure.
#[test]
fn a_line_of_one_centavo_per_installment_never_bricks_repay() {
    for n in [6u16, 120] {
        let mut env = Env::new();
        let k = pool_with(&mut env, 20 * UNIT);
        let p = env.scheduled_params(UNIT, 1_200, n, 10_000);
        let b = env.borrower(&k);
        let line_id = open(&mut env, &k, &b, &p);
        let ix = env.draw_ix_amount(&k, line_id, &b.pubkey(), u64::from(n) - 1);
        env.expect_err(ix, &b, &[], E_INVALID_SCHEDULE);
        env.draw_amount(&k, line_id, &b, u64::from(n));
        let council = env.council.insecure_clone();
        env.mint_to(&k.asset_mint, &council, &b.pubkey(), UNIT);
        let t0 = env.line(&k, line_id).drawn_at;
        env.warp(t0 + 3 * p.interval() + 1);
        assert_eq!(env.arrears(&k, line_id).missed, 3, "{n} installments");
        let keeper = env.funded(1);
        let ix = env.mark_in_collection_ix(&k, line_id);
        env.send(ix, &keeper, &[]);
        assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
        for _ in 0..3 {
            env.warp(env.now() + 86_400);
            let late = env.arrears(&k, line_id).late_due;
            let ev = env.repay(&k, line_id, &b, late + 1);
            assert_eq!(ev.principal_part + ev.interest_part, 1, "{n} installments");
        }
        env.warp(env.now() + YEAR);
        let payoff = env.payoff(&k, line_id);
        env.repay(&k, line_id, &b, payoff);
        assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid, "{n} installments");
    }
}
