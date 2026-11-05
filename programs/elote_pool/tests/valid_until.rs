//! `valid_until` on every council instruction: Squads proposals never expire and execute even when
//! stale, so the program refuses a council decision past its date (`ProposalExpired`; equal is
//! accepted) and one dated more than `MAX_PROPOSAL_HORIZON_SECONDS` ahead (`ProposalTooFar`; equal
//! is accepted). The harness builders pass `env.valid_until()`, one day ahead unless a test sets it.

mod common;

use common::*;
use elote_pool::errors::PoolError;
use solana_instruction::Instruction;
use solana_signer::Signer;

const E_PROPOSAL_EXPIRED: u32 = ERR_BASE + PoolError::ProposalExpired as u32;
const E_PROPOSAL_TOO_FAR: u32 = ERR_BASE + PoolError::ProposalTooFar as u32;
const DAY: i64 = 86_400;

/// Sends `build()` with `valid_until = now - 1` (refused) and then `= now` (accepted).
fn expires(env: &mut Env, signer: &Keypair, build: impl Fn(&Env) -> Instruction) {
    env.valid_until = Some(env.now() - 1);
    let ix = build(env);
    env.expect_err(ix, signer, &[], E_PROPOSAL_EXPIRED);
    env.valid_until = Some(env.now());
    let ix = build(env);
    env.send(ix, signer, &[]);
    env.valid_until = None;
}

fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

#[test]
fn approve_line_expires_and_a_stale_proposal_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 0);
    let (stale, _) = env.request_line_with(&k, &borrower, &p, None);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);

    // A proposal approved with one day to execute, executed two days later.
    env.valid_until = Some(env.now() + DAY);
    let hash = env.terms_hash(&k, stale, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council.pubkey(), stale, p.expediente_root, hash);
    env.valid_until = None;
    env.warp(env.now() + 2 * DAY);
    env.expect_err(ix, &council, &[], E_PROPOSAL_EXPIRED);

    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    expires(&mut env, &council, |env| env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash));
}

#[test]
fn the_council_setters_rotation_and_fix_expire() {
    use anchor_lang::InstructionData;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    expires(&mut env, &council, |env| {
        env.council_pool_ix(&k, &council.pubkey(), elote_pool::instruction::SetMaxLineBps { max_line_bps: 500, valid_until: env.valid_until() }.data())
    });
    assert_eq!(env.pool(&k).max_line_bps, 500);
    let today = env.today();
    expires(&mut env, &council, |env| env.post_fix_ix(&k, &council.pubkey(), 183_740, today));
    let next = solana_pubkey::Pubkey::new_unique();
    expires(&mut env, &council, |env| env.propose_council_ix(&k, &council.pubkey(), &next, elote_pool::state::COUNCIL_SQUADS));
    assert_eq!(env.pool(&k).pending_council, next);
}

#[test]
fn write_off_and_the_councils_cancellation_expire() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 0);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, id, &borrower);

    // Repay one pagaré in full, then the council cancels it.
    let (paid, _) = env.open_line_with(&k, &borrower, &env.line_params(UNIT, 0), None);
    env.draw(&k, paid, &borrower);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    env.repay(&k, paid, &borrower, UNIT);
    expires(&mut env, &council, |env| env.cancel_pagare_ix(&k, paid, 0, &council.pubkey(), &council.pubkey(), RECEIPT_HASH));
    assert_eq!(env.pagare(&k, paid, 0).status, elote_pool::state::PAGARE_PAID);

    env.warp(p.term_end + 1);
    expires(&mut env, &council, |env| env.write_off_ix(&k, id, &council.pubkey()));
    assert_eq!(env.line(&k, id).status, elote_pool::state::LineStatus::WrittenOff);
}

/// Sends `build()` with `valid_until = now + horizon + 1` (refused) and then `= now + horizon`
/// (accepted).
fn too_far(env: &mut Env, signer: &Keypair, build: impl Fn(&Env) -> Instruction) {
    use elote_pool::state::MAX_PROPOSAL_HORIZON_SECONDS;
    env.valid_until = Some(env.now() + MAX_PROPOSAL_HORIZON_SECONDS + 1);
    let ix = build(env);
    env.expect_err(ix, signer, &[], E_PROPOSAL_TOO_FAR);
    env.valid_until = Some(env.now() + MAX_PROPOSAL_HORIZON_SECONDS);
    let ix = build(env);
    env.send(ix, signer, &[]);
    env.valid_until = None;
}

#[test]
fn a_council_decision_dated_beyond_the_horizon_is_refused() {
    use anchor_lang::InstructionData;
    assert_eq!(elote_pool::state::MAX_PROPOSAL_HORIZON_SECONDS, 15 * DAY);
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 0);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    too_far(&mut env, &council, |env| env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash));
    too_far(&mut env, &council, |env| {
        env.council_pool_ix(&k, &council.pubkey(), elote_pool::instruction::SetMaxLineBps { max_line_bps: 500, valid_until: env.valid_until() }.data())
    });
    assert_eq!(env.pool(&k).max_line_bps, 500);
    // i64::MAX, the date of a proposal that never expires, is beyond any horizon.
    env.valid_until = Some(i64::MAX);
    let ix = env.council_line_ix(&k, id, &council.pubkey(), elote_pool::instruction::SuspendDraws { valid_until: env.valid_until() }.data());
    env.expect_err(ix, &council, &[], E_PROPOSAL_TOO_FAR);
}
