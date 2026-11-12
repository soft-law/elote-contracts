//! Tranched draws on a line: partial draws until the first due day, interest on each tranche
//! from its draw (decision 48: on the principal outstanding), the schedule re-amortising what is
//! drawn, and the line's states around the tranches.

mod common;
mod reference;

use common::*;
use elote_pool::events::{LineCancelled, LineDrawsSuspended, PagareIssued};
use elote_pool::state::LineStatus;
use solana_signer::Signer;

/// A pool with one investor who deposited `assets`.
fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    pool_capped(env, assets, HARNESS_MAX_LINE_BPS)
}

/// As `pool_with`, with the line cap `max_line_bps`.
fn pool_capped(env: &mut Env, assets: u64, max_line_bps: u16) -> PoolKeys {
    let (k, _) = env.setup_pool_capped(max_line_bps);
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

fn open(env: &mut Env, k: &PoolKeys, borrower: &Keypair, p: &LineParams) -> u64 {
    env.open_line_with(k, borrower, p, None).0
}

/// Two tranches add up to the commitment, a third is refused, and the pool's
/// outstanding principal and NAV move per tranche.
#[test]
fn two_tranches_sum_to_the_commitment_and_a_third_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 0);
    let line_id = open(&mut env, &k, &borrower, &p);

    let ev = env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!((ev.amount_mxn, ev.drawn, ev.available), (3 * UNIT, 3 * UNIT, UNIT));
    assert_eq!(env.pool(&k).outstanding_principal, 3 * UNIT);
    assert_eq!(env.nav(&k), 10 * UNIT, "a draw moves no value");
    assert_eq!(env.token_balance(&k.vault), 7 * UNIT);

    let ev = env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.drawn, ev.available), (4 * UNIT, 0));
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
    assert_eq!(env.token_balance(&k.asset_ata(&borrower.pubkey())), 4 * UNIT);

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    assert_eq!(env.line(&k, line_id).draws, 2);
}

