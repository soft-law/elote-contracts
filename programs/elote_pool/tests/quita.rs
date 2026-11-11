//! Decision 51: the council's quita reaches principal, and a written-off line takes recoveries.
//!
//! A quita of principal extinguishes that debt (CCF 2209): its cost, at each pagaré's own FIX,
//! leaves the book as a loss at that moment, as a write-off of that amount would. A payment on a
//! written-off line follows the same order (late interest, interest, principal); the principal it
//! recovers is income against the loss already recognised, so it comes off `written_off` and never
//! off `outstanding_principal` a second time. Either way the títulos leave once the line owes
//! nothing. Expected figures are worked by hand in the comments, never read back from the program.

mod common;

use common::*;
use elote_pool::errors::PoolError;
use elote_pool::events::{CondonationEntry, FxRealized, LineRepaid, LossRecovered, PagareDelivered};
use elote_pool::state::{
    LineStatus, KIND_CONDONACION, KIND_PAGADO, PAGARE_FLAG_CONDONED, PAGARE_FLAG_DELIVERED, PAGARE_FLAG_PAID_BY_AVAL, PAGARE_OUTSTANDING,
    PAGARE_PAID,
};
use solana_signer::Signer;

const DOCUMENT: [u8; 32] = [0xd5; 32];
const E_BALANCE_OUTSTANDING: u32 = ERR_BASE + PoolError::BalanceOutstanding as u32;
/// A sixth of a year, the interval of a six-installment schedule: 5,256,000 s.
const SIXTH: i64 = YEAR / 6;

fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool_capped(BPS);
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

