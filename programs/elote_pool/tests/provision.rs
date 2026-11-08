//! Decision 40: the mark provisions the NAV. The moment a line enters collection — by the
//! objective mark or the council's acceleration — the pool's NAV loses `provision_bps` of that
//! line's outstanding cost, so nobody redeems at a NAV that still counts a defaulted credit as
//! good. A recovery reverses its share of the provision; the write-off turns what remains of it
//! into the definitive loss. The rate starts at 100% and only the council moves it.

mod common;

use anchor_lang::InstructionData;
use common::*;
use elote_pool::events::{LineInCollection, ProvisionSet, Redeemed};
use elote_pool::instruction as ix;
use elote_pool::state::LineStatus;
use solana_signer::Signer;

/// A pool of 10 with a scheduled line of 4 drawn in full and three installments missed, marked.
/// Returns `(keys, line_id, usdc_outstanding_at_the_mark, borrower)`.
fn marked(env: &mut Env) -> (PoolKeys, u64, u64, Keypair) {
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let line = env.line(&k, line_id);
    let outstanding = line.usdc_drawn - line.usdc_cost_repaid;
    env.warp(p.first_due_at() + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &keeper, &[]);
    (k, line_id, outstanding, borrower)
}

#[test]
fn the_mark_provisions_the_nav_in_full() {
    let mut env = Env::new();
    // Before anything is marked, the NAV counts the full outstanding: 10.
    let nav_before = 10 * UNIT;
    let (k, line_id, outstanding, _) = marked(&mut env);
    let pool = env.pool(&k);
    assert_eq!(pool.provision_bps, 10_000, "init_pool starts the provision at 100%");
    assert_eq!(pool.impaired_principal, outstanding);
    assert_eq!(env.nav(&k), nav_before - outstanding, "the NAV lost the whole outstanding cost");
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
}

#[test]
fn the_mark_event_carries_the_pools_impairment() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(p.first_due_at() + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let ev: LineInCollection = find_event(&env.send(ix, &keeper, &[])).expect("LineInCollection");
    assert_eq!(ev.impaired_principal, env.pool(&k).impaired_principal);
    assert!(ev.impaired_principal > 0);
}

#[test]
fn a_redemption_after_the_mark_prices_at_the_provisioned_nav() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    let shares = env.deposit(&k, &investor, 10 * UNIT).shares;
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(p.first_due_at() + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let i = env.mark_in_collection_ix(&k, line_id);
    env.send(i, &keeper, &[]);

    // supply is shares + DEAD_SHARES; one tenth of the supply gets one tenth of the NAV.
    let supply = env.supply(&k);
    let nav = env.nav(&k);
    let redeemed: Redeemed = env.redeem(&k, &investor, supply / 10);
    assert_eq!(redeemed.assets, nav / 10, "priced against the provisioned NAV");
    assert!(shares > 0);
}

/// While anything is provisioned, deposits wait (the review's sandwich finding): a newcomer could
/// otherwise mint shares at the provisioned NAV and take the recovery — flash-amplified — from the
/// holders who bore the mark. Redemptions stay open; the gate lifts itself when the impairment
/// clears, by recovery or by write-off.
#[test]
fn deposits_wait_while_anything_is_provisioned() {
    let mut env = Env::new();
    let (k, line_id, _, borrower) = marked(&mut env);
    let newcomer = env.investor(&k, 2 * UNIT);
    let ix = env.deposit_ix(&k, &newcomer.pubkey(), UNIT);
    env.expect_err(ix, &newcomer, &[], ERR_BASE + elote_pool::errors::PoolError::PoolImpaired as u32);

    // The borrower pays the line off; the impairment clears and deposits reopen.
    let line = env.line(&k, line_id);
    let owed = line.drawn - line.principal_repaid + line.interest_due - line.interest_repaid;
    let late = env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed + late);
    env.repay(&k, line_id, &borrower, owed + late);
    assert_eq!(env.pool(&k).impaired_principal, 0);
    let ix = env.deposit_ix(&k, &newcomer.pubkey(), UNIT);
    env.send(ix, &newcomer, &[]);
}