/// A tranche drawn with half the time left accrues half the interest of one drawn at the start
/// (decision 48: interest runs on the principal from its draw); a half-drawn line repaid in full at
/// the term pays exactly drawn + 0.2 + 0.1.
#[test]
fn a_tranche_accrues_interest_from_its_draw() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let t0 = env.now();

    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    assert_eq!(env.line(&k, line_id).interest_accrued(p.term_end), Some(200_000), "2.0 at 10% for the whole year");
    env.warp(t0 + YEAR / 2);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    assert_eq!(env.line(&k, line_id).interest_accrued(p.term_end), Some(300_000), "and 2.0 more for half the year");

    // Repaid in full at the term: exactly 4.0 + 0.3, no more is taken.
    env.warp(p.term_end);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let before = env.token_balance(&k.asset_ata(&borrower.pubkey()));
    let ev = env.repay(&k, line_id, &borrower, 5 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (300_000, 4 * UNIT, 0));
    assert_eq!(before - env.token_balance(&k.asset_ata(&borrower.pubkey())), 4_300_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// On a scheduled line whose last due date is before `term_end`, ordinary interest accrues to the
/// last due date and stops there, not at the term (decision 48).
#[test]
fn interest_accrues_to_the_last_due_date_not_the_term() {
    let mut env = Env::new();
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
    let borrower = env.borrower(&k);
    let mut p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    p.term_end += YEAR; // the term outlives the calendar by a year
    let line_id = open(&mut env, &k, &borrower, &p);
    let last_due = p.first_due_at() + 5 * p.interval();
    assert!(last_due < p.term_end);
    assert_eq!(last_due - env.now(), YEAR);

    env.draw_amount(&k, line_id, &borrower, 6 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!(line.interest_accrued(last_due), Some(720_000), "a year of 12% on 6.0, to the last due date");
    assert_eq!(line.interest_accrued(p.term_end), Some(720_000), "and nothing between it and the term");
}

/// A second tranche before the first due date raises every installment of principal (decision 48);
/// the arrears before and after follow the principal drawn.
#[test]
fn a_second_tranche_raises_every_installment() {
    let mut env = Env::new();
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    env.warp(p.first_due_at());
    let a = env.arrears(&k, line_id);
    assert_eq!((a.due, a.overdue), (1, 500_000), "a sixth of 3.0");

    // A second tranche is refused now (the window closed at midnight in Mexico City of the first
    // due day), so open another line to see the installments stretch before its first due date.
    let p2 = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let second = open(&mut env, &k, &borrower, &p2);
    env.draw_amount(&k, second, &borrower, 3 * UNIT);
    env.warp(env.line(&k, second).availability_end - 1);
    env.draw_amount(&k, second, &borrower, 3 * UNIT);
    env.warp(p2.first_due_at());
    let a = env.arrears(&k, second);
    assert_eq!((a.due, a.overdue), (1, UNIT), "every installment is now a sixth of 6.0");
}

/// A tranche that would leave an installment below one centavo of principal is refused; the
/// tranche that brings `drawn` to the installment count is accepted and repays (decision 48).
#[test]
fn a_tranche_may_not_leave_the_schedule_unpayable() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(UNIT, 1_200, 12, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 5);
    env.expect_err(ix, &borrower, &[], E_INVALID_SCHEDULE);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 11);
    env.expect_err(ix, &borrower, &[], E_INVALID_SCHEDULE);
    env.draw_amount(&k, line_id, &borrower, 12);
    assert_eq!(env.arrears(&k, line_id).due, 0);
    env.warp(p.first_due_at());
    assert_eq!(env.arrears(&k, line_id).overdue, 1, "one centavo per installment");
    let ev = env.repay(&k, line_id, &borrower, 12);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 12, 0), "12 centavos accrue nothing in a month");
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// Weekly, quincenal and monthly calendars each run a full cycle with two tranches,
/// three partial payments and installments missed past their due dates; `mark_in_collection`
/// behaves as before.
#[test]
fn weekly_quincenal_and_monthly_calendars_run_a_full_cycle() {
    for interval in [7 * 86_400i64, 15 * 86_400, 30 * 86_400] {
        let mut env = Env::new();
        let k = pool_with(&mut env, 20 * UNIT);
        let borrower = env.borrower(&k);
        let n = 6u16;
        let mut p = env.scheduled_params(6 * UNIT, 1_200, n, 5_000);
        let first_due = env.now() + interval;
        p.schedule.as_mut().unwrap().interval_seconds = interval;
        p.schedule.as_mut().unwrap().first_due_at = first_due;
        p.term_end = first_due + i64::from(n) * interval;
        let line_id = open(&mut env, &k, &borrower, &p);
        let last_due = first_due + i64::from(n - 1) * interval;

        // Two tranches: at the opening and half-way to the first due date.
        let t0 = env.now();
        env.draw_amount(&k, line_id, &borrower, 4 * UNIT);
        env.warp(t0 + interval / 2);
        env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
        let mut r = reference::RefLine::new(1_200, last_due);
        r.draw(t0, 4 * UNIT);
        r.draw(t0 + interval / 2, 2 * UNIT);
        let council = env.council.insecure_clone();
        env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 2 * UNIT);

        // Three partial payments on the first due date add up to the interest accrued and the
        // first installment of principal, a sixth of 6.0.
        env.warp(first_due);
        assert_eq!(env.arrears(&k, line_id).missed, 1);
        let installment = r.interest_accrued(first_due) as u64 + UNIT;
        for part in [installment / 3, installment / 3, installment - 2 * (installment / 3)] {
            let ev = env.repay(&k, line_id, &borrower, part);
            let (_, i, pr) = r.pay(first_due, part, 0);
            assert_eq!((ev.interest_part, ev.principal_part), (i, pr), "interval {interval}");
        }
        let a = env.arrears(&k, line_id);
        assert_eq!((a.covered, a.missed, a.late_due), (1, 0, 0), "interval {interval}");

        // Then nothing: the second, third and fourth are missed; collection waits for the fourth's
        // due date to pass (covered 1 + max_missed 3), as on any scheduled line.
        let fourth_due = first_due + 3 * interval;
        env.warp(fourth_due);
        let keeper = env.funded(1);
        let ix = env.mark_in_collection_ix(&k, line_id);
        env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
        env.warp(fourth_due + 1);
        let a = env.arrears(&k, line_id);
        assert_eq!(a.missed, 3);
        assert!(a.late_due > 0, "late interest ran on the principal of the second and third");
        let ix = env.mark_in_collection_ix(&k, line_id);
        env.send(ix, &keeper, &[]);
        assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    }
}

