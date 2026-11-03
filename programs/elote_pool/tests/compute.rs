//! Compute-unit regression tests. The only loop in the program is `math::arrears`, bounded by
//! `MAX_INSTALLMENTS` (120). These tests drive it at 18 installments (the pilot's longest term)
//! and at the cap, on the three instructions that run it, and pin a ceiling so a change that
//! makes the loop heavier fails here before it fails on chain.
//!
//! The ceiling is below the 200_000 CU a transaction gets per instruction without a compute
//! budget instruction, so clients never need to request more.
//!
//! The figures are printed by each test and listed in ARCHITECTURE.md §10.

mod common;

use common::*;
use solana_signer::Signer;

/// Every instruction must stay below this, with headroom for a future accrual of state reads.
const CEILING: u64 = 100_000;

/// Compute units of `repay`, `mark_in_collection` and `write_off` on a line with `installments`
/// installments, all of them missed, so the arrears loop runs its full length each time.
fn compute_units(installments: u16) -> [(&'static str, u64); 3] {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, installments, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // Past the last due date: every installment is missed and late interest has run on each.
    env.warp(env.now() + i64::from(installments) * p.interval() + 1);
    let late = env.arrears(&k, line_id).late_due;
    assert!(late > 0);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    // The write-off waits for the collection period from the mark.
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let write_off = env.send(ix, &council, &[]).compute_units_consumed;
    [("repay", repay), ("mark_in_collection", mark), ("write_off", write_off)]
}

/// `init_pool` walks the extensions of both mints and validates the share mint; the heaviest
/// realistic shape is a Token-2022 asset with issuer controls and a permissioned share mint.
#[test]
fn init_pool_stays_under_the_ceiling_with_and_without_extensions() {
    let mut env = Env::new();
    let (_, meta) = env.setup_pool();
    let plain = meta.compute_units_consumed;
    let gate = env.funded(1);
    let (_, meta) = env.setup_permissioned_pool(&gate.pubkey());
    let permissioned = meta.compute_units_consumed;
    println!("init_pool, classic asset and plain shares: {plain} CU");
    println!("init_pool, classic asset and permissioned shares: {permissioned} CU");
    assert!(plain < CEILING && permissioned < CEILING, "init_pool used {plain} / {permissioned} CU, ceiling {CEILING}");
}

/// `draw` on a 120-installment line: the payability check and the tranche interest add a few
/// multiplications; the Metaplex Core CPI lands here too (about 37 000 CU of the total, fixed by
/// the plugin set) and so does the registry's issuance entry. Deterministic since the client
/// passes the PDA bumps. At about 82 500 CU there are some 17 000 CU of room before any growth in
/// `draw` needs a compute-budget request from clients; trimming attributes is the lever.
#[test]
fn draw_stays_under_the_ceiling_at_120_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 40 * UNIT);
    let first = env.send_with_attestor(ix, &borrower).compute_units_consumed;
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 60 * UNIT);
    let second = env.send_with_attestor(ix, &borrower).compute_units_consumed;
    println!("draw, first tranche at 120 installments: {first} CU");
    println!("draw, second tranche at 120 installments: {second} CU");
    assert!(first < CEILING && second < CEILING, "draw used {first} / {second} CU, ceiling {CEILING}");
}

/// `cancel_pagare` runs no arrears loop: one coverage comparison, the sidecar's mark and the Core
/// `BurnV1` CPI. Measured on a 120-installment line repaid in full, the shape of a real
/// cancellation.
#[test]
fn cancel_pagare_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let interest = env.line(&k, line_id).interest_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 100 * UNIT + interest);
    let cancel = env.cancel_pagare(&k, line_id, 0).compute_units_consumed;
    println!("cancel_pagare at 120 installments: {cancel} CU");
    assert!(cancel < CEILING, "cancel_pagare used {cancel} CU, ceiling {CEILING}");
    // The procurador's branch, on a second line.
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(50 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 50 * UNIT + interest);
    let attestor = env.attestor.insecure_clone();
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &procurador.pubkey(), &council.pubkey(), RECEIPT_HASH);
    let by_procurador = env.send(ix, &council, &[&procurador, &attestor]).compute_units_consumed;
    println!("cancel_pagare by the procurador at 120 installments: {by_procurador} CU");
    assert!(by_procurador < CEILING, "cancel by the procurador used {by_procurador} CU, ceiling {CEILING}");
    // The backstop, on a third line: the request, then the holder alone after the delay.
    let borrower = env.borrower(&k);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 50 * UNIT + interest);
    let ix = env.request_cancellation_ix(&k, line_id, 0, &council.pubkey(), RECEIPT_HASH);
    let request = env.send(ix, &council, &[]).compute_units_consumed;
    println!("request_cancellation at 120 installments: {request} CU");
    env.warp(env.now() + i64::from(CANCEL_DELAY) + 1);
    let ix = env.cancel_pagare_alone_ix(&k, line_id, 0, &council.pubkey(), &council.pubkey(), RECEIPT_HASH);
    let alone = env.send(ix, &council, &[]).compute_units_consumed;
    println!("cancel_pagare by the holder alone at 120 installments: {alone} CU");
    assert!(request < CEILING && alone < CEILING);
}

