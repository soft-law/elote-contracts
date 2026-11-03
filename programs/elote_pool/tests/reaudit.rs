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

/// N-03: the procurador is a third party: neither the attestor (whose signature the procurador's
/// cancellation also needs) nor the council (the holder) may be named.
#[test]
fn n03_the_attestor_and_the_admin_cannot_be_procurador() {
    use elote_pool::state::KIND_PROCURACION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let a = council.pubkey();
    for to in [attestor.pubkey(), a] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, to, [0x44; 32]);
        env.expect_err(ix, &council, &[&attestor], E_ENDORSEE_INVALID);
    }
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 1, "nothing was appended");
}

/// N-04: the payer's `receipt_hash` on the PAGADO mark is provisional. The holder's annotation
/// replaces it while the burn is still owed; the attestor's only appends; after the burn the
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
    let attestor = env.attestor.insecure_clone();
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

    // The attestor's annotation appends only.
    let ix = env.annotate_payment_ix(&k, line_id, 0, &attestor.pubkey(), payment_entry, [0x77; 32]);
    env.send(ix, &council, &[&attestor]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.receipt_hash, p0.entry_count), ([0x66; 32], payment_entry + 1));

    // The holder's replaces the provisional hash.
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.receipt_hash, p0.entry_count), (RECEIPT_HASH, payment_entry + 2));

    // After the burn the field is frozen; the annotation still appends.
    env.cancel_pagare(&k, line_id, 0);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, [0x88; 32]);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(p0.status, PAGARE_PAID);
    assert_eq!(p0.receipt_hash, RECEIPT_HASH);
    assert_eq!(p0.last_kind, elote_pool::state::KIND_RECIBO);
}

/// N-06: the holder replaces a pending cancellation request, so a wrong receipt does not block
/// the backstop. The replacement takes the new receipt and restarts the wait from now, but never
/// brings the deadline earlier than the one it replaces, even after the delay was shortened.
#[test]
fn n06_the_holder_replaces_a_pending_request_without_shortening_it() {
    use elote_pool::events::CancellationRequested;
    use elote_pool::state::{KIND_SOLICITUD_CANCELACION, PAGARE_PAID};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, interest + UNIT);
    let a = council.pubkey();
    let short = i64::from(CANCEL_DELAY);
    let long = 3 * short;

    // A request with the wrong receipt, under a long delay.
    let ix = env.set_cancel_delay_ix(&k, &a, 3 * CANCEL_DELAY);
    env.send(ix, &council, &[]);
    let ix = env.request_cancellation_ix(&k, line_id, 0, &a, [0x66; 32]);
    env.send(ix, &council, &[]);
    let t1 = env.now();
    assert_eq!(env.pagare(&k, line_id, 0).cancel_after, t1 + long);

    // Replaced later with the right receipt: the wait restarts from now.
    env.warp(t1 + 100);
    let entries = env.pagare(&k, line_id, 0).entry_count;
    let ix = env.request_cancellation_ix(&k, line_id, 0, &a, RECEIPT_HASH);
    let meta = env.send(ix, &council, &[]);
    let t2 = env.now();
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.cancel_after, p0.requested_receipt_hash), (t2 + long, RECEIPT_HASH));
    assert_eq!((p0.entry_count, p0.last_kind), (entries + 1, KIND_SOLICITUD_CANCELACION));
    let ev: CancellationRequested = find_event(&meta).expect("CancellationRequested");
    assert_eq!((ev.receipt_hash, ev.cancel_after), (RECEIPT_HASH, t2 + long));

    // The delay is shortened; a replacement keeps the later deadline it replaces.
    let ix = env.set_cancel_delay_ix(&k, &a, CANCEL_DELAY);
    env.send(ix, &council, &[]);
    env.warp(t2 + 100);
    let ix = env.request_cancellation_ix(&k, line_id, 0, &a, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    let t3 = env.now();
    assert!(t3 + short < t2 + long);
    assert_eq!(env.pagare(&k, line_id, 0).cancel_after, t2 + long, "never shortened");

    // The old receipt no longer cancels; the new one does, after the deadline.
    env.warp(t2 + long + 1);
    let ix = env.cancel_pagare_alone_ix(&k, line_id, 0, &a, &a, [0x66; 32]);
    env.expect_err(ix, &council, &[], E_ATTESTOR_REQUIRED);
    let ix = env.cancel_pagare_alone_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
}