/// No tranche while in collection, while paused, or after the window; a tranche
/// larger than the vault fails and succeeds after a deposit.
#[test]
fn tranches_are_refused_in_collection_paused_or_after_the_window() {
    let mut env = Env::new();
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
    let investor = env.investor(&k, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);

    env.set_paused(&k, true);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_PAUSED);
    env.set_paused(&k, false);

    // 5.0 of the 6.0 drawn, 5.0 left in the vault: a 6.0 tranche on a second line is bigger than
    // the vault until a deposit arrives.
    env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    let p2 = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let second = open(&mut env, &k, &borrower, &p2);
    let ix = env.draw_ix_amount(&k, second, &borrower.pubkey(), 6 * UNIT);
    env.expect_err(ix, &borrower, &[], E_INSUFFICIENT_LIQUIDITY);
    env.deposit(&k, &investor, 10 * UNIT);
    env.draw_amount(&k, second, &borrower, 6 * UNIT);

    // After the window: the first line's first due date has passed, with 1.0 still available.
    env.warp(p.first_due_at());
    assert_eq!(env.line(&k, line_id).available(), Some(UNIT));
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_AVAILABILITY_ENDED);

    // In collection (three installments missed, the third past its due date): no tranche either,
    // and the window check does not even matter.
    env.warp(p.first_due_at() + 2 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &keeper, &[]);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

/// A half-drawn line repaid in full while its window is open stays `Active` and keeps drawing
/// (audit F-01); it closes only once nothing can be drawn. The payment takes the interest accrued
/// and the principal, and leaves it owing nothing (decision 48).
#[test]
fn full_repayment_of_a_half_drawn_line_leaves_it_open_while_it_can_draw() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 200_000);

    // Half a year of 10% on 2.0: 0.1.
    env.warp(env.now() + YEAR / 2);
    let ev = env.repay(&k, line_id, &borrower, 2_200_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, 2 * UNIT, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.drawn, line.principal), (LineStatus::Active, 2 * UNIT, 4 * UNIT));
    assert!(env.now() < line.availability_end, "the window was still open");
    assert_eq!(env.pool(&k).outstanding_principal, 0);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!(env.pool(&k).outstanding_principal, UNIT);
    // The window closes: the next full repayment, half a year of 10% on 1.0 and the 1.0, closes
    // the line.
    env.warp(p.term_end);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 50_000);
    let ev = env.repay(&k, line_id, &borrower, UNIT + 50_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (50_000, UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// The write-off of a half-drawn line books `drawn - repaid`, and `MAX_DRAWS`
/// tranches are the limit.
#[test]
fn write_off_books_the_drawn_principal_and_max_draws_is_the_limit() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    // At the term a year of 10% on 2.0 has accrued: 0.2 of interest, 0.5 of principal.
    env.warp(p.term_end);
    env.repay(&k, line_id, &borrower, 700_000);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
    let ev: elote_pool::events::LineWrittenOff = find_event(&meta).unwrap();
    assert_eq!(ev.loss, 1_500_000);
    assert_eq!(env.pool(&k).written_off, 1_500_000);
    assert_eq!(env.pool(&k).outstanding_principal, 0);

    // Sixty-four tranches of one unit; the sixty-fifth is refused.
    let p = env.line_params(100, 0);
    let many = open(&mut env, &k, &borrower, &p);
    for _ in 0..64 {
        env.draw_amount(&k, many, &borrower, 1);
    }
    assert_eq!(env.line(&k, many).draws, 64);
    let ix = env.draw_ix_amount(&k, many, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_TOO_MANY_DRAWS);
    assert_eq!(env.line(&k, many).available(), Some(36));
}

/// The council closes the draw window early on a KYC or KYB failure or a default event. The
/// undrawn remainder is forfeited, repayment is untouched, and it is not a pause.
#[test]
fn suspend_draws_closes_the_window_early_and_leaves_repayment_alone() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);

    // Council only.
    let stranger = env.funded(1);
    let ix = env.suspend_draws_ix(&k, line_id, &stranger.pubkey());
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);

    let ix = env.suspend_draws_ix(&k, line_id, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
    let ev: LineDrawsSuspended = find_event(&meta).expect("LineDrawsSuspended");
    assert_eq!((ev.pool, ev.line_id, ev.available_forfeited), (k.pool, line_id, 3 * UNIT));
    let line = env.line(&k, line_id);
    assert_eq!((line.availability_end, line.status), (env.now(), LineStatus::Active));

    // No more tranches, even a second later; the pool is not paused and repayment works.
    env.warp(env.now() + 1);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_AVAILABILITY_ENDED);
    assert!(!env.pool(&k).paused);
    // At the term: a year of 10% on the 1.0 drawn.
    env.warp(env.line(&k, line_id).term_end);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 100_000);
    let ev = env.repay(&k, line_id, &borrower, UNIT + 100_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);

    // Not on a closed line; fine on an approved one nobody drew yet. (Fresh terms: the clock
    // moved to the first line's term for its settlement.)
    let ix = env.suspend_draws_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    let p = env.line_params(4 * UNIT, 1_000);
    let fresh = open(&mut env, &k, &borrower, &p);
    let ix = env.suspend_draws_ix(&k, fresh, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
    assert_eq!(find_event::<LineDrawsSuspended>(&meta).unwrap().available_forfeited, 4 * UNIT);
    let ix = env.draw_ix_amount(&k, fresh, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_AVAILABILITY_ENDED);
    assert_eq!(env.line(&k, fresh).status, LineStatus::Approved);
}

