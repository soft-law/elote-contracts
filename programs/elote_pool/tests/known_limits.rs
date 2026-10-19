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

/// The admin may name itself as the borrower and draw the whole vault. Nothing in the program
/// bounds a line's principal or separates the admin from borrowers (roadmap: "One admin key").
#[test]
fn limit_admin_can_open_a_line_to_itself_for_the_whole_vault() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let admin = env.admin.insecure_clone();
    env.create_ata(&admin.pubkey(), &k.asset_mint);

    let p = env.line_params(10 * UNIT, 0);
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &admin.pubkey(), &p, None);
    env.send(ix, &admin, &[]);
    let ix = env.draw_ix(&k, 0, &admin.pubkey());
    env.send(ix, &admin, &[]);

    assert_eq!(env.token_balance(&k.vault), 0, "the vault is empty");
    assert_eq!(env.token_balance(&ata(&admin.pubkey(), &k.asset_mint)), 10 * UNIT);
    assert_eq!(env.line(&k, 0).borrower, admin.pubkey());
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

/// `MIN_FIRST_DEPOSIT` is a raw amount of 1_000_000 base units, meant as 1.0 of a six-decimal
/// asset. On a two-decimal asset it is 10,000.00 (roadmap: "Any SPL asset").
#[test]
fn limit_first_deposit_minimum_assumes_six_decimals() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let mint = env.create_mint(&admin.pubkey(), 2);
    let ix = env.init_pool_ix(&admin.pubkey(), &mint);
    env.send(ix, &admin, &[]);
    let k = PoolKeys::new(&admin.pubkey(), &mint);
    assert_eq!(env.mint_decimals(&k.share_mint), 2);

    let investor = env.funded(10);
    env.mint_to(&mint, &admin, &investor.pubkey(), 1_000_000);
    env.create_ata(&investor.pubkey(), &k.share_mint);
    let ix = env.deposit_ix(&k, &investor.pubkey(), 999_999); // 9,999.99
    env.expect_err(ix, &investor, &[], E_FIRST_DEPOSIT_TOO_SMALL);
    env.deposit(&k, &investor, 1_000_000); // 10,000.00
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