/// After the mark every remaining installment is due, so a repayment early in a 120-installment
/// schedule loops over all of them instead of the few that were missed; the figure is the
/// all-missed shape above, which the ceiling already covers.
#[test]
fn repay_after_the_mark_stays_under_the_ceiling_at_120_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(env.now() + 3 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    let late = env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let write_off = env.send(ix, &council, &[]).compute_units_consumed;
    println!("mark_in_collection at 120 installments, 3 missed: {mark} CU");
    println!("repay after the mark at 120 installments: {repay} CU");
    println!("write_off after the mark at 120 installments: {write_off} CU");
    assert!(mark < CEILING && repay < CEILING && write_off < CEILING);
}

/// `endorse_pagare` en procuración: one registry entry and one Core CPI (the first time an
/// `AddPluginV1`, later an approve).
#[test]
fn procuracion_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let procurador = env.funded(1);
    let cu = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH).compute_units_consumed;
    println!("endorse_pagare (procuracion, plugin added) at 120 installments: {cu} CU");
    assert!(cu < CEILING, "procuracion used {cu} CU, ceiling {CEILING}");
    let council = env.council.insecure_clone();
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &council.pubkey(), [0x7e; 32]);
    let revoke = env.send(ix, &council, &[]).compute_units_consumed;
    println!("revoke_procuracion at 120 installments: {revoke} CU");
    let again = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH).compute_units_consumed;
    println!("endorse_pagare (procuracion, plugin approved) at 120 installments: {again} CU");
    assert!(revoke < CEILING && again < CEILING);
    let court = env
        .court_annotation(&k, line_id, 0, elote_pool::state::KIND_ANOTACION_JUDICIAL, [0x6a; 32])
        .compute_units_consumed;
    println!("court_annotation at 120 installments: {court} CU");
    assert!(court < CEILING);
}

#[test]
fn arrears_instructions_stay_under_the_ceiling_at_18_and_120_installments() {
    for n in [18u16, 120] {
        for (name, cu) in compute_units(n) {
            println!("{name} at {n} installments: {cu} CU");
            assert!(cu < CEILING, "{name} at {n} installments used {cu} CU, ceiling {CEILING}");
        }
    }
}

/// The ceiling case of the payment entries: one payment covering five pagarés (the client rule,
/// `MAX_SIDECARS_PER_REPAY`) on a 120-installment line with every installment missed (late
/// interest on all of them), with at least 5 000 CU to spare. Six leave less than that margin.
#[test]
fn a_payment_covering_five_sidecars_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(60 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    for _ in 0..5 {
        env.draw_amount(&k, line_id, &borrower, 10 * UNIT);
    }
    let t0 = env.line(&k, line_id).drawn_at;
    env.warp(t0 + 120 * p.interval() + 1);
    let line = env.line(&k, line_id);
    let owed = line.drawn + line.interest_due - line.principal_repaid - line.interest_repaid + env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed);
    let sidecars = env.payment_sidecars(&k, line_id, owed);
    assert_eq!(sidecars.len(), 5);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), owed);
    let cu = env.send(ix, &borrower, &[]).compute_units_consumed;
    println!("repay covering five sidecars at 120 installments, all missed: {cu} CU");
    assert_eq!(env.line(&k, line_id).pagares_paid, 5);
    assert!(cu + 5_000 <= CEILING, "the five-sidecar repay used {cu} CU, ceiling {CEILING} less a 5 000 CU margin");
}

#[test]
fn request_and_approve_stay_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.scheduled_params(4 * UNIT, 1_200, 120, 5_000);
    p.sponsor = None;
    let (id, request) = env.request_line_with(&k, &borrower, &p, None);
    let approve = env.approve_line(&k, id, &borrower.pubkey(), &p);
    let (request, approve) = (request.compute_units_consumed, approve.compute_units_consumed);
    println!("request_line at 120 installments: {request} CU");
    println!("approve_line (with the multisig check): {approve} CU");
    assert!(request < CEILING && approve < CEILING);
}
