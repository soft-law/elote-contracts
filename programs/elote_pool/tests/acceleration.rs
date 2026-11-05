//! The council's line instructions: suspension and resumption of draws, acceleration with notice,
//! the pause by a member or the guardian, the pool limits, and the per-instruction multisig check.

mod common;

use common::*;
use elote_pool::errors::PoolError;
use elote_pool::events::{
    AccelerationProposed, AccelerationWithdrawn, GuardianSet, LineAccelerated, LineDrawsResumed, LineInCollection, PoolPaused, PoolResumed,
};
use elote_pool::instruction as ix;
use elote_pool::state::LineStatus;
use anchor_lang::InstructionData;
use solana_signer::Signer;

fn code(e: PoolError) -> u32 {
    ERR_BASE + e as u32
}

fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

/// `data` is built with the harness's `valid_until`.
fn expect_line(env: &mut Env, k: &PoolKeys, line_id: u64, data: impl FnOnce(i64) -> Vec<u8>, e: PoolError) {
    let council = env.council.insecure_clone();
    let i = env.council_line_ix(k, line_id, &council.pubkey(), data(env.valid_until()));
    env.expect_err(i, &council, &[], code(e));
}

#[test]
fn suspension_keeps_the_original_window_and_resumption_restores_it() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    let original = env.line(&k, id).availability_end;
    assert_eq!(env.line(&k, id).availability_end_saved, 0, "not suspended");

    env.council_line(&k, id, ix::SuspendDraws { valid_until: env.valid_until() }.data());
    let line = env.line(&k, id);
    assert_eq!((line.availability_end, line.availability_end_saved), (env.now(), original), "suspended: saved != 0");
    let i = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.expect_err(i, &borrower, &[], E_AVAILABILITY_ENDED);
    env.warp(env.now() + 100);
    expect_line(&mut env, &k, id, |vu| ix::SuspendDraws { valid_until: vu }.data(), PoolError::WindowClosed);
    assert_eq!(env.line(&k, id).availability_end_saved, original, "the original end is kept");

    let ev: LineDrawsResumed = find_event(&env.council_line(&k, id, ix::ResumeDraws { valid_until: env.valid_until() }.data())).expect("LineDrawsResumed");
    assert_eq!(ev.availability_end, original);
    let line = env.line(&k, id);
    assert_eq!((line.availability_end, line.availability_end_saved), (original, 0));
    env.draw_amount(&k, id, &borrower, UNIT);
    expect_line(&mut env, &k, id, |vu| ix::ResumeDraws { valid_until: vu }.data(), PoolError::DrawsNotSuspended);
}

#[test]
fn a_proposal_suspends_draws_and_withdrawing_it_does_not_resume_them() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (undrawn, _) = env.open_line_with(&k, &borrower, &p, None);
    expect_line(&mut env, &k, undrawn, |vu| ix::ProposeAcceleration { valid_until: vu }.data(), PoolError::LineStatus);

    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, id, &borrower, UNIT);
    let original = env.line(&k, id).availability_end;
    let ev: AccelerationProposed = find_event(&env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data())).expect("AccelerationProposed");
    let due_at = env.now() + i64::from(ACCELERATION_NOTICE);
    assert_eq!((ev.line_id, ev.due_at), (id, due_at));
    let line = env.line(&k, id);
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (due_at, original));
    let i = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.expect_err(i, &borrower, &[], E_AVAILABILITY_ENDED);
    expect_line(&mut env, &k, id, |vu| ix::ProposeAcceleration { valid_until: vu }.data(), PoolError::AccelerationState);
    expect_line(&mut env, &k, id, |vu| ix::ResumeDraws { valid_until: vu }.data(), PoolError::AccelerationState);

    let meta = env.council_line(&k, id, ix::WithdrawAcceleration { valid_until: env.valid_until() }.data());
    assert!(find_event::<AccelerationWithdrawn>(&meta).is_some());
    let line = env.line(&k, id);
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (0, original), "still suspended");
    expect_line(&mut env, &k, id, |vu| ix::WithdrawAcceleration { valid_until: vu }.data(), PoolError::AccelerationState);
    env.council_line(&k, id, ix::ResumeDraws { valid_until: env.valid_until() }.data());
    env.draw_amount(&k, id, &borrower, UNIT);
}

