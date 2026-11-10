//! Documentation tests: each one pins a known design limit of the program as it is deployed
//! today. They pass against the current code on purpose. None of them is a bug report; every
//! limit still in force here is listed under "Known risks by design" in SECURITY.md.
//!
//! When a limit is removed (two-step origination, roles, accrual, a cancel instruction, a
//! recovery instruction...), the matching test here must be inverted or deleted in the same
//! change, so the suite always states what the program does, not what it should do. Inverted
//! tests stay in this file, marked "Inverted <date>" in their doc comment, so the history of
//! what used to be a limit is readable next to what still is (four so far, 2026-11-05).

mod common;

use common::*;
use elote_pool::state::LineStatus;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

/// Inverted 2026-10-31. The council can no longer name itself as the borrower: the
/// one-key path "open a line to self, draw the vault" is closed. Everything else about the
/// council's lending power (any other borrower, no allowlist) still holds; see the trust table.
#[test]
fn the_council_cannot_be_the_borrower_of_its_own_line() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let council = env.council.insecure_clone();
    env.create_ata(&council.pubkey(), &k.asset_mint);

    let p = env.line_params(UNIT, 0);
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &council.pubkey(), &p, None);
    env.expect_err(ix, &council, &[], E_BORROWER_IS_COUNCIL);
    assert_eq!(env.pool(&k).line_count, 0);
    assert_eq!(env.token_balance(&k.vault), 10 * UNIT, "the vault is untouched");
}

/// Inverted 2026-10-31. A line for the whole vault is refused once
/// the pool carries a concentration cap: with the scripts' default of 1,000 bps, the largest
/// line is a tenth of the NAV. The harness opens its pools at 10,000 bps, where the cap is
/// inert, which is why this test sets its own.
#[test]
fn a_line_for_the_whole_vault_is_refused_under_the_concentration_cap() {
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let mint = env.create_mint(&council.pubkey(), 6);
    let (k, ix) = env.init_pool_full(&mint, 1_000);
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);

    let borrower = env.borrower(&k);
    let p = env.line_params(10 * UNIT, 0);
    let (line_id, _) = env.request_line_with(&k, &borrower, &p, None);
    let hash = env.terms_hash(&k, line_id, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council.pubkey(), line_id, p.expediente_root, hash);
    env.expect_err(ix, &council, &[], E_LINE_TOO_LARGE);
    assert_eq!(env.line(&k, line_id).status, elote_pool::state::LineStatus::Requested, "the cap is checked at the approval");
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
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 400_000);

    // The late investor enters at the pre-interest NAV of 10.0...
    let late = env.investor(&k, 10 * UNIT);
    let minted = env.deposit(&k, &late, 10 * UNIT).shares;
    assert_eq!(minted, 10 * UNIT, "one share per unit at NAV 1.0");
    // ...the borrower repays principal plus a year of interest on it at the term...
    env.warp(env.line(&k, line_id).term_end);
    env.repay(&k, line_id, &borrower, 4 * UNIT + 400_000);
    // ...and the late investor leaves with half of the interest the early one earned.
    let out = env.redeem(&k, &late, minted);
    assert_eq!(out.assets, 10 * UNIT + 200_000);
}

