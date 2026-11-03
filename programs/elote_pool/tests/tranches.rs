//! Tranched draws on a line: partial draws until the
//! first due date, interest proportional to time per tranche, the schedule re-amortising what is
//! drawn, and the line's states around the tranches.

mod common;

use common::*;
use elote_pool::events::{LineCancelled, LineDrawsSuspended, PagareIssued};
use elote_pool::state::LineStatus;
use solana_signer::Signer;

/// A pool with one investor who deposited `assets` and `grace` seconds of grace.
fn pool_with(env: &mut Env, assets: u64, grace: u32) -> PoolKeys {
    let (k, _) = env.setup_pool_with_grace(grace);
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

fn open(env: &mut Env, k: &PoolKeys, borrower: &Keypair, p: &LineParams) -> u64 {
    env.open_line_with(k, borrower, p, None).0
}

/// `amount * rate_bps * seconds / (BPS * YEAR)`, the program's tranche interest, spelled out.
fn interest(amount: u64, rate_bps: u64, seconds: i64) -> u64 {
    (amount as u128 * rate_bps as u128 * seconds as u128 / (10_000 * YEAR as u128)) as u64
}

/// Two tranches add up to the commitment, a third is refused, and the pool's
/// outstanding principal and NAV move per tranche.
#[test]
fn two_tranches_sum_to_the_commitment_and_a_third_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
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

/// A tranche drawn with half the time left owes half the interest of one drawn
/// at the start, to the unit; a half-drawn line repaid in full needs exactly drawn + Σ interest.
#[test]
fn tranche_interest_is_proportional_to_the_time_to_the_last_due_date() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let t0 = env.now();

    let first = env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    assert_eq!(first.interest_due, 200_000, "2.0 at 10% for the whole year");
    env.warp(t0 + YEAR / 2);
    let second = env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    assert_eq!(second.interest_due - first.interest_due, 100_000, "the same amount for half the time");
    assert_eq!(env.line(&k, line_id).interest_due, 300_000);

    // Repaid in full: exactly 4.0 + 0.3, no more is taken.
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let before = env.token_balance(&k.asset_ata(&borrower.pubkey()));
    let ev = env.repay(&k, line_id, &borrower, 5 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (300_000, 4 * UNIT, 0));
    assert_eq!(before - env.token_balance(&k.asset_ata(&borrower.pubkey())), 4_300_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// On a scheduled line whose last due date is before `term_end`, the
/// tranche accrues to the last due date, not to the term.
#[test]
fn tranche_interest_runs_to_the_last_due_date_not_the_term() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let mut p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    p.term_end += YEAR; // the term outlives the calendar by a year
    let line_id = open(&mut env, &k, &borrower, &p);
    let last_due = p.first_due_at() + 5 * p.interval();
    assert!(last_due < p.term_end);

    let ev = env.draw_amount(&k, line_id, &borrower, 6 * UNIT);
    assert_eq!(ev.interest_due, interest(6 * UNIT, 1_200, last_due - env.now()));
    assert_eq!(ev.interest_due, 720_000, "to the last due date, exactly a year away");
    assert_ne!(ev.interest_due, interest(6 * UNIT, 1_200, p.term_end - env.now()));
}

/// A second tranche before the first due date raises every installment; the
/// arrears before and after are what `math::arrears` predicts.
#[test]
fn a_second_tranche_raises_every_installment() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    let line = env.line(&k, line_id);
    let total_one = line.drawn + line.interest_due;
    assert_eq!(total_one, 3_360_000, "3.0 plus 0.36 of interest");
    env.warp(p.first_due_at());
    let a = env.arrears(&k, line_id);
    assert_eq!((a.due, a.overdue), (1, total_one / 6));

    // A second tranche is refused now (the window closed at the first due date), so open another
    // line to see the installments stretch before its first due date.
    let p2 = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let second = open(&mut env, &k, &borrower, &p2);
    env.draw_amount(&k, second, &borrower, 3 * UNIT);
    env.warp(p2.first_due_at() - 1);
    env.draw_amount(&k, second, &borrower, 3 * UNIT);
    let line = env.line(&k, second);
    // The second tranche accrued for one second less than a whole-year tranche would.
    let expected_total = 6 * UNIT + 360_000 + interest(3 * UNIT, 1_200, p2.first_due_at() + 5 * p2.interval() - (p2.first_due_at() - 1));
    assert_eq!(line.drawn + line.interest_due, expected_total);
    env.warp(p2.first_due_at());
    let a = env.arrears(&k, second);
    assert_eq!((a.due, a.overdue), (1, expected_total / 6), "every installment is now a sixth of the larger total");
}