#[test]
fn a_bullet_line_accelerated_keeps_the_interest_accrued_to_the_mark() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000); // 10% a year for a year
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, id, &borrower);
    let t0 = env.now();
    assert_eq!(env.line(&k, id).interest_due, 400_000);

    env.warp(t0 + YEAR / 4 - i64::from(ACCELERATION_NOTICE));
    env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    env.warp(t0 + YEAR / 4 - 1);
    expect_line(&mut env, &k, id, |vu| ix::AccelerateLine { valid_until: vu }.data(), PoolError::AccelerationNotDue);
    env.warp(t0 + YEAR / 4);
    let ev: LineAccelerated = find_event(&env.council_line(&k, id, ix::AccelerateLine { valid_until: env.valid_until() }.data())).expect("LineAccelerated");
    // A quarter of a year of 10% on 4.0: 0.1 kept, 0.3 forgiven.
    assert_eq!((ev.interest_forgiven, ev.remaining), (300_000, 4 * UNIT + 100_000));
    let line = env.line(&k, id);
    assert_eq!((line.status, line.interest_due, line.accelerated_at), (LineStatus::InCollection, 100_000, t0 + YEAR / 4));
    assert!(line.accelerated_at != 0 && line.acceleration_due_at != 0, "route 2: the council's");

    // Written off once the collection period has run from the mark.
    let council = env.council.insecure_clone();
    env.warp(line.accelerated_at + i64::from(COLLECTION_PERIOD));
    let i = env.write_off_ix(&k, id, &council.pubkey());
    env.expect_err(i, &council, &[], code(PoolError::WriteOffTooEarly));
    env.warp(line.accelerated_at + i64::from(COLLECTION_PERIOD) + 1);
    let i = env.write_off_ix(&k, id, &council.pubkey());
    env.send(i, &council, &[]);
    assert_eq!(env.line(&k, id).status, LineStatus::WrittenOff);
}

#[test]
fn a_scheduled_line_accelerated_keeps_the_interest_of_the_installments_due() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 12, 5_000);
    let (early, _) = env.open_line_with(&k, &borrower, &p, None);
    let (late, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, early, &borrower);
    env.draw(&k, late, &borrower);

    // Accelerated before any installment is due: only what was repaid (nothing) is kept.
    env.council_line(&k, early, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    env.warp(env.now() + i64::from(ACCELERATION_NOTICE));
    let ev: LineAccelerated = find_event(&env.council_line(&k, early, ix::AccelerateLine { valid_until: env.valid_until() }.data())).expect("LineAccelerated");
    let line = env.line(&k, early);
    assert_eq!((line.interest_due, ev.remaining), (0, 6 * UNIT));

    // Three installments due at the mark: their share of the interest is kept.
    let before = env.line(&k, late).interest_due;
    env.warp(p.first_due_at() + 2 * p.interval() - i64::from(ACCELERATION_NOTICE));
    env.council_line(&k, late, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    env.warp(p.first_due_at() + 2 * p.interval());
    env.council_line(&k, late, ix::AccelerateLine { valid_until: env.valid_until() }.data());
    let (kept, _) = elote_pool::math::accelerated_interest(before, 3, 12).unwrap();
    assert_eq!(env.line(&k, late).interest_due, kept);
}

#[test]
fn the_objective_mark_supersedes_a_pending_proposal() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, id, &borrower);
    env.warp(p.first_due_at() + 2 * p.interval() + 1);
    env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    let saved = env.line(&k, id).availability_end_saved;
    let keeper = env.funded(1);
    let i = env.mark_in_collection_ix(&k, id);
    let meta = env.send(i, &keeper, &[]);
    assert!(find_event::<LineInCollection>(&meta).is_some());
    assert!(find_event::<AccelerationWithdrawn>(&meta).is_some(), "the proposal is superseded");
    let line = env.line(&k, id);
    assert_eq!((line.status, line.acceleration_due_at, line.availability_end_saved), (LineStatus::InCollection, 0, saved));
    assert!(line.accelerated_at != 0 && line.acceleration_due_at == 0, "route 1: the objective one");
}

#[test]
fn any_member_or_the_guardian_pauses_and_only_the_council_resumes() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let curator = env.curators[0].insecure_clone();
    env.svm.airdrop(&curator.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let stranger = env.funded(1);
    let guardian = env.funded(1);

    let i = env.pause_ix(&k, &stranger.pubkey());
    env.expect_err(i, &stranger, &[], E_UNAUTHORIZED);
    let i = env.pause_ix(&k, &curator.pubkey());
    let ev: PoolPaused = find_event(&env.send(i, &curator, &[])).expect("PoolPaused");
    assert_eq!((ev.by, env.pool(&k).paused), (curator.pubkey(), true));
    let i = env.council_pool_ix(&k, &curator.pubkey(), ix::Resume { valid_until: env.valid_until() }.data());
    env.expect_err(i, &curator, &[], E_UNAUTHORIZED);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::Resume { valid_until: env.valid_until() }.data());
    assert!(find_event::<PoolResumed>(&env.send(i, &council, &[])).is_some());
    assert!(!env.pool(&k).paused);

    let i = env.pause_ix(&k, &guardian.pubkey());
    env.expect_err(i, &guardian, &[], E_UNAUTHORIZED);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::SetGuardian { guardian: guardian.pubkey(), valid_until: env.valid_until() }.data());
    let ev: GuardianSet = find_event(&env.send(i, &council, &[])).expect("GuardianSet");
    assert_eq!(ev.guardian, guardian.pubkey());
    let i = env.pause_ix(&k, &guardian.pubkey());
    env.send(i, &guardian, &[]);
    assert!(env.pool(&k).paused);
}