/// Inverted 2026-11-08 (decision 40). A loss used to count only at `write_off`: the mark left the
/// NAV at par, so a holder who redeemed between the two steps exited at the pre-loss value and
/// the loss fell on whoever stayed. The mark now provisions the line's outstanding cost in full,
/// so the quick and the patient redeem at the same provisioned price.
#[test]
fn redeem_between_collection_and_write_off_prices_the_provision() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let quick = env.investor(&k, 10 * UNIT);
    let quick_shares = env.deposit(&k, &quick, 10 * UNIT).shares; // minus the dead shares
    let patient = env.investor(&k, 10 * UNIT);
    let patient_shares = env.deposit(&k, &patient, 10 * UNIT).shares;
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // Three installments missed: anyone moves the line to collection, and the NAV provisions the
    // 6.0 outstanding at once.
    env.warp(env.now() + 3 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &quick, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    assert_eq!(env.nav(&k), 14 * UNIT);

    let supply = env.supply(&k);
    let out = env.redeem(&k, &quick, quick_shares);
    let quick_assets = out.assets;
    assert!(quick_assets < quick_shares, "below par: the provision already counts");
    assert_eq!(quick_assets, (u128::from(quick_shares) * u128::from(14 * UNIT) / u128::from(supply)) as u64);

    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let council = env.council.insecure_clone();
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    // The write-off moves nothing more, and the patient holder leaves at the same price.
    let out = env.redeem(&k, &patient, patient_shares);
    let per_share = |assets: u64, shares: u64| u128::from(assets) * 1_000_000 / u128::from(shares);
    assert_eq!(per_share(out.assets, patient_shares), per_share(quick_assets, quick_shares), "the loss was shared the moment of the mark");
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
/// `Overflow` (a division by the zero NAV). Value entering the vault reopens the pool: a raw
/// transfer, or since decision 51 a recovery on the written-off line.
#[test]
fn limit_total_write_off_leaves_a_pool_that_cannot_take_deposits() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_capped(BPS); // the whole NAV lent to one line
    let investor = env.investor(&k, 2 * UNIT);
    env.deposit(&k, &investor, 2 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 0);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(p.term_end + 1);
    let council = env.council.insecure_clone();
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    assert_eq!((env.nav(&k), env.supply(&k)), (0, 2 * UNIT));

    let newcomer = env.investor(&k, UNIT);
    let ix = env.deposit_ix(&k, &newcomer.pubkey(), UNIT);
    env.expect_err(ix, &newcomer, &[], E_OVERFLOW);
    // A recovery can only arrive as an untracked donation, and then deposits work again.
    env.donate(&k, UNIT);
    let minted = env.deposit(&k, &newcomer, UNIT).shares;
    assert_eq!(minted, 2 * UNIT, "priced against the donated NAV of 1.0 and the old supply of 2.0");
    // Decision 51: the borrower's money returns as a recovery, which comes off the loss.
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.pool(&k).written_off, UNIT);
}

/// Inverted 2026-11-08 (decision 41). Collection used to need `covered + max_missed <=
/// installments`, so a borrower who paid early installments ahead and then stopped could never be
/// moved to collection. Now the requirement clamps to the installments that remain: the line is
/// marked once every remaining installment is missed, one second after the last due date.
#[test]
fn collection_is_reachable_when_the_final_installments_are_the_missed_ones() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_capped(BPS); // a line of 6 of 10
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 3, 5_000); // 2.0 of principal in each of three, max_missed 3
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.repay(&k, line_id, &borrower, 2 * UNIT); // exactly one installment, paid ahead at the draw (no interest yet)
    assert_eq!(env.arrears(&k, line_id).covered, 1);

    env.warp(p.term_end + 10 * 365 * 86_400);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &investor, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
}

/// Decision 41: a line with four of six installments paid ahead and the two final ones unpaid is
/// marked only after the last due date, not a second before.
#[test]
fn a_line_paid_ahead_is_marked_only_after_its_last_due_date() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_capped(BPS);
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000); // six installments, max_missed 3
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    // Four installments of principal paid ahead in the second of the draw, before any interest
    // accrued (decision 48: the schedule is on principal).
    env.repay(&k, line_id, &borrower, 4 * UNIT);
    assert_eq!(env.arrears(&k, line_id).covered, 4);

    // At the last due date exactly, the final installment is not yet missed.
    env.warp(p.term_end);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.expect_err(ix, &investor, &[], E_NOT_ENOUGH_MISSED);

    // One second past it, both remaining installments are missed and the mark lands.
    env.warp(p.term_end + 1);
    assert_eq!(env.arrears(&k, line_id).missed, 2);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &investor, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
}

