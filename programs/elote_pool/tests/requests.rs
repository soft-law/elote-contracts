//! Requests and the council's approval: `request_line`, `approve_line`, `reject_line`,
//! `withdraw_request`.

mod common;

use common::*;
use elote_pool::errors::PoolError;
use elote_pool::events::{LineApproved, LineRejected, LineRequested, RequestWithdrawn};
use elote_pool::state::{LineStatus, REQUEST_TTL_SECONDS};
use solana_signer::Signer;

fn code(e: PoolError) -> u32 {
    ERR_BASE + e as u32
}

/// A funded pool, a borrower and the default bullet terms.
fn setup(env: &mut Env) -> (PoolKeys, Keypair, LineParams) {
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    (k, borrower, p)
}

#[test]
fn a_request_stores_the_terms_and_the_payer_funds_it() {
    let mut env = Env::new();
    let (k, borrower, p) = setup(&mut env);
    let payer = env.funded(1);
    let before = env.account(&borrower.pubkey()).lamports;
    let ix = env.request_line_ix(&k, &payer.pubkey(), 0, &borrower.pubkey(), &p, None);
    let meta = env.send(ix, &payer, &[&borrower]);
    assert_eq!(env.account(&borrower.pubkey()).lamports, before, "the borrower pays nothing");

    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    let line = env.line(&k, 0);
    assert_eq!((line.status, line.payer, line.requested_at, line.approved_at), (LineStatus::Requested, payer.pubkey(), env.now(), 0));
    assert_eq!(line.availability_end, 0, "nothing can be drawn before the approval");
    assert_eq!((ev.terms_hash, line.terms_hash), (env.terms_hash(&k, 0, &borrower.pubkey(), &p), ev.terms_hash));
    assert_eq!(env.pool(&k).line_count, 1);

    let ix = env.draw_ix_amount(&k, 0, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
    env.expect_err_with_attestor(ix, &borrower, E_LINE_STATUS);
}

#[test]
fn the_council_approves_exactly_the_requested_expediente_and_terms() {
    let mut env = Env::new();
    let (k, borrower, p) = setup(&mut env);
    let council = env.council.insecure_clone();
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);

    let ix = env.approve_line_ix(&k, &council.pubkey(), id, [1; 32], hash);
    env.expect_err(ix, &council, &[], code(PoolError::ExpedienteMismatch));
    let mut other = p;
    other.principal += 1;
    let wrong = env.terms_hash(&k, id, &borrower.pubkey(), &other);
    let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, wrong);
    env.expect_err(ix, &council, &[], code(PoolError::ExpedienteMismatch));
    let ix = env.approve_line_ix(&k, &borrower.pubkey(), id, p.expediente_root, hash);
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);
    let mut ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    let multisig = env.pool(&k).multisig;
    ix.accounts.iter_mut().find(|m| m.pubkey == multisig).unwrap().pubkey = env.creator.pubkey();
    env.expect_err(ix, &council, &[], code(PoolError::CouncilInvalid));

    let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    let meta = env.send(ix, &council, &[]);
    let ev: LineApproved = find_event(&meta).expect("LineApproved");
    assert_eq!((ev.line_id, ev.expediente_root, ev.terms_hash, ev.availability_end), (id, p.expediente_root, hash, p.term_end));
    let line = env.line(&k, id);
    assert_eq!((line.status, line.approved_at), (LineStatus::Approved, env.now()));

    let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    env.draw(&k, id, &borrower);
}