#[test]
fn the_pool_limits_and_the_notice_are_the_councils_with_their_floors() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let stranger = env.funded(1);
    let cases: Vec<(Vec<u8>, u32)> = vec![
        (ix::SetMaxLineBps { max_line_bps: 0, valid_until: env.valid_until() }.data(), E_WRONG_AMOUNT),
        (ix::SetMaxLineBps { max_line_bps: BPS + 1, valid_until: env.valid_until() }.data(), E_WRONG_AMOUNT),
        (ix::SetOutstandingCap { max_outstanding_principal: 0, valid_until: env.valid_until() }.data(), E_WRONG_AMOUNT),
        (ix::SetAccelerationNotice { acceleration_notice_seconds: ACCELERATION_NOTICE - 1, valid_until: env.valid_until() }.data(), code(PoolError::AccelerationNoticeTooShort)),
    ];
    for (data, e) in cases {
        let i = env.council_pool_ix(&k, &council.pubkey(), data);
        env.expect_err(i, &council, &[], e);
    }
    let i = env.council_pool_ix(&k, &stranger.pubkey(), ix::SetMaxLineBps { max_line_bps: 500, valid_until: env.valid_until() }.data());
    env.expect_err(i, &stranger, &[], E_UNAUTHORIZED);
    for data in [
        ix::SetMaxLineBps { max_line_bps: 500, valid_until: env.valid_until() }.data(),
        ix::SetOutstandingCap { max_outstanding_principal: 7 * UNIT, valid_until: env.valid_until() }.data(),
        ix::SetAccelerationNotice { acceleration_notice_seconds: 2 * ACCELERATION_NOTICE, valid_until: env.valid_until() }.data(),
    ] {
        let i = env.council_pool_ix(&k, &council.pubkey(), data);
        env.send(i, &council, &[]);
    }
    let pool = env.pool(&k);
    assert_eq!((pool.max_line_bps, pool.max_outstanding_principal, pool.acceleration_notice_seconds), (500, 7 * UNIT, 2 * ACCELERATION_NOTICE));
}

#[test]
fn a_council_line_instruction_refuses_a_multisig_weakened_below_a_majority() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    let members: Vec<(solana_pubkey::Pubkey, u8)> = env.curators.iter().map(|c| (c.pubkey(), PERM_ALL)).collect();
    let multisig = env.multisig;
    put_squads_account(&mut env.svm, &multisig, multisig_data(&solana_pubkey::Pubkey::new_unique(), &solana_pubkey::Pubkey::default(), 1, None, &members));
    expect_line(&mut env, &k, id, |vu| ix::SuspendDraws { valid_until: vu }.data(), PoolError::CouncilInvalid);
}

#[test]
fn a_window_closed_by_date_is_not_suspended() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, id, &borrower, UNIT);
    env.warp(p.term_end);
    expect_line(&mut env, &k, id, |vu| ix::SuspendDraws { valid_until: vu }.data(), PoolError::WindowClosed);
    // A proposal on a line whose window closed by date sets only its deadline.
    env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    let line = env.line(&k, id);
    assert_eq!((line.availability_end, line.availability_end_saved), (p.term_end, 0), "not suspended");
    assert_eq!(line.acceleration_due_at, env.now() + i64::from(ACCELERATION_NOTICE));
}

#[test]
fn an_accelerated_bullet_line_owes_late_interest_from_the_mark_collected_first() {
    use elote_pool::events::{LateInterestCollected, LineRepaid};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.line_params(4 * UNIT, 0);
    p.late_bps = 3_650; // 36.5% a year: 0.1% of the overdue principal a day
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    assert_eq!(env.line(&k, id).late_bps, 3_650, "a bullet line carries its late rate");
    env.draw(&k, id, &borrower);
    env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    env.warp(env.now() + i64::from(ACCELERATION_NOTICE));
    env.council_line(&k, id, ix::AccelerateLine { valid_until: env.valid_until() }.data());
    let mark = env.line(&k, id).accelerated_at;
    assert_eq!(env.arrears(&k, id).late_due, 0, "nothing at the mark itself");

    env.warp(mark + 10 * 86_400);
    // 4.0 overdue for 10 days at 36.5% a year: 0.04.
    let late = 4 * UNIT * 3_650 * 10 * 86_400 / (10_000 * elote_pool::state::SECONDS_PER_YEAR);
    assert_eq!((late, env.arrears(&k, id).late_due), (40_000, 40_000));
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), late);
    let nav = env.nav(&k);
    let i = env.repay_ix(&k, id, &borrower.pubkey(), UNIT);
    let meta = env.send(i, &borrower, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("LateInterestCollected");
    let repaid: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.amount, repaid.principal_part), (late, UNIT - late), "late interest first");
    assert_eq!(env.nav(&k), nav + late, "the late interest raises the NAV");
    assert_eq!(env.line(&k, id).late_collected, late);
}