/// The pool's books, every field stated by the test, and the NAV recomputed from them by hand:
/// `vault + outstanding - impaired * provision_bps / BPS`; the invariant `impaired <= outstanding`.
fn books(env: &Env, k: &PoolKeys, vault: u64, outstanding: u64, impaired: u64, written_off: u64) -> u64 {
    let pool = env.pool(k);
    assert_eq!(
        (env.token_balance(&k.vault), pool.outstanding_principal, pool.impaired_principal, pool.written_off),
        (vault, outstanding, impaired, written_off),
        "vault, outstanding, impaired, written off"
    );
    assert!(impaired <= outstanding);
    let nav = vault + outstanding - (u128::from(impaired) * u128::from(pool.provision_bps) / 10_000) as u64;
    assert_eq!(env.nav(k), nav);
    nav
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// What a `Condonacion` entry commits: the domain, the record, the principal condoned on this
/// pagaré and its unpaid principal after.
fn condonation(document: &[u8; 32], principal: u64, balance: u64) -> [u8; 32] {
    sha(&[b"elote.quita.v1", document, &principal.to_le_bytes(), &balance.to_le_bytes()])
}

/// A payment entry's annotation (decision 38).
fn payment(receipt: &[u8; 32], principal: u64, accessory: u64, balance: u64, by_borrower: bool) -> [u8; 32] {
    sha(&[b"elote.pago.v2", receipt, &principal.to_le_bytes(), &accessory.to_le_bytes(), &balance.to_le_bytes(), &[u8::from(by_borrower)]])
}

/// One link of the registry: `sha256(root || sha256(asset || kind || from || to || hash || at || n))`.
fn chain(root: &[u8; 32], asset: &solana_pubkey::Pubkey, kind: u8, from: &solana_pubkey::Pubkey, to: &solana_pubkey::Pubkey, hash: &[u8; 32], at: i64, n: u32) -> [u8; 32] {
    let entry = sha(&[asset.as_ref(), &[kind], from.as_ref(), to.as_ref(), hash, &at.to_le_bytes(), &n.to_le_bytes()]);
    sha(&[root, &entry])
}

/// A pool of 10.0 and a six-installment line of `draws` (12%, late interest 100%) drawn at t0 and
/// marked in collection at `t0 + 4 * SIXTH + 1`, when the third missed installment is past due.
/// Returns `(keys, borrower, line_id, t0, mark)`.
fn in_collection(env: &mut Env, draws: &[u64]) -> (PoolKeys, Keypair, u64, i64, i64) {
    let k = pool_with(env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let principal = draws.iter().sum();
    let t0 = env.now();
    let p = env.scheduled_params(principal, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    for &d in draws {
        env.draw_amount(&k, line_id, &borrower, d);
    }
    assert_eq!(env.line(&k, line_id).drawn_at, t0);
    let mark = t0 + 4 * SIXTH + 1;
    env.warp(mark);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let keeper = env.funded(1);
    env.send(ix, &keeper, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    (k, borrower, line_id, t0, mark)
}

/// A total quita in collection (late interest, interest and every centavo of principal) closes the
/// line: the whole cost becomes the loss, the provision that anticipated it is released, and the
/// título, extinguished without a payment, goes to the suscriptor.
#[test]
fn a_total_quita_in_collection_closes_the_line_and_its_titulo_leaves() {
    let mut env = Env::new();
    let (k, borrower, line_id, t0, mark) = in_collection(&mut env, &[4 * UNIT]);
    // Before: the vault holds 10.0 - 4.0, the line's 4.0 is outstanding and impaired, fully
    // provisioned (100%): NAV 6.0.
    let nav = books(&env, &k, 6 * UNIT, 4 * UNIT, 4 * UNIT, 0);
    assert_eq!(nav, 6 * UNIT);
    let before = env.pagare(&k, line_id, 0);

    // Thirty days after the mark.
    let at = mark + 30 * DAY_SECONDS;
    env.warp(at);
    // Ordinary interest stopped at the mark: 4,000,000 * 12% * (4 * SIXTH + 1) / YEAR
    // = 4,000,000 * 0.12 * (2/3 + 1/31,536,000) = 320,000.015, rounded down: 320,000.
    let interest = 320_000;
    // Late interest at 100% a year on the six parts of 666,666 (the last 666,670): parts 1 to 4 from
    // their own due dates t0 + k * SIXTH, parts 5 and 6 from the mark; one division by YEAR.
    let weighted: u128 = (1..=4).map(|k| 666_666u128 * (at - (t0 + k * SIXTH)) as u128).sum::<u128>()
        + (666_666u128 + 666_670) * (at - mark) as u128;
    let late = (weighted / YEAR as u128) as u64;
    assert!(late > 0);

    let ev = env.condone(&k, line_id, interest, 4 * UNIT, DOCUMENT);
    assert_eq!(
        (ev.pool, ev.line_id, ev.document_hash, ev.late_interest, ev.interest, ev.principal),
        (k.pool, line_id, DOCUMENT, late, interest, 4 * UNIT)
    );
    // At parity a centavo costs a base unit: the cost is 4.0, all of it newly recognised.
    assert_eq!((ev.cost, ev.loss_recognized, ev.remaining, ev.condoned_at), (4 * UNIT, 4 * UNIT, 0, at));
    assert_eq!(ev.entries, vec![CondonationEntry { draw_index: 0, entry_count: 2, principal_condoned: 4 * UNIT, principal_balance: 0 }]);
    // After: nothing outstanding, nothing impaired, 4.0 written off; the provision had already
    // taken it out of the NAV, which stays at 6.0. No funds moved.
    assert_eq!(books(&env, &k, 6 * UNIT, 0, 0, 4 * UNIT), 6 * UNIT);
    assert_eq!(ev.nav_after, 6 * UNIT);
    let pool = env.pool(&k);
    assert_eq!((pool.interest_collected, pool.fx_realized), (0, 0));

    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.closed_at), (LineStatus::Repaid, at));
    assert_eq!((line.principal_repaid, line.usdc_cost_repaid, line.interest_repaid), (4 * UNIT, 4 * UNIT, interest));
    assert_eq!((line.late_paid_until, line.late_collected, line.pagares_paid), (at, 0, 1));

    let p = env.pagare(&k, line_id, 0);
    assert_eq!((p.status, p.flags, p.paid_at, p.receipt_hash), (PAGARE_PAID, PAGARE_FLAG_CONDONED, at, DOCUMENT));
    assert_eq!((p.last_kind, p.entry_count), (KIND_CONDONACION, 2));
    assert_eq!(
        (p.signed_pagare_hash, p.pagare_signature, p.signed_on),
        (before.signed_pagare_hash, before.pagare_signature, before.signed_on),
        "the quita leaves the signed message as the draw wrote it (decision 55)"
    );
    // The registry replays: from the pool to the suscriptor, over the record and the amounts.
    let expected = chain(&before.log_root, &p.asset, KIND_CONDONACION, &k.pool, &borrower.pubkey(), &condonation(&DOCUMENT, 4 * UNIT, 0), at, 2);
    assert_eq!(p.log_root, expected);

    let meta = env.deliver_pagare(&k, line_id, 0);
    let delivered: PagareDelivered = find_event(&meta).expect("PagareDelivered");
    assert_eq!((delivered.recipient, delivered.receipt_hash), (borrower.pubkey(), DOCUMENT));
    assert_eq!(env.pagare(&k, line_id, 0).flags, PAGARE_FLAG_CONDONED | PAGARE_FLAG_DELIVERED);
}

/// With half the impairment provisioned, the quita lowers the NAV by the unprovisioned half of the
/// cost, exactly as a write-off of that amount would.
#[test]
fn a_quita_lowers_the_nav_by_the_unprovisioned_part_of_its_cost() {
    let mut env = Env::new();
    let (k, _, line_id, _, _) = in_collection(&mut env, &[4 * UNIT]);
    let council = env.council.insecure_clone();
    let ix = env.council_pool_ix(&k, &council.pubkey(), anchor_lang::InstructionData::data(&elote_pool::instruction::SetProvisionBps { provision_bps: 5_000, valid_until: env.valid_until() }));
    env.send(ix, &council, &[]);
    // 6.0 + 4.0 - 4.0 * 50% = 8.0.
    assert_eq!(books(&env, &k, 6 * UNIT, 4 * UNIT, 4 * UNIT, 0), 8 * UNIT);
    // A quita of 1.0 of principal (no interest): outstanding and impaired fall by 1.0, the NAV by
    // its unprovisioned half: 6.0 + 3.0 - 1.5 = 7.5.
    let ev = env.condone(&k, line_id, 0, UNIT, DOCUMENT);
    assert_eq!((ev.cost, ev.loss_recognized, ev.nav_after), (UNIT, UNIT, 7 * UNIT + UNIT / 2));
    assert_eq!(books(&env, &k, 6 * UNIT, 3 * UNIT, 3 * UNIT, UNIT), 7 * UNIT + UNIT / 2);
}

/// The common settlement in collection: the line owes 60,000.00 of principal; the council condones
/// 20,000.00 of it, and the debtor pays the other 40,000.00 with the interest and the late interest
/// that ran since. The quita leaves the interest unpaid (the council remits what it chooses); the
/// payment takes it first, as always. The line ends at zero and both títulos leave.
#[test]
fn a_partial_quita_then_the_debtor_pays_the_rest_and_the_line_settles() {
    let mut env = Env::new();
    let (k, borrower, line_id, t0, mark) = in_collection(&mut env, &[3 * UNIT, 3 * UNIT]);
    let _ = t0;
    // Before: vault 10.0 - 6.0 = 4.0, outstanding and impaired 6.0, NAV 4.0.
    assert_eq!(books(&env, &k, 4 * UNIT, 6 * UNIT, 6 * UNIT, 0), 4 * UNIT);
    let root0 = env.pagare(&k, line_id, 0).log_root;
    // Interest to the mark: 6,000,000 * 12% * (2/3 + 1/YEAR) = 480,000.02, rounded down 480,000.
    // Late interest at the mark: parts 1 to 4 of 1,000,000 have run 3S+1, 2S+1, S+1 and 1 seconds
    // (S = SIXTH, 6S = YEAR), parts 5 and 6 nothing: 1,000,000 * (YEAR + 4) / YEAR = 1,000,000.
    let ev = env.condone(&k, line_id, 0, 2 * UNIT, DOCUMENT);
    assert_eq!((ev.late_interest, ev.interest, ev.principal, ev.cost, ev.loss_recognized), (1_000_000, 0, 2 * UNIT, 2 * UNIT, 2 * UNIT));
    // What remains: 4.0 of principal and the 480,000 of interest, not condoned.
    assert_eq!(ev.remaining, 4 * UNIT + 480_000);
    // The quita covers 2.0 of pagaré 0's 3.0: an entry with its balance of 1.0; pagaré 1 untouched.
    assert_eq!(ev.entries, vec![CondonationEntry { draw_index: 0, entry_count: 2, principal_condoned: 2 * UNIT, principal_balance: UNIT }]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.flags, p0.last_kind), (PAGARE_OUTSTANDING, 0, KIND_CONDONACION));
    let root1 = chain(&root0, &p0.asset, KIND_CONDONACION, &k.pool, &borrower.pubkey(), &condonation(&DOCUMENT, 2 * UNIT, UNIT), mark, 2);
    assert_eq!(p0.log_root, root1);
    assert_eq!(env.pagare(&k, line_id, 1).entry_count, 1);
    // 2.0 of cost leaves the book and the provision; the NAV (fully provisioned) does not move.
    assert_eq!(books(&env, &k, 4 * UNIT, 4 * UNIT, 4 * UNIT, 2 * UNIT), 4 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.principal_repaid, line.usdc_cost_repaid, line.pagares_paid), (LineStatus::InCollection, 2 * UNIT, 2 * UNIT, 0));
    assert_eq!(line.interest_unpaid(mark), Some(480_000), "the interest stays owed");
    // Two installments of 1,000,000 are met by the 2,000,000 condoned.
    let a = env.arrears(&k, line_id);
    assert_eq!((a.covered, a.late_due), (2, 0));

    // A month (YEAR / 12) later the debtor pays. Late interest ran on the 4,000,000 still overdue
    // since the quita: 4,000,000 * (YEAR / 12) / YEAR = 333,333.3, 333,333. Interest: 480,000.
    let at = mark + YEAR / 12;
    env.warp(at);
    let pay = 333_333 + 480_000 + 4 * UNIT;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), pay);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), pay);
    let meta = env.send(ix, &borrower, &[]);
    let repaid: LineRepaid = find_event(&meta).expect("LineRepaid");
    assert_eq!((repaid.interest_part, repaid.principal_part, repaid.remaining), (480_000, 4 * UNIT, 0));
    // After: the vault took 4,813,333; nothing outstanding; the 2.0 condoned stays written off;
    // the NAV rose by everything received: 4.0 + 4,813,333.
    assert_eq!(books(&env, &k, 4 * UNIT + pay, 0, 0, 2 * UNIT), 4 * UNIT + pay);
    assert_eq!(env.pool(&k).interest_collected, 480_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    // Pagaré 0: its last 1.0 paid, the accessories annotated on it; it was paid, not condoned.
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.flags, p0.receipt_hash), (PAGARE_PAID, 0, RECEIPT_HASH));
    let root2 = chain(&root1, &p0.asset, KIND_PAGADO, &borrower.pubkey(), &k.pool, &payment(&RECEIPT_HASH, UNIT, 333_333 + 480_000, 0, true), at, 3);
    assert_eq!(p0.log_root, root2);
    env.deliver_pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 1);
}

