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

/// Inverted 2026-11-08 (decision 38, LGTOC 17): a payment of accessories alone now leaves its
/// `Abono` on the oldest outstanding pagaré, stating zero principal, the accessories applied and
/// the balance. The registry grows one entry per real payment — the chain is constant-size, so
/// that is the record working, not a risk (N-01's remedy reversed; the u32 counter stays).
#[test]
fn n01_an_interest_only_payment_appends_its_abono() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, 4 * UNIT);
    env.warp(p.term_end);
    let council = env.council.insecure_clone();
    let stranger = env.funded(1);
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), 1_000);

    let before = env.pagare(&k, line_id, 0);
    for i in 0..5u32 {
        let ix = env.repay_ix(&k, line_id, &stranger.pubkey(), 1);
        let meta = env.send(ix, &stranger, &[]);
        let ev: PagarePayment = find_event(&meta).expect("the payment is recorded");
        assert_eq!((ev.payer, ev.interest_part, ev.principal_part), (stranger.pubkey(), 1, 0));
        let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.principal_applied, e.accessory_applied, e.by_borrower)).collect();
        assert_eq!(e, vec![(0, KIND_ABONO, 0, 1, false)], "the accessory-only Abono, by a third party");
        assert_eq!(env.pagare(&k, line_id, 0).entry_count, before.entry_count + i + 1);
    }
    let after = env.pagare(&k, line_id, 0);
    assert_ne!(after.log_root, before.log_root);

    // Interest cleared plus one unit of principal: one Abono stating both. A year on 4.0 at 10%
    // accrued 400_000, of which the stranger paid 5.
    let rest = 400_000 - 5;
    let amount = rest + 1;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), amount);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), amount);
    let meta = env.send(ix, &borrower, &[]);
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.principal_applied, e.accessory_applied, e.by_borrower)).collect();
    assert_eq!(e, vec![(0, KIND_ABONO, 1, rest, true)]);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, before.entry_count + 6);
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

    // Paid in the second it was drawn: the principal, and no interest accrued yet.
    let owed = 4 * UNIT;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), owed);
    env.send(ix, &borrower, &[]);
    assert_eq!(u64::from(env.pagare(&k, line_id, 0).entry_count), u64::from(u16::MAX) + 1);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// N-03: the procurador is a third party: not the council (the holder), a curator or the line's
/// borrower. (The guardian role was retired by decision 39.)
#[test]
fn n03_the_council_its_curators_and_the_borrower_cannot_be_procurador() {
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

}

/// Inverted 2026-11-08 (decision 38, LGTOC 41): the payer's `receipt_hash` on the PAGADO mark is
/// provisional and now stays as written — the holder's correction is the `Recibo` entry itself,
/// and no field is ever rewritten, before or after the delivery.
#[test]
fn n04_the_holders_correction_is_the_recibo_entry_not_a_rewrite() {
    use elote_pool::state::PAGARE_PAID;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();

    // A stranger's payment covers pagaré 0 with a hash of its own and settles the line, whose
    // borrower renounced the rest: the delivery below needs a line that owes nothing (decision 49).
    let ix = env.renounce_availability_ix(&k, line_id, &borrower.pubkey());
    env.send(ix, &borrower, &[]);
    let stranger = env.funded(1);
    let amount = env.payoff(&k, line_id);
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), amount);
    let sidecars = env.payment_sidecars(&k, line_id, amount);
    let ix = env.repay_ix_full(&k, line_id, &stranger.pubkey(), amount, [0x66; 32], &sidecars);
    env.send(ix, &stranger, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.receipt_hash), (PAGARE_PAID, [0x66; 32]));
    assert_eq!(env.line(&k, line_id).status, elote_pool::state::LineStatus::Repaid);
    let payment_entry = p0.entry_count;

    // The holder's correction appends; the field stands.
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.receipt_hash, p0.entry_count), ([0x66; 32], payment_entry + 1));

    // After the delivery too: the annotation still appends, nothing is rewritten.
    env.deliver_pagare(&k, line_id, 0);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &a, payment_entry, [0x88; 32]);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.receipt_hash, p0.last_kind), (PAGARE_PAID, [0x66; 32], elote_pool::state::KIND_RECIBO));
}


