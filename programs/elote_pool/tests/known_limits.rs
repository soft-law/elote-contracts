//! Documentation tests: each one pins a known design limit of the program as it is deployed
//! today. They pass against the current code on purpose. None of them is a bug report; every
//! limit here is listed under "Known risks by design" in SECURITY.md.
//!
//! When a limit is removed (two-step origination, roles, accrual, a cancel instruction, a
//! recovery instruction...), the matching test here must be inverted or deleted in the same
//! change, so the suite always states what the program does, not what it should do.

mod common;

use common::*;
use elote_pool::state::LineStatus;
use solana_signer::Signer;

/// Inverted 2026-10-31 (R-01 delta). The admin can no longer name itself as the borrower: the
/// one-key path "open a line to self, draw the vault" is closed. Everything else about the
/// admin's lending power (any other borrower, no allowlist) still holds; see the trust table.
#[test]
fn admin_cannot_be_the_borrower_of_its_own_line() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let admin = env.admin.insecure_clone();
    env.create_ata(&admin.pubkey(), &k.asset_mint);

    let p = env.line_params(UNIT, 0);
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &admin.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[], E_BORROWER_IS_ADMIN);
    assert_eq!(env.pool(&k).line_count, 0);
    assert_eq!(env.token_balance(&k.vault), 10 * UNIT, "the vault is untouched");
}

/// Inverted 2026-10-31 (decision 20, register P1-1). A line for the whole vault is refused once
/// the pool carries a concentration cap: with the scripts' default of 1,000 bps, the largest
/// line is a tenth of the NAV. The harness opens its pools at 10,000 bps, where the cap is
/// inert, which is why this test sets its own.
#[test]
fn a_line_for_the_whole_vault_is_refused_under_the_concentration_cap() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 6);
    let ix = env.init_pool_ix_full(&admin.pubkey(), &mint, 0, 1_000);
    env.send(ix, &admin, &[]);
    let k = PoolKeys::new(&admin.pubkey(), &mint);
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);

    let borrower = env.borrower(&k);
    let p = env.line_params(10 * UNIT, 0);
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&borrower], E_LINE_TOO_LARGE);
    assert_eq!(env.pool(&k).line_count, 0);
    assert_eq!(env.token_balance(&k.vault), 10 * UNIT, "the vault is untouched");
}

/// Interest counts when it is collected, so a deposit placed just before a known repayment
/// shares in it and can leave right after (roadmap: "Interest counts when repaid").
#[test]
fn limit_deposit_just_before_a_repayment_shares_the_interest() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let early = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &early, 10 * UNIT);
    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000); // 0.4 of interest
    env.draw(&k, line_id, &borrower);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), 400_000);

    // The late investor enters at the pre-interest NAV of 10.0...
    let late = env.investor(&k, 10 * UNIT);
    let minted = env.deposit(&k, &late, 10 * UNIT).shares;
    assert_eq!(minted, 10 * UNIT, "one share per unit at NAV 1.0");
    // ...the borrower repays principal plus interest...
    env.repay(&k, line_id, &borrower, 4 * UNIT + 400_000);
    // ...and the late investor leaves with half of the interest the early one earned.
    let out = env.redeem(&k, &late, minted);
    assert_eq!(out.assets, 10 * UNIT + 200_000);
}

/// A loss counts only at `write_off`. `mark_in_collection` is public and leaves the NAV at par,
/// so a holder who redeems between the two steps exits at the pre-loss value and the loss falls
/// on whoever stays (roadmap: "Losses count when written off").
#[test]
fn limit_redeem_between_collection_and_write_off_exits_at_par() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let quick = env.investor(&k, 10 * UNIT);
    let quick_shares = env.deposit(&k, &quick, 10 * UNIT).shares; // minus the dead shares
    let patient = env.investor(&k, 10 * UNIT);
    let patient_shares = env.deposit(&k, &patient, 10 * UNIT).shares;
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // Three installments missed: anyone moves the line to collection. NAV is still 20.0.
    env.warp(env.now() + 3 * MONTH + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &quick, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    assert_eq!(env.nav(&k), 20 * UNIT);

    let out = env.redeem(&k, &quick, quick_shares);
    assert_eq!(out.assets, quick_shares, "one unit per share: the par value");

    let admin = env.admin.insecure_clone();
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    let out = env.redeem(&k, &patient, patient_shares);
    assert!(out.assets < 5 * UNIT, "the whole 6.0 loss lands on the holder who stayed: {}", out.assets);
}

/// The pause blocks redemptions but not repayments: depositors cannot leave while paused, and
/// nothing bounds the pause (roadmap: "Pause scope").
#[test]
fn limit_pause_blocks_exit_while_repayments_continue() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    let shares = env.deposit(&k, &investor, 10 * UNIT).shares;
    let borrower = env.borrower(&k);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 0);
    env.draw(&k, line_id, &borrower);

    env.set_paused(&k, true);
    let ix = env.redeem_ix(&k, &investor.pubkey(), shares);
    env.expect_err(ix, &investor, &[], E_PAUSED);
    env.repay(&k, line_id, &borrower, 4 * UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    let ix = env.redeem_ix(&k, &investor.pubkey(), shares);
    env.expect_err(ix, &investor, &[], E_PAUSED);
}