/// A quita of principal that would touch more than three pagaré records is split, like a payment;
/// each pagaré it reaches gets its own entry, and the line closes at the call that leaves it owing
/// nothing. On an active line no provision anticipated the loss: the NAV falls by the whole cost.
#[test]
fn a_quita_over_more_than_three_pagares_is_split() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(5 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    for _ in 0..5 {
        env.draw_amount(&k, line_id, &borrower, UNIT);
    }
    // Half a year of 10% on 5.0: 250,000.
    env.warp(env.line(&k, line_id).drawn_at + YEAR / 2);
    assert_eq!(books(&env, &k, 15 * UNIT, 5 * UNIT, 0, 0), 20 * UNIT);
    let council = env.council.insecure_clone();
    let ix = env.condone_ix(&k, line_id, &council.pubkey(), 250_000, 5 * UNIT, DOCUMENT);
    assert_eq!(ix.accounts.len(), 8 + 5, "five records");
    env.expect_err(ix, &council, &[], E_TOO_MANY_SIDECARS);

    // 2.0 over [0, 1, 2]: pagarés 0 and 1 extinguished, 2 ends the set untouched.
    let ev = env.condone(&k, line_id, 250_000, 2 * UNIT, DOCUMENT);
    assert_eq!(ev.entries.iter().map(|e| (e.draw_index, e.principal_condoned, e.principal_balance)).collect::<Vec<_>>(), vec![(0, UNIT, 0), (1, UNIT, 0)]);
    assert_eq!((ev.remaining, ev.nav_after), (3 * UNIT, 18 * UNIT));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Active);
    // The rest over [2, 3, 4].
    let ev = env.condone(&k, line_id, 0, 3 * UNIT, DOCUMENT);
    assert_eq!(ev.entries.iter().map(|e| e.draw_index).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert_eq!((ev.remaining, ev.cost, ev.loss_recognized, ev.nav_after), (0, 3 * UNIT, 3 * UNIT, 15 * UNIT));
    assert_eq!(books(&env, &k, 15 * UNIT, 0, 0, 5 * UNIT), 15 * UNIT);
    assert_eq!((env.line(&k, line_id).status, env.line(&k, line_id).pagares_paid), (LineStatus::Repaid, 5));
    for i in 0..5 {
        assert_eq!(env.pagare(&k, line_id, i).flags, PAGARE_FLAG_CONDONED);
        env.deliver_pagare(&k, line_id, i);
    }
}

