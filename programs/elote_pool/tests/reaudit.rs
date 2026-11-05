//! Regression tests for the findings of the 2026-11-02 re-audit
//! (docs/audits/2026-11-02-elote-pool-reaudit.md).

mod common;

use common::*;
use elote_pool::events::PagarePayment;
use elote_pool::state::{LineStatus, KIND_ABONO};
use solana_signer::Signer;

fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

/// Overwrites one sidecar's `entry_count`, leaving every other byte as the program wrote it.
fn set_entry_count(env: &mut Env, k: &PoolKeys, line_id: u64, draw_index: u16, count: u16) {
    let sidecar = pagare_pda(&line_pda(&k.pool, line_id).0, draw_index).0;
    let mut acc = env.account(&sidecar);
    let mut state = env.pagare(k, line_id, draw_index);
    state.entry_count = count.into();
    let mut data = Vec::new();
    anchor_lang::AccountSerialize::try_serialize(&state, &mut data).unwrap();
    acc.data[..data.len()].copy_from_slice(&data);
    env.svm.set_account(sidecar, acc).unwrap();
}

/// N-01: a payment that applies no principal to a pagaré writes no entry on it, so a stranger's
/// one-unit payments (interest first) cannot grow the registry; the payment is still recorded in
/// `PagarePayment`. A payment that moves principal still writes its `Abono`.
#[test]
fn n01_an_interest_only_payment_appends_no_registry_entry() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, 4 * UNIT);
    let council = env.council.insecure_clone();
    let stranger = env.funded(1);
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), 1_000);

    let before = env.pagare(&k, line_id, 0);
    for _ in 0..5 {
        let ix = env.repay_ix(&k, line_id, &stranger.pubkey(), 1);
        let meta = env.send(ix, &stranger, &[]);
        let ev: PagarePayment = find_event(&meta).expect("the payment is still recorded");
        assert_eq!((ev.payer, ev.interest_part, ev.principal_part), (stranger.pubkey(), 1, 0));
        assert!(ev.entries.is_empty(), "no entry on a pagaré the payment applied no principal to");
    }
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.entry_count, after.log_root), (before.entry_count, before.log_root));

    // Interest cleared plus one unit of principal: one Abono.
    let line = env.line(&k, line_id);
    let amount = line.interest_due - line.interest_repaid + 1;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), amount);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), amount);
    let meta = env.send(ix, &borrower, &[]);
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.principal_applied)).collect();
    assert_eq!(e, vec![(0, KIND_ABONO, 1)]);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, before.entry_count + 1);
}

/// N-01: the entry counter does not saturate in any practical range: a sidecar past 65 535
/// entries still takes payments, and the borrower can pay off the line.
#[test]
fn n01_the_entry_counter_is_wide_enough() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, 4 * UNIT);
    let council = env.council.insecure_clone();
    set_entry_count(&mut env, &k, line_id, 0, u16::MAX);

    let line = env.line(&k, line_id);
    let owed = line.drawn - line.principal_repaid + line.interest_due - line.interest_repaid;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), owed);
    env.send(ix, &borrower, &[]);
    assert_eq!(u64::from(env.pagare(&k, line_id, 0).entry_count), u64::from(u16::MAX) + 1);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// N-03: the procurador is a third party: not the council (the holder), a curator, the guardian
/// or the line's borrower.
#[test]
fn n03_the_council_its_curators_the_guardian_and_the_borrower_cannot_be_procurador() {
    use elote_pool::state::KIND_PROCURACION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();
    let curator = env.curators[0].pubkey();
    for to in [a, curator, borrower.pubkey()] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, to, [0x44; 32]);
        env.expect_err(ix, &council, &[], E_ENDORSEE_INVALID);
    }
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 1, "nothing was appended");

    // A pool with a guardian refuses it too.
    let guardian = env.funded(1);
    let asset = env.create_mint(&a, 6);
    let k = env.pool_keys_for(&asset);
    let mut params = env.init_params(BPS, COLLECTION_PERIOD);
    params.guardian = guardian.pubkey();
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), params);
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 20 * UNIT);
    env.deposit(&k, &investor, 20 * UNIT);
    let borrower = env.borrower(&k);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, guardian.pubkey(), [0x44; 32]);
    env.expect_err(ix, &council, &[], E_ENDORSEE_INVALID);
}

/// N-04: the payer's `receipt_hash` on the PAGADO mark is provisional. The holder's annotation
/// replaces it while the burn is still owed; after the burn the
/// field is frozen with the receipt the cancellation rested on.
#[test]
fn n04_the_holder_corrects_the_payment_receipt_until_the_burn() {
    use elote_pool::state::{PAGARE_PAID, PAGARE_PAID_PENDING_CANCEL};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();

    // A stranger's payment covers pagaré 0 with a hash of its own.
    let stranger = env.funded(1);
    let amount = env.line(&k, line_id).interest_due + UNIT;
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), amount);
    let sidecars = env.payment_sidecars(&k, line_id, amount);
    let ix = env.repay_ix_full(&k, line_id, &stranger.pubkey(), amount, [0x66; 32], &sidecars);
    env.send(ix, &stranger, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.receipt_hash), (PAGARE_PAID_PENDING_CANCEL, [0x66; 32]));
    let payment_entry = p0.entry_count;

    // The holder's replaces the provisional hash.
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.receipt_hash, p0.entry_count), (RECEIPT_HASH, payment_entry + 1));

    // After the burn the field is frozen; the annotation still appends.
    env.cancel_pagare(&k, line_id, 0);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, [0x88; 32]);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(p0.status, PAGARE_PAID);
    assert_eq!(p0.receipt_hash, RECEIPT_HASH);
    assert_eq!(p0.last_kind, elote_pool::state::KIND_RECIBO);
}