/// Decision 25: an approved commitment nobody drew is cleared by the council. Once anything was
/// drawn there is something owed, and the line follows the repayment path instead.
#[test]
fn cancel_line_clears_an_undrawn_commitment_only() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    let stranger = env.funded(1);
    let ix = env.cancel_line_ix(&k, line_id, &stranger.pubkey());
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);

    let ix = env.cancel_line_ix(&k, line_id, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
    let ev: LineCancelled = find_event(&meta).expect("LineCancelled");
    assert_eq!((ev.pool, ev.line_id), (k.pool, line_id));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.closed_at, line.drawn), (LineStatus::Cancelled, env.now(), 0));
    assert_eq!(env.pool(&k).line_count, 1, "the id stays taken");

    // Terminal: no draw, no repayment, no write-off, no second cancel, no suspension.
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    let ix = env.cancel_line_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    let ix = env.suspend_draws_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);

    // A line with one tranche drawn is not cancellable; after the window closes it still owes.
    let p = env.line_params(4 * UNIT, 1_000);
    let drawn = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, drawn, &borrower, UNIT);
    let ix = env.cancel_line_ix(&k, drawn, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    assert_eq!(env.pool(&k).outstanding_principal, UNIT);
}

/// The pagaré hash is derived on chain from the approved template and terms, the line, the draw
/// index, the amount in MXN and the draw's time; the delivery hash binds the FIX and the USDC paid.
#[test]
fn a_draw_derives_the_pagare_hash_and_the_delivery_hash() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    let meta = env.send(ix, &borrower, &[]);
    let t0 = env.now();
    let ev: PagareIssued = find_event(&meta).unwrap();
    let line = env.line(&k, line_id);
    let key = line_pda(&k.pool, line_id).0;
    let manual = <sha2::Sha256 as sha2::Digest>::digest(
        [&line.pagare_template_hash[..], &line.terms_hash, key.as_ref(), &0u16.to_le_bytes(), &UNIT.to_le_bytes(), &t0.to_le_bytes()].concat(),
    );
    assert_eq!(ev.pagare_hash[..], manual[..]);
    let record = env.pagare(&k, line_id, 0);
    let delivery = <sha2::Sha256 as sha2::Digest>::digest(
        [&ev.pagare_hash[..], &record.fix_rate.to_le_bytes(), &record.fix_date.to_le_bytes(), &record.usdc_delivered.to_le_bytes()].concat(),
    );
    assert_eq!((ev.delivery_hash[..] == delivery[..], record.delivery_hash), (true, ev.delivery_hash));

    // The same amount a second later is another pagaré.
    env.warp(t0 + 1);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    let ev2: PagareIssued = find_event(&env.send(ix, &borrower, &[])).unwrap();
    assert_ne!(ev2.pagare_hash, ev.pagare_hash);
}

/// A pool with a `max_line_bps` concentration cap, FIX `fix_rate` published today by the council,
/// and `assets` deposited by one investor. The harness does not keep the FIX current.
fn capped_pool(env: &mut Env, max_line_bps: u16, fix_rate: u64, assets: u64) -> PoolKeys {
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let (k, ix) = env.init_pool_full(&asset, max_line_bps);
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), fix_rate, env.today());
    env.send(ix, &council, &[]);
    let investor = env.investor(&k, assets);
    env.deposit(&k, &investor, assets);
    k
}

