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

fn expect_line(env: &mut Env, k: &PoolKeys, line_id: u64, data: Vec<u8>, e: PoolError) {
    let council = env.council.insecure_clone();
    let i = env.council_line_ix(k, line_id, &council.pubkey(), data);
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

    env.council_line(&k, id, ix::SuspendDraws {}.data());
    let line = env.line(&k, id);
    assert_eq!((line.availability_end, line.availability_end_saved), (env.now(), original), "suspended: saved != 0");
    let i = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.expect_err(i, &borrower, &[], E_AVAILABILITY_ENDED);
    env.warp(env.now() + 100);
    env.council_line(&k, id, ix::SuspendDraws {}.data());
    assert_eq!(env.line(&k, id).availability_end_saved, original, "a second suspension keeps the original");

    let ev: LineDrawsResumed = find_event(&env.council_line(&k, id, ix::ResumeDraws {}.data())).expect("LineDrawsResumed");
    assert_eq!(ev.availability_end, original);
    let line = env.line(&k, id);
    assert_eq!((line.availability_end, line.availability_end_saved), (original, 0));
    env.draw_amount(&k, id, &borrower, UNIT);
    expect_line(&mut env, &k, id, ix::ResumeDraws {}.data(), PoolError::DrawsNotSuspended);
}

#[test]
fn a_proposal_suspends_draws_and_withdrawing_it_does_not_resume_them() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (undrawn, _) = env.open_line_with(&k, &borrower, &p, None);
    expect_line(&mut env, &k, undrawn, ix::ProposeAcceleration {}.data(), PoolError::LineStatus);

    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, id, &borrower, UNIT);
    let original = env.line(&k, id).availability_end;
    let ev: AccelerationProposed = find_event(&env.council_line(&k, id, ix::ProposeAcceleration {}.data())).expect("AccelerationProposed");
    let due_at = env.now() + i64::from(ACCELERATION_NOTICE);
    assert_eq!((ev.line_id, ev.due_at), (id, due_at));
    let line = env.line(&k, id);
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (due_at, original));
    let i = env.draw_ix_amount(&k, id, &borrower.pubkey(), UNIT);
    env.expect_err(i, &borrower, &[], E_AVAILABILITY_ENDED);
    expect_line(&mut env, &k, id, ix::ProposeAcceleration {}.data(), PoolError::AccelerationState);
    expect_line(&mut env, &k, id, ix::ResumeDraws {}.data(), PoolError::AccelerationState);

    let meta = env.council_line(&k, id, ix::WithdrawAcceleration {}.data());
    assert!(find_event::<AccelerationWithdrawn>(&meta).is_some());
    let line = env.line(&k, id);
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (0, original), "still suspended");
    expect_line(&mut env, &k, id, ix::WithdrawAcceleration {}.data(), PoolError::AccelerationState);
    env.council_line(&k, id, ix::ResumeDraws {}.data());
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
    env.council_line(&k, id, ix::ProposeAcceleration {}.data());
    env.warp(t0 + YEAR / 4 - 1);
    expect_line(&mut env, &k, id, ix::AccelerateLine {}.data(), PoolError::AccelerationNotDue);
    env.warp(t0 + YEAR / 4);
    let ev: LineAccelerated = find_event(&env.council_line(&k, id, ix::AccelerateLine {}.data())).expect("LineAccelerated");
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
    env.council_line(&k, early, ix::ProposeAcceleration {}.data());
    env.warp(env.now() + i64::from(ACCELERATION_NOTICE));
    let ev: LineAccelerated = find_event(&env.council_line(&k, early, ix::AccelerateLine {}.data())).expect("LineAccelerated");
    let line = env.line(&k, early);
    assert_eq!((line.interest_due, ev.remaining), (0, 6 * UNIT));

    // Three installments due at the mark: their share of the interest is kept.
    let before = env.line(&k, late).interest_due;
    env.warp(p.first_due_at() + 2 * p.interval() - i64::from(ACCELERATION_NOTICE));
    env.council_line(&k, late, ix::ProposeAcceleration {}.data());
    env.warp(p.first_due_at() + 2 * p.interval());
    env.council_line(&k, late, ix::AccelerateLine {}.data());
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
    env.council_line(&k, id, ix::ProposeAcceleration {}.data());
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
    let i = env.council_pool_ix(&k, &curator.pubkey(), ix::Resume {}.data());
    env.expect_err(i, &curator, &[], E_UNAUTHORIZED);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::Resume {}.data());
    assert!(find_event::<PoolResumed>(&env.send(i, &council, &[])).is_some());
    assert!(!env.pool(&k).paused);

    let i = env.pause_ix(&k, &guardian.pubkey());
    env.expect_err(i, &guardian, &[], E_UNAUTHORIZED);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::SetGuardian { guardian: guardian.pubkey() }.data());
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
        (ix::SetMaxLineBps { max_line_bps: 0 }.data(), E_WRONG_AMOUNT),
        (ix::SetMaxLineBps { max_line_bps: BPS + 1 }.data(), E_WRONG_AMOUNT),
        (ix::SetOutstandingCap { max_outstanding_principal: 0 }.data(), E_WRONG_AMOUNT),
        (ix::SetAccelerationNotice { acceleration_notice_seconds: ACCELERATION_NOTICE - 1 }.data(), code(PoolError::AccelerationNoticeTooShort)),
    ];
    for (data, e) in cases {
        let i = env.council_pool_ix(&k, &council.pubkey(), data);
        env.expect_err(i, &council, &[], e);
    }
    let i = env.council_pool_ix(&k, &stranger.pubkey(), ix::SetMaxLineBps { max_line_bps: 500 }.data());
    env.expect_err(i, &stranger, &[], E_UNAUTHORIZED);
    for data in [
        ix::SetMaxLineBps { max_line_bps: 500 }.data(),
        ix::SetOutstandingCap { max_outstanding_principal: 7 * UNIT }.data(),
        ix::SetAccelerationNotice { acceleration_notice_seconds: 2 * ACCELERATION_NOTICE }.data(),
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
    expect_line(&mut env, &k, id, ix::SuspendDraws {}.data(), PoolError::CouncilInvalid);
}