/// A tranche that would leave an installment below one unit is refused; the
/// tranche that brings `drawn + interest_due` to the installment count is accepted and repays.
#[test]
fn a_tranche_may_not_leave_the_schedule_unpayable() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(UNIT, 0, 12, 5_000); // 0%: no interest helps the count
    let line_id = open(&mut env, &k, &borrower, &p);

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 5);
    env.expect_err(ix, &borrower, &[], E_INVALID_SCHEDULE);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 11);
    env.expect_err(ix, &borrower, &[], E_INVALID_SCHEDULE);
    env.draw_amount(&k, line_id, &borrower, 12);
    assert_eq!(env.arrears(&k, line_id).due, 0);
    env.warp(p.first_due_at());
    assert_eq!(env.arrears(&k, line_id).overdue, 1, "one unit per installment");
    let ev = env.repay(&k, line_id, &borrower, 12);
    assert_eq!((ev.principal_part, ev.remaining), (12, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// Weekly, quincenal and monthly calendars each run a full cycle with two tranches,
/// three partial payments and one installment missed past the grace; `mark_in_collection`
/// behaves as before.
#[test]
fn weekly_quincenal_and_monthly_calendars_run_a_full_cycle() {
    for interval in [7 * 86_400i64, 15 * 86_400, 30 * 86_400] {
        let mut env = Env::new();
        let grace = 86_400u32;
        let k = pool_with(&mut env, 20 * UNIT, grace);
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
        let line = env.line(&k, line_id);
        let expected_interest = interest(4 * UNIT, 1_200, last_due - t0) + interest(2 * UNIT, 1_200, last_due - (t0 + interval / 2));
        assert_eq!(line.interest_due, expected_interest, "interval {interval}");
        let total = line.drawn + line.interest_due;
        let installment = total / u64::from(n);
        let council = env.council.insecure_clone();
        env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 2 * UNIT);

        // Three partial payments on the first due date add up to the first installment.
        env.warp(first_due);
        assert_eq!(env.arrears(&k, line_id).missed, 1);
        for part in [installment / 3, installment / 3, installment - 2 * (installment / 3)] {
            env.repay(&k, line_id, &borrower, part);
        }
        let a = env.arrears(&k, line_id);
        assert_eq!((a.covered, a.missed, a.late_due), (1, 0, 0), "interval {interval}");

        // Then nothing: the second, third and fourth are missed; collection waits for the grace
        // of the fourth (covered 1 + max_missed 3), as on any scheduled line.
        let fourth_due = first_due + 3 * interval;
        env.warp(fourth_due + i64::from(grace));
        let keeper = env.funded(1);
        let ix = env.mark_in_collection_ix(&k, line_id);
        env.expect_err(ix, &keeper, &[], E_NOT_ENOUGH_MISSED);
        env.warp(fourth_due + i64::from(grace) + 1);
        let a = env.arrears(&k, line_id);
        assert_eq!(a.missed, 3);
        assert!(a.late_due > 0, "late interest ran on the principal share of the second and third");
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
    let k = pool_with(&mut env, 10 * UNIT, 0);
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

    // In collection (three installments missed past the grace of the third): no tranche either,
    // and the window check does not even matter.
    env.warp(p.first_due_at() + 2 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &keeper, &[]);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

/// A half-drawn line repaid in full while its window is open stays `Active` and keeps drawing
/// (audit F-01); it closes only once nothing can be drawn.
#[test]
fn full_repayment_of_a_half_drawn_line_leaves_it_open_while_it_can_draw() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 200_000);

    let ev = env.repay(&k, line_id, &borrower, 2_200_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (200_000, 2 * UNIT, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.drawn, line.principal), (LineStatus::Active, 2 * UNIT, 4 * UNIT));
    assert!(env.now() < line.availability_end, "the window was still open");
    assert_eq!(env.pool(&k).outstanding_principal, 0);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!(env.pool(&k).outstanding_principal, UNIT);
    // The window closes: the next full repayment closes the line.
    env.warp(p.term_end);
    let interest = env.line(&k, line_id).interest_due - env.line(&k, line_id).interest_repaid;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, UNIT + interest);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// The write-off of a half-drawn line books `drawn - repaid`, and `MAX_DRAWS`
/// tranches are the limit.
#[test]
fn write_off_books_the_drawn_principal_and_max_draws_is_the_limit() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT, 0);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    env.repay(&k, line_id, &borrower, 700_000); // 0.2 of interest, 0.5 of principal
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
    let k = pool_with(&mut env, 10 * UNIT, 0);
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
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 100_000);
    let ev = env.repay(&k, line_id, &borrower, UNIT + 100_000);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (100_000, UNIT, 0));
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);

    // Not on a closed line; fine on an approved one nobody drew yet.
    let ix = env.suspend_draws_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
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
    let k = pool_with(&mut env, 10 * UNIT, 0);
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
    let k = pool_with(&mut env, 10 * UNIT, 0);
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
        [&ev.pagare_hash[..], &record.fx_rate.to_le_bytes(), &record.fx_date.to_le_bytes(), &record.usdc_delivered.to_le_bytes()].concat(),
    );
    assert_eq!((ev.delivery_hash[..] == delivery[..], record.delivery_hash), (true, ev.delivery_hash));

    // The same amount a second later is another pagaré.
    env.warp(t0 + 1);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    let ev2: PagareIssued = find_event(&env.send(ix, &borrower, &[])).unwrap();
    assert_ne!(ev2.pagare_hash, ev.pagare_hash);
}
