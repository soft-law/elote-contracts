//! Compute-unit regression tests. The only loop in the program is `math::arrears`, bounded by
//! `MAX_INSTALLMENTS` (120). These tests drive it at 18 installments (the pilot's longest term)
//! and at the cap, on the three instructions that run it, and pin a ceiling so a change that
//! makes the loop heavier fails here before it fails on chain.
//!
//! The ceiling is below the 200_000 CU a transaction gets per instruction without a compute
//! budget instruction, so clients never need to request more.
//!
//! Measured at 120 installments, every installment missed: repay 56 569 CU, mark_in_collection
//! 33 453, write_off 38 620 (84 381, 60 106 and 65 271 before steps 5d and the carried cumulative
//! due; the loop now costs one u64 multiply, one subtraction and one u128 multiply per installment
//! on the regular path).

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
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    // The write-off waits for the collection period from the mark.
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    let write_off = env.send(ix, &admin, &[]).compute_units_consumed;
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
/// multiplications; the Metaplex Core CPI lands here too (about 44 000 CU of the
/// total, fixed by the plugin set). Deterministic since the client passes the PDA bumps. At
/// about 90 000 CU there are some 10 000 CU of room before any growth in `draw` needs a
/// compute-budget request from clients; trimming attributes is the lever if it comes to that.
#[test]
fn draw_stays_under_the_ceiling_at_120_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 40 * UNIT, DOCUMENT_HASH);
    let first = env.send_with_attestor(ix, &borrower).compute_units_consumed;
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 60 * UNIT, DOCUMENT_HASH);
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
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 100 * UNIT + interest);
    let cancel = env.cancel_pagare(&k, line_id, 0).compute_units_consumed;
    println!("cancel_pagare at 120 installments: {cancel} CU");
    assert!(cancel < CEILING, "cancel_pagare used {cancel} CU, ceiling {CEILING}");
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
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    let write_off = env.send(ix, &admin, &[]).compute_units_consumed;
    println!("mark_in_collection at 120 installments, 3 missed: {mark} CU");
    println!("repay after the mark at 120 installments: {repay} CU");
    println!("write_off after the mark at 120 installments: {write_off} CU");
    assert!(mark < CEILING && repay < CEILING && write_off < CEILING);
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