/// Only the council condones, always against its record, never more than is owed, and with
/// exactly the records the principal reaches.
#[test]
fn a_principal_quita_is_the_councils_against_its_record_and_never_more_than_is_owed() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let ix = env.condone_ix(&k, line_id, &borrower.pubkey(), 0, UNIT, DOCUMENT);
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);
    let ix = env.condone_ix(&k, line_id, &council.pubkey(), 0, UNIT, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    // More principal than the 2.0 outstanding.
    let ix = env.condone_ix_full(&k, line_id, &council.pubkey(), 0, 2 * UNIT + 1, DOCUMENT, &env.condone_sidecars(&k, line_id, 2 * UNIT));
    env.expect_err(ix, &council, &[], E_WRONG_AMOUNT);
    // Principal needs its records; no principal takes none.
    let ix = env.condone_ix_full(&k, line_id, &council.pubkey(), 0, UNIT, DOCUMENT, &[]);
    env.expect_err(ix, &council, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    let ix = env.condone_ix_full(&k, line_id, &council.pubkey(), 0, UNIT / 2, DOCUMENT, &env.condone_sidecars(&k, line_id, 2 * UNIT));
    env.expect_err(ix, &council, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    env.warp(env.now() + DAY_SECONDS);
    let ix = env.condone_ix_full(&k, line_id, &council.pubkey(), 1, 0, DOCUMENT, &env.condone_sidecars(&k, line_id, UNIT));
    env.expect_err(ix, &council, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    // All the principal is condoned only with the interest still unpaid: left alone, that
    // interest would be paid with no título left to take its entry. A day of 10% on 2.0:
    // 2,000,000 * 0.10 / 365 = 547.9, 547.
    // The council signs minutes after proposing and the interest grows meanwhile, so a quita of
    // all the principal takes all the interest there is when it lands, whatever figure it named:
    // built now naming none, sent an hour later (2,000,000 * 0.10 * 90,000 / YEAR = 570.7, 570).
    let ix = env.condone_ix(&k, line_id, &council.pubkey(), 0, 2 * UNIT, DOCUMENT);
    env.warp(env.now() + 3_600);
    let meta = env.send(ix, &council, &[]);
    let ev: elote_pool::events::LineCondoned = find_event(&meta).expect("LineCondoned");
    assert_eq!((ev.interest, ev.principal, ev.remaining), (570, 2 * UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
    // A closed line takes no quita.
    let ix = env.condone_ix(&k, line_id, &council.pubkey(), 0, 0, DOCUMENT);
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
}

/// Principal condoned counts toward the schedule (decision 48: installments are principal), the
/// late interest accrued is condoned with it, and ordinary interest from then on accrues on the
/// lower balance; the interest accrued before stays owed.
#[test]
fn principal_condoned_meets_installments_and_lowers_the_accrual() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let t0 = env.now();
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, 6 * UNIT);
    let at = t0 + 2 * SIXTH + 1_000;
    env.warp(at);
    let a = env.arrears(&k, line_id);
    assert_eq!((a.due, a.covered, a.missed), (2, 0, 2));
    // Before (active, no provision): vault 4.0, outstanding 6.0, NAV 10.0.
    assert_eq!(books(&env, &k, 4 * UNIT, 6 * UNIT, 0, 0), 10 * UNIT);
    // Late at 100%: installment 1 has run SIXTH + 1,000 s and 2 1,000 s on 1,000,000 each:
    // 1,000,000 * 5,258,000 / 31,536,000 = 166,730.06, 166,730.
    let ev = env.condone(&k, line_id, 0, 2 * UNIT, DOCUMENT);
    assert_eq!((ev.late_interest, ev.interest, ev.principal), (166_730, 0, 2 * UNIT));
    let a = env.arrears(&k, line_id);
    assert_eq!((a.covered, a.missed, a.late_due), (2, 0, 0));
    // The whole 2.0 is a loss on an active line: NAV 10.0 - 2.0.
    assert_eq!(books(&env, &k, 4 * UNIT, 4 * UNIT, 0, 2 * UNIT), 8 * UNIT);
    // Interest accrued so far: 6,000,000 * 12% * (2 * SIXTH + 1,000) / YEAR = 240,022.8, 240,022;
    // it stays owed.
    assert_eq!(env.line(&k, line_id).interest_unpaid(at), Some(240_022));
    // At the third due date: (6,000,000 * (2S + 1,000) + 4,000,000 * (S - 1,000)) * 12% / YEAR
    // = 320,007.6, 320,007: the balance after the quita accrues 4.0, not 6.0.
    env.warp(t0 + 3 * SIXTH);
    assert_eq!(env.line(&k, line_id).interest_accrued(env.now()), Some(320_007));
}

/// A recovery on a written-off line, at parity, partial then full. The write-off recognised the
/// whole 2.0 as a loss; ordinary interest stopped at the term, but the late interest keeps running
/// on the principal owed. The recovery pays late interest, interest, then principal; the principal
/// comes off `written_off`, never off `outstanding_principal` again, and the NAV takes every
/// centavo received. When nothing is owed the line closes and the títulos leave.
#[test]
fn a_written_off_line_takes_recoveries_and_its_titulos_leave_when_it_owes_nothing() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.line_params(2 * UNIT, 1_000);
    p.late_bps = 3_650;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    // The write-off: 2.0 leaves the book as the loss; the NAV goes 20.0 -> 18.0.
    assert_eq!(books(&env, &k, 18 * UNIT, 0, 0, 2 * UNIT), 18 * UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);

    // Ten days past the term (864,000 s). Ordinary interest is frozen at a year of 10% on 2.0,
    // 200,000; late interest at 36.5% on 2.0 since the term: 2,000,000 * 0.365 * 864,000 / YEAR
    // = 20,000.
    let at = p.term_end + 864_000;
    env.warp(at);
    let line = env.line(&k, line_id);
    assert_eq!(line.interest_accrued(at), Some(200_000));
    assert_eq!(env.arrears(&k, line_id).late_due, 20_000);

    // A partial recovery of 1.5: 20,000 late, 200,000 interest, 1,280,000 principal (pagaré 0 and
    // 280,000 of pagaré 1).
    let pay = 1_500_000;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 3 * UNIT);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), pay);
    let meta = env.send(ix, &borrower, &[]);
    let rec: LossRecovered = find_event(&meta).expect("LossRecovered");
    assert_eq!(
        (rec.line_id, rec.receipt_hash, rec.late_part, rec.interest_part, rec.principal_part),
        (line_id, RECEIPT_HASH, 20_000, 200_000, 1_280_000)
    );
    // At parity the principal cost 1,280,000 and paid 1,280,000; 2.0 - 1,280,000 stays a loss.
    assert_eq!((rec.cost_recovered, rec.received, rec.written_off_after), (1_280_000, 1_280_000, 720_000));
    let fx: FxRealized = find_event(&meta).expect("FxRealized");
    assert_eq!((fx.cost, fx.received, fx.delta), (1_280_000, 1_280_000, 0));
    // After: the vault took 1.5, nothing returns to outstanding, the NAV rose by 1.5.
    assert_eq!(books(&env, &k, 18 * UNIT + pay, 0, 0, 720_000), 18 * UNIT + pay);
    assert_eq!(rec.nav_after, 18 * UNIT + pay);
    assert_eq!(env.pool(&k).interest_collected, 200_000);
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.principal_repaid, line.usdc_cost_repaid, line.pagares_paid), (LineStatus::WrittenOff, 1_280_000, 1_280_000, 1));
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);

    // The rest, in the same second: 720,000 of principal, no late interest left.
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 720_000);
    let meta = env.send(ix, &borrower, &[]);
    let rec: LossRecovered = find_event(&meta).expect("LossRecovered");
    assert_eq!((rec.late_part, rec.interest_part, rec.principal_part, rec.cost_recovered, rec.written_off_after), (0, 0, 720_000, 720_000, 0));
    // The loss is fully recovered: written off back to zero; the NAV is the 20.0 deposited plus
    // the 220,000 of interest and late interest.
    assert_eq!(books(&env, &k, 20 * UNIT + 220_000, 0, 0, 0), 20 * UNIT + 220_000);
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.closed_at, line.principal_repaid), (LineStatus::Repaid, at, 2 * UNIT));
    env.deliver_pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 1);
    assert_eq!(env.pagare(&k, line_id, 1).flags, PAGARE_FLAG_DELIVERED);
}