/// Late interest is computed at the second the repayment executes. A client that reads
/// `late_due`, then lands the transaction a little later, is refused with `LateInterestUnpaid`
/// unless it pads the amount (SECURITY.md: "Late interest timing").
#[test]
fn limit_late_interest_is_settled_at_execution_time_so_clients_must_pad() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_capped(BPS); // a line of 6 of 10
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // One installment missed for a while: late interest is accruing.
    env.warp(env.now() + p.interval() + 1_000);
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

/// Limit: `draw` takes the pagaré PDA bumps from the client
/// and verifies each with one `create_program_address`, so its compute does not depend on the
/// line's key. The price: a valid bump that is not the canonical one (the highest valid) is
/// accepted too, and the asset and sidecar then sit at addresses `find_program_address` never
/// derives. Proving a bump canonical costs the same search that was removed. The client
/// derives the canonical bump, and the `PagareIssued` event and the
/// sidecar carry the asset's address, so indexers never need to derive it; a non-canonical
/// mint is a client bug, not an attack, and this test pins that it is possible.
#[test]
fn limit_draw_accepts_a_valid_non_canonical_pagare_bump() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 0);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    let line_key = line_pda(&k.pool, line_id).0;

    let (canonical, canonical_bump) = pagare_asset_pda(&line_key, 0);
    let (odd, odd_bump) = (0..canonical_bump)
        .rev()
        .find_map(|b| {
            Pubkey::create_program_address(&[b"pagare_asset", line_key.as_ref(), &[0, 0], &[b]], &PROGRAM_ID).ok().map(|a| (a, b))
        })
        .expect("a second valid bump exists for practically every seed");
    assert_ne!(odd, canonical);

    let b = borrower.pubkey();
    let pagare_bump = pagare_pda(&line_key, 0).1;
    let mut ix = env.draw_ix_full(&k, line_id, &b, &b, UNIT, odd_bump, pagare_bump, CERT_FINGERPRINT, SIGNATURE_METHOD);
    let i = ix.accounts.iter().position(|m| m.pubkey == canonical).unwrap();
    ix.accounts[i].pubkey = odd;
    env.send(ix, &borrower, &[]);
    let sidecar = env.pagare(&k, line_id, 0);
    assert_eq!(sidecar.asset, odd, "the sidecar and the event name the asset, wherever it sits");
    assert_eq!(env.asset_state(&odd).base.owner, k.pool);
    assert!(env.svm.get_account(&canonical).is_none(), "nothing at the canonical address");
}