/// Requests and approves `p` on a pool whose FIX the test sets itself.
fn open_at_own_fix(env: &mut Env, k: &PoolKeys, borrower: &Keypair, p: &LineParams) -> u64 {
    let (id, _) = env.request_line_with(k, borrower, p, None);
    env.release_fix(k);
    env.approve_line(k, id, &borrower.pubkey(), p);
    id
}

/// The concentration cap is measured in USDC when the money leaves: the line's USDC drawn after the
/// tranche stays within `max_line_bps` of the NAV at the draw, whatever the line's MXN principal.
#[test]
fn a_tranche_past_the_cap_in_usdc_is_refused_though_the_line_fits_in_mxn() {
    let mut env = Env::new();
    // 10% of 1,000 USDC: 100 USDC, MXN 2,000.00 at 20.0000.
    let k = capped_pool(&mut env, 1_000, 200_000, 1_000 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(200_000, 0);
    let id = open_at_own_fix(&mut env, &k, &borrower, &p);
    let ev = env.draw_amount(&k, id, &borrower, 100_000);
    assert_eq!(ev.usdc_delivered, 50 * UNIT, "MXN 1,000.00 at 20.0000");

    // The peso strengthens to 18.0000: MXN 1,000.00 is 55.56 USDC, past the 50 USDC left.
    env.warp(env.now() + 86_400);
    let council = env.council.insecure_clone();
    let ix = env.post_fix_ix(&k, &council.pubkey(), 180_000, env.today());
    env.send(ix, &council, &[]);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 100_000);
    env.expect_err(ix, &borrower, &[], E_LINE_TOO_LARGE);
    assert!(env.line(&k, id).available().unwrap() >= 100_000, "the line still has the MXN");
    // MXN 900.00 is exactly the 50 USDC left.
    let ev = env.draw_amount(&k, id, &borrower, 90_000);
    assert_eq!((ev.usdc_delivered, env.line(&k, id).usdc_drawn), (50 * UNIT, 100 * UNIT));
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 1_000);
    env.expect_err(ix, &borrower, &[], E_LINE_TOO_LARGE);
}

/// A deposit redeemed after the approval does not carry the cap it raised to the draw.
#[test]
fn a_deposit_redeemed_after_the_approval_does_not_raise_the_cap_at_the_draw() {
    let mut env = Env::new();
    // 50% of the NAV per line, at parity (one centavo is one USDC base unit).
    let k = capped_pool(&mut env, 5_000, PARITY_FIX, 400 * UNIT);
    let whale = env.investor(&k, 600 * UNIT);
    env.deposit(&k, &whale, 600 * UNIT);
    assert_eq!(env.nav(&k), 1_000 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(500 * UNIT, 0);
    let id = open_at_own_fix(&mut env, &k, &borrower, &p);
    let shares = env.token_balance(&k.share_ata(&whale.pubkey()));
    env.redeem(&k, &whale, shares);
    assert_eq!((env.nav(&k), env.token_balance(&k.vault)), (400 * UNIT, 400 * UNIT));

    // 50% of 400 USDC: 200 USDC, though the vault holds 400 and the line 500.
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 200 * UNIT + 1);
    env.expect_err(ix, &borrower, &[], E_LINE_TOO_LARGE);
    env.draw_amount(&k, id, &borrower, 200 * UNIT);
    let ix = env.draw_ix_amount(&k, id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_TOO_LARGE);
}

/// `approve_line` converts the cap at the FIX in force, so it needs the same freshness as a draw.
#[test]
fn approve_line_needs_a_fix_at_most_five_days_old() {
    let mut env = Env::new();
    let k = capped_pool(&mut env, 5_000, PARITY_FIX, 100 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(10 * UNIT, 0);
    let (a, _) = env.request_line_with(&k, &borrower, &p, None);
    let (b, _) = env.request_line_with(&k, &borrower, &p, None);
    env.release_fix(&k);
    env.warp(env.now() + 5 * 86_400);
    env.approve_line(&k, a, &borrower.pubkey(), &p);
    env.warp(env.now() + 86_400);
    let council = env.council.insecure_clone();
    let hash = env.terms_hash(&k, b, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council.pubkey(), b, p.expediente_root, hash);
    env.expect_err(ix, &council, &[], ERR_BASE + elote_pool::errors::PoolError::FixStale as u32);
    env.ensure_fix(&k);
    env.approve_line(&k, b, &borrower.pubkey(), &p);
}