/// Whoever deposits just before a recovery takes none of it (decision 51, with decision 40b): a
/// written-off line's cost stays in the price deposits pay until it is recovered or condoned, so
/// the entrant bought that claim at par. 20.0 in the pool, 2.0 written off at 0%; 60.0 deposited
/// before the debtor pays the 2.0 back, then redeemed: 60.0 at most.
#[test]
fn a_deposit_before_a_recovery_takes_none_of_it() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.line_params(2 * UNIT, 0);
    p.late_bps = 0;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(books(&env, &k, 18 * UNIT, 0, 0, 2 * UNIT), 18 * UNIT);
    assert_eq!(env.pool(&k).recoverable, 2 * UNIT, "the claim written off is still owed");

    let entrant = env.investor(&k, 60 * UNIT);
    let shares = env.deposit(&k, &entrant, 60 * UNIT).shares;
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 2 * UNIT);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pool(&k).recoverable, 0);
    let out = env.redeem(&k, &entrant, shares).assets;
    assert!(out <= 60 * UNIT && out >= 60 * UNIT - 2, "the entrant leaves with what it brought: {out}");
    // The 2.0 recovered went to those who bore the loss: 20.0 again.
    assert!(env.nav(&k) >= 20 * UNIT);
}

/// What the entry price does not cover: the interest and late interest a recovery brings are
/// income of the moment, shared by whoever holds shares then, as for any payment
/// (`limit_deposit_just_before_a_repayment_shares_the_interest`). 2.0 at 10%, late 36.5%, ten
/// days past the term: 200,000 of interest and 20,000 late. The entrant holds 60.0 of 80.0 of
/// the shares and leaves with three quarters of those 220,000.
#[test]
fn limit_a_deposit_before_a_recovery_shares_its_interest_and_late_interest() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.line_params(2 * UNIT, 1_000);
    p.late_bps = 3_650;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    env.warp(p.term_end + 864_000);
    let entrant = env.investor(&k, 60 * UNIT);
    let shares = env.deposit(&k, &entrant, 60 * UNIT).shares;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 2 * UNIT + 220_000);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    let out = env.redeem(&k, &entrant, shares).assets;
    assert_eq!(out, 60 * UNIT + 165_000, "none of the principal, three quarters of the accessories");
}

