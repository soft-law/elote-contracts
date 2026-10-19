//! Compute-unit regression tests. The only loop in the program is `math::arrears`, bounded by
//! `MAX_INSTALLMENTS` (120). These tests drive it at 18 installments (the pilot's longest term)
//! and at the cap, on the three instructions that run it, and pin a ceiling so a change that
//! makes the loop heavier fails here before it fails on chain.
//!
//! The ceiling is below the 200_000 CU a transaction gets per instruction without a compute
//! budget instruction, so clients never need to request more.

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
    env.warp(env.now() + i64::from(installments) * MONTH + 1);
    let late = env.arrears(&k, line_id).late_due;
    assert!(late > 0);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    let write_off = env.send(ix, &admin, &[]).compute_units_consumed;
    [("repay", repay), ("mark_in_collection", mark), ("write_off", write_off)]
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