#[test]
fn a_request_expires_after_fifteen_days() {
    let mut env = Env::new();
    let (k, borrower, mut p) = setup(&mut env);
    p.term_end += REQUEST_TTL_SECONDS;
    let council = env.council.insecure_clone();
    let (on_time, _) = env.request_line_with(&k, &borrower, &p, None);
    let (late, _) = env.request_line_with(&k, &borrower, &p, None);
    let requested_at = env.line(&k, late).requested_at;

    env.warp(requested_at + REQUEST_TTL_SECONDS);
    env.approve_line(&k, on_time, &borrower.pubkey(), &p);
    env.warp(requested_at + REQUEST_TTL_SECONDS + 1);
    let hash = env.terms_hash(&k, late, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council.pubkey(), late, p.expediente_root, hash);
    env.expect_err(ix, &council, &[], code(PoolError::RequestExpired));

    // The borrower withdraws the expired request.
    let ix = env.withdraw_request_ix(&k, &borrower.pubkey(), late, &council.pubkey());
    env.send(ix, &borrower, &[]);
}

#[test]
fn a_curator_cannot_borrow_from_the_council() {
    let mut env = Env::new();
    let (k, _, p) = setup(&mut env);
    let curator = env.curators[1].insecure_clone();
    env.svm.airdrop(&curator.pubkey(), LAMPORTS_PER_SOL).unwrap();
    env.create_ata(&curator.pubkey(), &k.asset_mint);
    let (id, _) = env.request_line_with(&k, &curator, &p, None);
    let council = env.council.insecure_clone();
    let hash = env.terms_hash(&k, id, &curator.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    env.expect_err(ix, &council, &[], code(PoolError::BorrowerIsCurator));
}

#[test]
fn reject_and_withdraw_close_the_request_and_return_the_rent_to_the_payer() {
    let mut env = Env::new();
    let (k, borrower, p) = setup(&mut env);
    let council = env.council.insecure_clone();
    let payer = env.funded(1);
    let stranger = env.funded(1);

    for id in 0..2u64 {
        let ix = env.request_line_ix(&k, &payer.pubkey(), id, &borrower.pubkey(), &p, None);
        env.send(ix, &payer, &[&borrower]);
    }
    let line0 = line_pda(&k.pool, 0).0;
    let rent = env.account(&line0).lamports;
    let funded = env.account(&payer.pubkey()).lamports;

    let ix = env.reject_line_ix(&k, &borrower.pubkey(), 0, &payer.pubkey());
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);
    let ix = env.reject_line_ix(&k, &council.pubkey(), 0, &stranger.pubkey());
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
    let ix = env.reject_line_ix(&k, &council.pubkey(), 0, &payer.pubkey());
    let meta = env.send(ix, &council, &[]);
    let ev: LineRejected = find_event(&meta).expect("LineRejected");
    assert_eq!((ev.pool, ev.line_id), (k.pool, 0));
    assert!(env.svm.get_account(&line0).is_none_or(|a| a.lamports == 0), "closed");
    assert_eq!(env.account(&payer.pubkey()).lamports, funded + rent);

    let ix = env.withdraw_request_ix(&k, &stranger.pubkey(), 1, &payer.pubkey());
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);
    let ix = env.withdraw_request_ix(&k, &borrower.pubkey(), 1, &payer.pubkey());
    let meta = env.send(ix, &borrower, &[]);
    let ev: RequestWithdrawn = find_event(&meta).expect("RequestWithdrawn");
    assert_eq!(ev.line_id, 1);
    assert_eq!(env.account(&payer.pubkey()).lamports, funded + 2 * rent);

    // The ids are not reused: the next request is line 2.
    assert_eq!(env.pool(&k).line_count, 2);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    assert_eq!(id, 2);
}

#[test]
fn an_approved_line_is_neither_rejected_nor_withdrawn() {
    let mut env = Env::new();
    let (k, borrower, p) = setup(&mut env);
    let council = env.council.insecure_clone();
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    let ix = env.reject_line_ix(&k, &council.pubkey(), id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    let ix = env.withdraw_request_ix(&k, &borrower.pubkey(), id, &council.pubkey());
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

#[test]
fn a_paused_pool_takes_no_requests() {
    let mut env = Env::new();
    let (k, borrower, p) = setup(&mut env);
    env.set_paused(&k, true);
    let council = env.council.insecure_clone();
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &[&borrower], E_PAUSED);
}