/// After a total write-off the NAV is zero while shares still exist, and `deposit` refuses with
/// `Overflow` (a division by the zero NAV). Only a raw transfer into the vault reopens the pool
/// (roadmap: "No recoveries").
#[test]
fn limit_total_write_off_leaves_a_pool_that_cannot_take_deposits() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 2 * UNIT);
    env.deposit(&k, &investor, 2 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 0);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(p.term_end + 1);
    let admin = env.admin.insecure_clone();
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    assert_eq!((env.nav(&k), env.supply(&k)), (0, 2 * UNIT));

    let newcomer = env.investor(&k, UNIT);
    let ix = env.deposit_ix(&k, &newcomer.pubkey(), UNIT);
    env.expect_err(ix, &newcomer, &[], E_OVERFLOW);
    // A recovery can only arrive as an untracked donation, and then deposits work again.
    env.donate(&k, UNIT);
    let minted = env.deposit(&k, &newcomer, UNIT).shares;
    assert_eq!(minted, 2 * UNIT, "priced against the donated NAV of 1.0 and the old supply of 2.0");
    // The written-off line takes no repayment, so the borrower's money cannot return as such.
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

/// Collection needs `covered + max_missed <= installments`. On a short schedule a borrower who
/// pays early installments and then stops can never be moved to collection; only the write-off
/// after the term remains.
#[test]
fn limit_collection_is_unreachable_when_covered_plus_max_missed_exceeds_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 3, 5_000); // 6.72 in three, max_missed 3
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.repay(&k, line_id, &borrower, 2_240_000); // exactly one installment, paid ahead
    assert_eq!(env.arrears(&k, line_id).covered, 1);

    env.warp(p.term_end + 10 * 365 * 86_400);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &investor, &[], E_NOT_ENOUGH_MISSED);
    let admin = env.admin.insecure_clone();
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

/// Inverted 2026-10-31 (R-07 delta). The first-deposit minimum is derived from the asset's
/// decimals: 1.0 of the asset, and never fewer base units than 1,000 times `DEAD_SHARES`, the
/// ratio the dead-shares defence relies on. On a 9-decimal asset (MXNe) that is 1.0, not 0.001.
#[test]
fn first_deposit_minimum_is_one_unit_of_the_asset_with_a_dead_shares_floor() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();

    // Nine decimals: the minimum is 10^9 base units, one whole unit.
    let mint = env.create_mint(&admin.pubkey(), 9);
    let ix = env.init_pool_ix(&admin.pubkey(), &mint);
    env.send(ix, &admin, &[]);
    let k = PoolKeys::new(&admin.pubkey(), &mint);
    assert_eq!(env.mint_decimals(&k.share_mint), 9);
    let investor = env.funded(10);
    env.mint_to(&mint, &admin, &investor.pubkey(), 2_000_000_000);
    env.create_ata(&investor.pubkey(), &k.share_mint);
    let ix = env.deposit_ix(&k, &investor.pubkey(), 999_999_999); // 0.999999999
    env.expect_err(ix, &investor, &[], E_FIRST_DEPOSIT_TOO_SMALL);
    env.deposit(&k, &investor, 1_000_000_000); // 1.0

    // Two decimals: 1.0 would be 100 base units, below the dead shares; the floor of
    // 1,000 x DEAD_SHARES = 1_000_000 base units (10,000.00) applies instead.
    let mint = env.create_mint(&admin.pubkey(), 2);
    let ix = env.init_pool_ix(&admin.pubkey(), &mint);
    env.send(ix, &admin, &[]);
    let k = PoolKeys::new(&admin.pubkey(), &mint);
    let investor = env.funded(10);
    env.mint_to(&mint, &admin, &investor.pubkey(), 1_000_000);
    env.create_ata(&investor.pubkey(), &k.share_mint);
    let ix = env.deposit_ix(&k, &investor.pubkey(), 999_999);
    env.expect_err(ix, &investor, &[], E_FIRST_DEPOSIT_TOO_SMALL);
    env.deposit(&k, &investor, 1_000_000);
}

/// Late interest is computed at the second the repayment executes. A client that reads
/// `late_due`, then lands the transaction a little later, is refused with `LateInterestUnpaid`
/// unless it pads the amount (SECURITY.md: "Late interest timing").
#[test]
fn limit_late_interest_is_settled_at_execution_time_so_clients_must_pad() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_with_grace(0);
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // One installment missed for a while: late interest is accruing.
    env.warp(env.now() + MONTH + 1_000);
    let quoted = env.arrears(&k, line_id).late_due;
    assert!(quoted > 0);
    // The transaction lands a minute later: the quote is stale by a few base units.
    env.warp(env.now() + 60);
    let due_now = env.arrears(&k, line_id).late_due;
    assert!(due_now > quoted);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), quoted);
    env.expect_err(ix, &borrower, &[], E_LATE_INTEREST_UNPAID);
    // Padded, it settles; the program debits only what is due.
    let before = env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint));
    env.repay(&k, line_id, &borrower, due_now + 100_000);
    assert_eq!(before - env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), due_now + 100_000);
}