/// A bullet line past its term owes late interest without any acceleration: it is one installment
/// due at `term_end`, and late interest runs on its unpaid principal from `term_end + grace`, by the
/// same formula as an accelerated bullet line (anchor `term_end - 1`, interval 1, one installment).
/// Up to `term_end + grace` nothing accrues and a payment takes none. The line's terms, and so its
/// `terms_hash`, are what they were.
#[test]
fn a_bullet_line_past_its_term_owes_late_interest_from_the_term_end_plus_grace() {
    use elote_pool::events::{LateInterestCollected, LineRepaid};
    let grace = 5 * 86_400u32;
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(grace);
    let investor = env.investor(&k, 20 * UNIT);
    env.deposit(&k, &investor, 20 * UNIT);
    let council = env.council.insecure_clone();
    let mut p = env.line_params(4 * UNIT, 1_000); // 10% a year for a year: 0.4 of interest
    p.late_bps = 3_650; // 36.5% a year
    let late_one = env.borrower(&k);
    let (id, _) = env.open_line_with(&k, &late_one, &p, None);
    let on_time = env.borrower(&k);
    let (other, _) = env.open_line_with(&k, &on_time, &p, None);
    let hash = env.terms_hash(&k, id, &late_one.pubkey(), &p);
    env.draw(&k, id, &late_one);
    env.draw(&k, other, &on_time);
    let line = env.line(&k, id);
    assert_eq!((line.installments, line.accelerated_at, line.status, line.interest_due), (0, 0, LineStatus::Active, 400_000));
    let start = p.term_end + i64::from(grace);

    // Up to the term end plus grace, to the second, nothing accrues.
    for at in [p.term_end - 1, p.term_end, start] {
        env.warp(at);
        assert_eq!(env.arrears(&k, id).late_due, 0, "at {at}");
    }
    // A payoff at that second takes no late interest.
    let owed = 4 * UNIT + 400_000;
    env.mint_to(&k.asset_mint, &council, &on_time.pubkey(), owed);
    let i = env.repay_ix(&k, other, &on_time.pubkey(), owed);
    let meta = env.send(i, &on_time, &[]);
    assert!(find_event::<LateInterestCollected>(&meta).is_none());
    let line = env.line(&k, other);
    assert_eq!((line.status, line.late_collected, line.late_paid_until), (LineStatus::Repaid, 0, 0));

    // One year later, never accelerated: a year of 36.5% on the 4.0 of principal, 1.46.
    env.warp(start + YEAR);
    let late = 4 * UNIT * 3_650 / 10_000;
    assert_eq!((late, env.arrears(&k, id).late_due), (1_460_000, 1_460_000));
    assert_eq!(env.line(&k, id).accelerated_at, 0);
    env.mint_to(&k.asset_mint, &council, &late_one.pubkey(), late + 400_000);
    // The payment must cover it.
    let i = env.repay_ix(&k, id, &late_one.pubkey(), late - 1);
    env.expect_err(i, &late_one, &[], code(PoolError::LateInterestUnpaid));
    let nav = env.nav(&k);
    let i = env.repay_ix(&k, id, &late_one.pubkey(), owed + late);
    let meta = env.send(i, &late_one, &[]);
    let ev: LateInterestCollected = find_event(&meta).expect("LateInterestCollected");
    let repaid: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((ev.amount, repaid.interest_part, repaid.principal_part, repaid.remaining), (late, 400_000, 4 * UNIT, 0));
    assert_eq!(env.nav(&k), nav + 400_000 + late, "the interest and the late interest raise the NAV");
    let line = env.line(&k, id);
    assert_eq!((line.status, line.late_collected, line.late_paid_until, line.accelerated_at), (LineStatus::Repaid, late, start + YEAR, 0));
    assert_eq!(line.terms_hash, hash, "the terms are unchanged");
}

#[test]
fn the_pause_also_blocks_approvals() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 0);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    env.set_paused(&k, true);
    let council = env.council.insecure_clone();
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    let i = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    env.expect_err(i, &council, &[], E_PAUSED);
    env.set_paused(&k, false);
    env.approve_line(&k, id, &borrower.pubkey(), &p);
}