#[test]
fn a_recovery_reverses_its_share_of_the_provision() {
    let mut env = Env::new();
    let (k, line_id, outstanding, borrower) = marked(&mut env);
    let nav_marked = env.nav(&k);

    // The payment settles the late interest, the interest and some principal. The vault gains the
    // whole payment; the cost recovered leaves both the outstanding and the provision, cancelling
    // out at 100%: the NAV rises by exactly what was paid in.
    let ev = env.repay(&k, line_id, &borrower, 2 * UNIT);
    let line = env.line(&k, line_id);
    let recovered = outstanding - (line.usdc_drawn - line.usdc_cost_repaid);
    assert!(ev.principal_part > 0 && recovered > 0);
    assert_eq!(env.pool(&k).impaired_principal, outstanding - recovered);
    assert_eq!(env.nav(&k), nav_marked + ev.usdc_received);
}

#[test]
fn the_write_off_turns_the_provision_into_the_loss() {
    let mut env = Env::new();
    let (k, line_id, outstanding, _) = marked(&mut env);
    let nav_marked = env.nav(&k);
    assert_eq!(env.pool(&k).impaired_principal, outstanding);

    env.warp(env.line(&k, line_id).accelerated_at + i64::from(COLLECTION_PERIOD) + 1);
    let council = env.council.insecure_clone();
    let i = env.write_off_ix(&k, line_id, &council.pubkey());
    env.send(i, &council, &[]);
    let pool = env.pool(&k);
    assert_eq!(pool.impaired_principal, 0, "the provision became the loss");
    assert_eq!(env.nav(&k), nav_marked, "already provisioned at 100%: the write-off moves nothing");
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}

#[test]
fn the_councils_acceleration_provisions_too() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let outstanding = {
        let line = env.line(&k, line_id);
        line.usdc_drawn - line.usdc_cost_repaid
    };
    env.council_line(&k, line_id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data());
    env.warp(env.line(&k, line_id).acceleration_due_at);
    env.council_line(&k, line_id, ix::AccelerateLine { valid_until: env.valid_until() }.data());
    assert_eq!(env.pool(&k).impaired_principal, outstanding);
    assert_eq!(env.nav(&k), 10 * UNIT - outstanding);
}

#[test]
fn the_provision_rate_is_the_councils_alone() {
    let mut env = Env::new();
    let (k, line_id, outstanding, _) = marked(&mut env);
    let council = env.council.insecure_clone();
    let stranger = env.funded(1);

    let i = env.council_pool_ix(&k, &stranger.pubkey(), ix::SetProvisionBps { provision_bps: 5_000, valid_until: env.valid_until() }.data());
    env.expect_err(i, &stranger, &[], E_UNAUTHORIZED);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::SetProvisionBps { provision_bps: 10_001, valid_until: env.valid_until() }.data());
    env.expect_err(i, &council, &[], E_WRONG_AMOUNT);
    let i = env.council_pool_ix(&k, &council.pubkey(), ix::SetProvisionBps { provision_bps: 5_000, valid_until: env.valid_until() }.data());
    let ev: ProvisionSet = find_event(&env.send(i, &council, &[])).expect("ProvisionSet");
    assert_eq!(ev.provision_bps, 5_000);
    assert_eq!(env.nav(&k), 10 * UNIT - outstanding / 2, "half the impairment provisions");
    assert!(line_id == env.line(&k, line_id).line_id);
}

/// With the whole NAV lent to the marked line, the provisioned NAV is zero while shares exist:
/// deposits refuse (the division by zero NAV), redemptions price at zero and refuse, and the
/// borrower can still repay — the same shape as the total write-off, a month earlier.
#[test]
fn a_fully_impaired_pool_prices_like_a_written_off_one() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool_capped(BPS);
    let investor = env.investor(&k, 2 * UNIT);
    env.deposit(&k, &investor, 2 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(2 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(p.first_due_at() + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let i = env.mark_in_collection_ix(&k, line_id);
    env.send(i, &keeper, &[]);
    assert_eq!((env.nav(&k), env.supply(&k) > 0), (0, true));

    let newcomer = env.investor(&k, UNIT);
    let i = env.deposit_ix(&k, &newcomer.pubkey(), UNIT);
    env.expect_err(i, &newcomer, &[], ERR_BASE + elote_pool::errors::PoolError::PoolImpaired as u32);
    let i = env.redeem_ix(&k, &investor.pubkey(), UNIT);
    env.expect_err(i, &investor, &[], E_WRONG_AMOUNT);
    // The borrower's repayment still lands, and what it recovers reopens the pool.
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert!(ev.usdc_received > 0);
    assert!(env.nav(&k) > 0);
}
