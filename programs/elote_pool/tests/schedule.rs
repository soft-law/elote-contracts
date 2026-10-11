//! Credit lines repaid in installments: schedule, arrears, late interest and collection.

mod common;

use common::*;
use elote_pool::events::{LineOpened, LineScheduled};
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
        Schedule { interval_seconds: MONTH, installments: 6, late_bps: 5_000, max_missed: 3, flags: 0, late_collected: 0 }
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

    // The edges are accepted: one installment, max_missed == installments, 100% late rate,
    // last due date exactly at the term.
    let p = with(&|s| {
        s.installments = 1;
        s.max_missed = 1;
        s.late_bps = 10_000;
        s.interval_seconds = 6 * MONTH;
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