/// The claim leaves the entry price when the council condones it: nothing is left to recover.
#[test]
fn a_quita_on_a_written_off_line_takes_its_claim_out_of_the_entry_price() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.line_params(2 * UNIT, 0);
    p.late_bps = 0;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    env.condone(&k, line_id, 0, UNIT, DOCUMENT);
    assert_eq!(env.pool(&k).recoverable, UNIT);
    env.condone(&k, line_id, 0, UNIT, DOCUMENT);
    let pool = env.pool(&k);
    assert_eq!((pool.recoverable, pool.written_off), (0, 2 * UNIT));
    // A deposit now prices at the 18.0 the pool holds: 18.0 buys as many shares as there are.
    let supply = env.supply(&k);
    let entrant = env.investor(&k, 18 * UNIT);
    assert_eq!(env.deposit(&k, &entrant, 18 * UNIT).shares, supply);
}

/// A quita on a line already written off recognises nothing again: the write-off took the loss,
/// so the pool does not move; the line closes and its títulos leave, marked condoned.
#[test]
fn a_quita_on_a_written_off_line_recognises_no_second_loss() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(books(&env, &k, 18 * UNIT, 0, 0, 2 * UNIT), 18 * UNIT);
    // A year of 10% on 2.0: 200,000, and the whole principal.
    let ev = env.condone(&k, line_id, 200_000, 2 * UNIT, DOCUMENT);
    assert_eq!((ev.principal, ev.cost, ev.loss_recognized, ev.remaining, ev.nav_after), (2 * UNIT, 2 * UNIT, 0, 0, 18 * UNIT));
    assert_eq!(books(&env, &k, 18 * UNIT, 0, 0, 2 * UNIT), 18 * UNIT, "the pool does not move");
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.flags & (PAGARE_FLAG_CONDONED | PAGARE_FLAG_PAID_BY_AVAL)), (PAGARE_PAID, PAGARE_FLAG_CONDONED));
    env.deliver_pagare(&k, line_id, 0);
}