/// Inverted 2026-11-05. `repay` takes at most `MAX_SIDECARS_PER_REPAY` (3) pagaré records, on
/// chain: four or more are refused with `TooManySidecars` before anything moves, whatever the
/// amount. The client splits a larger payment (the amount is free) into parts of at most three
/// records, each under the project's 100 000 CU ceiling (`tests/compute.rs` pins three with a
/// margin).
#[test]
fn a_repay_takes_at_most_three_pagare_records() {
    use elote_pool::state::MAX_SIDECARS_PER_REPAY;
    const CEILING: u64 = 100_000;
    assert_eq!(MAX_SIDECARS_PER_REPAY, 3);
    fn eight_draws(env: &mut Env) -> (PoolKeys, Keypair, u64) {
        let (k, _) = env.setup_pool();
        let investor = env.investor(&k, 200 * UNIT);
        env.deposit(&k, &investor, 200 * UNIT);
        let borrower = env.borrower(&k);
        let p = env.scheduled_params(80 * UNIT, 1_200, 120, 5_000);
        let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
        for _ in 0..8 {
            env.draw_amount(&k, line_id, &borrower, 10 * UNIT);
        }
        let t0 = env.line(&k, line_id).drawn_at;
        env.warp(t0 + 120 * p.interval() + 1);
        let owed = env.payoff(&k, line_id);
        let council = env.council.insecure_clone();
        env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed);
        (k, borrower, line_id)
    }

    // One payment for everything needs eight records, and one covering three pagarés needs four:
    // both refused, nothing moved.
    let mut env = Env::new();
    let (k, borrower, line_id) = eight_draws(&mut env);
    let owed = env.payoff(&k, line_id);
    let interest = env.line(&k, line_id).interest_unpaid(env.now()).unwrap();
    assert_eq!(env.payment_sidecars(&k, line_id, owed).len(), 8);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), owed);
    env.expect_err(ix, &borrower, &[], E_TOO_MANY_SIDECARS);
    let four = env.arrears(&k, line_id).late_due + interest + 30 * UNIT;
    assert_eq!(env.payment_sidecars(&k, line_id, four).len(), 4);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), four);
    env.expect_err(ix, &borrower, &[], E_TOO_MANY_SIDECARS);
    let after = env.line(&k, line_id);
    assert_eq!((after.principal_repaid, after.interest_repaid, after.pagares_paid), (0, 0, 0));

    // Split in four: records 0..=2 (2 untouched by principal), then 2..=4, 4..=6 and 6..=7.
    let first = env.arrears(&k, line_id).late_due + interest + 20 * UNIT;
    assert_eq!(env.payment_sidecars(&k, line_id, first).len(), 3);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), first);
    let mut cus = vec![env.send(ix, &borrower, &[]).compute_units_consumed];
    for _ in 0..2 {
        let part = env.arrears(&k, line_id).late_due + 20 * UNIT;
        assert_eq!(env.payment_sidecars(&k, line_id, part).len(), 3);
        let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), part);
        cus.push(env.send(ix, &borrower, &[]).compute_units_consumed);
    }
    let rest = env.payoff(&k, line_id);
    assert_eq!(env.payment_sidecars(&k, line_id, rest).len(), 2);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), rest);
    cus.push(env.send(ix, &borrower, &[]).compute_units_consumed);
    assert!(cus.iter().all(|cu| *cu < CEILING), "split: {cus:?} CU");
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.pagares_paid), (LineStatus::Repaid, 8));
}

/// `repay` takes exactly the pagaré records `[pagares_paid ..= last_touched]`, computed by the
/// client from the line when it builds the transaction. Another payment landing first can change
/// that set (here a stranger's one unit makes the borrower's amount cover pagaré 0 exactly, so
/// the set grows from `[0]` to `[0, 1]`), and the borrower's transaction fails with
/// `PagareAccountsMismatch`. Nothing moves; the client re-reads `pagares_paid` and the line and
/// retries (re-audit N-02).
#[test]
fn limit_a_payment_landing_first_makes_an_in_flight_repay_retry() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 20 * UNIT);
    env.deposit(&k, &investor, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(3 * UNIT, 1_000);
    let line_id = env.open_line_with(&k, &borrower, &p, None).0;
    for _ in 0..3 {
        env.draw_amount(&k, line_id, &borrower, UNIT);
    }
    env.warp(p.term_end);
    let council = env.council.insecure_clone();
    // A year on 3.0 at 10%: 300_000 of interest; one unit short of covering pagaré 0.
    let amount = 300_000 + UNIT - 1;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 10 * UNIT);
    let in_flight = env.repay_ix(&k, line_id, &borrower.pubkey(), amount);

    let stranger = env.funded(1);
    env.mint_to(&k.asset_mint, &council, &stranger.pubkey(), 10);
    env.repay(&k, line_id, &stranger, 1);
    env.expect_err(in_flight, &borrower, &[], E_PAGARE_ACCOUNTS_MISMATCH);

    // The retry, built from the line as it is now.
    let retry = env.repay_ix(&k, line_id, &borrower.pubkey(), amount);
    env.send(retry, &borrower, &[]);
    assert_eq!(env.line(&k, line_id).pagares_paid, 1);
}