/// A pool at the real FIX: 5,000 USDC deposited at 18.3740, a MXN 50,000 bullet line at 0%.
fn real_fix_line(env: &mut Env) -> (PoolKeys, Keypair, u64) {
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset);
    let mut params = env.init_params(BPS, COLLECTION_PERIOD);
    params.max_outstanding_principal = u64::MAX;
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), params);
    env.send(ix, &council, &[]);
    post(env, &k, 183_740);
    let investor = env.investor(&k, 5_000 * UNIT);
    env.deposit(&k, &investor, 5_000 * UNIT);
    let borrower = env.borrower(&k);
    let terms = env.line_params(5_000_000, 0);
    let (id, _) = env.request_line_with(&k, &borrower, &terms, None);
    env.release_fix(&k);
    env.approve_line(&k, id, &borrower.pubkey(), &terms);
    (k, borrower, id)
}

fn post(env: &mut Env, k: &PoolKeys, fix_rate: u64) {
    let council = env.council.insecure_clone();
    let ix = env.post_fix_ix(k, &council.pubkey(), fix_rate, env.today());
    env.send(ix, &council, &[]);
}

/// Each pagaré keeps its own cost basis: a quita across two pagarés drawn at different FIXes costs
/// the first one's whole delivery and the second's share of its own, realizes no FX, and a later
/// recovery of the rest at a third FIX takes that remaining cost off `written_off` and realizes
/// the difference.
#[test]
fn quita_and_recovery_cost_each_pagare_at_its_own_fix() {
    let mut env = Env::new();
    let (k, borrower, id) = real_fix_line(&mut env);
    // MXN 20,000.00 at 18.3740: 2,000,000 * 10^8 / 183,740 = 1,088,494,611.95, 1,088,494,611.
    env.draw_amount(&k, id, &borrower, 2_000_000);
    let d0 = 1_088_494_611;
    assert_eq!(env.pagare(&k, id, 0).usdc_delivered, d0);
    // A day later, MXN 30,000.00 at 20.0000: 1,500,000,000.
    env.warp(env.now() + DAY_SECONDS);
    post(&mut env, &k, 200_000);
    env.draw_amount(&k, id, &borrower, 3_000_000);
    let d1 = 1_500_000_000;
    assert_eq!(env.pagare(&k, id, 1).usdc_delivered, d1);
    let vault = 5_000 * UNIT - d0 - d1;
    assert_eq!(books(&env, &k, vault, d0 + d1, 0, 0), 5_000 * UNIT);

    // A quita of MXN 25,000.00: pagaré 0 whole (cost d0) and 500,000 of pagaré 1's 3,000,000
    // (cost 1,500,000,000 * 500,000 / 3,000,000 = 250,000,000).
    let ev = env.condone(&k, id, 0, 2_500_000, DOCUMENT);
    let cost = d0 + 250_000_000;
    assert_eq!((ev.cost, ev.loss_recognized), (cost, cost));
    assert_eq!(books(&env, &k, vault, d1 - 250_000_000, 0, cost), vault + d1 - 250_000_000);
    assert_eq!(env.pool(&k).fx_realized, 0, "no funds moved, nothing realized");

    // Past the term, the rest is written off: the remaining cost 1,250,000,000.
    env.warp(env.line(&k, id).term_end + 1);
    let council = env.council.insecure_clone();
    let ix = env.write_off_ix(&k, id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!(books(&env, &k, vault, 0, 0, d0 + d1), vault);

    // Recovered at 19.0000: MXN 25,000.00 takes ceil(2,500,000 * 10^8 / 190,000) = 1,315,789,474
    // USDC; its cost was 1,250,000,000, which comes off `written_off`; the 65,789,474 above it is
    // an FX gain. The NAV takes the whole 1,315,789,474.
    env.warp(env.now() + DAY_SECONDS);
    post(&mut env, &k, 190_000);
    let usdc = 1_315_789_474;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), usdc);
    let ix = env.repay_ix(&k, id, &borrower.pubkey(), usdc);
    let meta = env.send(ix, &borrower, &[]);
    let rec: LossRecovered = find_event(&meta).expect("LossRecovered");
    assert_eq!((rec.principal_part, rec.cost_recovered, rec.received, rec.written_off_after), (2_500_000, 1_250_000_000, usdc, cost));
    let fx: FxRealized = find_event(&meta).expect("FxRealized");
    assert_eq!(fx.delta, 65_789_474);
    assert_eq!(books(&env, &k, vault + usdc, 0, 0, cost), vault + usdc);
    assert_eq!(env.pool(&k).fx_realized, 65_789_474);
    assert_eq!(env.line(&k, id).status, LineStatus::Repaid);
    assert_eq!(env.pagare(&k, id, 0).flags, PAGARE_FLAG_CONDONED);
    assert_eq!(env.pagare(&k, id, 1).flags, 0, "paid by the recovery");
    env.deliver_pagare(&k, id, 0);
    env.deliver_pagare(&k, id, 1);
}

/// A quita that extinguishes a título under an endoso en procuración ends it, as a payment does:
/// a `Terminacion` entry to the procurador over the same commitment, right after the
/// `Condonacion`.
#[test]
fn a_quita_that_extinguishes_a_titulo_ends_its_procuracion() {
    use elote_pool::state::{KIND_TERMINACION, PAGARE_FLAG_PROCURACION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let before = env.pagare(&k, line_id, 0);
    assert_eq!((before.flags, before.entry_count), (PAGARE_FLAG_PROCURACION, 2));
    let at = env.now();
    // Drawn this second: no interest; the whole 1.0 of principal.
    let ev = env.condone(&k, line_id, 0, UNIT, DOCUMENT);
    assert_eq!((ev.remaining, ev.entries[0].entry_count), (0, 3));
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.flags, p0.procurador, p0.last_kind, p0.entry_count), (PAGARE_PAID, PAGARE_FLAG_CONDONED, solana_pubkey::Pubkey::default(), KIND_TERMINACION, 4));
    let commitment = condonation(&DOCUMENT, UNIT, 0);
    let root = chain(&before.log_root, &p0.asset, KIND_CONDONACION, &k.pool, &borrower.pubkey(), &commitment, at, 3);
    let root = chain(&root, &p0.asset, KIND_TERMINACION, &k.pool, &procurador.pubkey(), &commitment, at, 4);
    assert_eq!(p0.log_root, root);
    env.deliver_pagare(&k, line_id, 0);
}
