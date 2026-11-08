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

/// Decision 40b, Maple's dual price: a deposit is priced at the NAV with no provision (the vault
/// plus the whole outstanding cost), a redemption at the provisioned NAV. Whoever enters while a
/// line is impaired buys that book at par, so a later recovery hands them nothing: the sandwich —
/// flash or patient — nets zero or less, and deposits are never gated.
#[test]
fn a_deposit_prices_at_the_unprovisioned_nav_and_a_recovery_hands_it_nothing() {
    let mut env = Env::new();
    let (k, line_id, outstanding, borrower) = marked(&mut env);
    // Provisioned NAV 6.0, unprovisioned 10.0.
    assert_eq!(env.nav(&k), 10 * UNIT - outstanding);

    let newcomer = env.investor(&k, 2 * UNIT);
    let supply_before = env.supply(&k);
    let minted = env.deposit(&k, &newcomer, 2 * UNIT).shares;
    // Shares priced against 10.0, not 6.0: 2.0 * supply / 10.0.
    assert_eq!(minted, (u128::from(2 * UNIT) * u128::from(supply_before) / u128::from(10 * UNIT)) as u64);

    // The borrower pays the line off in full. The provision reversal hands the entrant NOTHING
    // (they bought the impaired book at par); what they do share, pro rata like any holder, is the
    // payoff's interest and late interest — ordinary income, not the recovery.
    let line = env.line(&k, line_id);
    let owed = line.drawn - line.principal_repaid + line.interest_due - line.interest_repaid;
    let late = env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed + late);
    let ev = env.repay(&k, line_id, &borrower, owed + late);
    assert_eq!(env.pool(&k).impaired_principal, 0);
    let income = ev.usdc_received - outstanding; // interest + late; the principal leg is par
    let supply = env.supply(&k);
    let income_share = (u128::from(income) * u128::from(minted) / u128::from(supply)) as u64;
    let out = env.redeem(&k, &newcomer, minted);
    assert!(
        out.assets <= 2 * UNIT + income_share,
        "the reversal hands the entrant nothing beyond its income share: {} out of {} + {}",
        out.assets, 2 * UNIT, income_share
    );
    assert!(out.assets >= 2 * UNIT, "and the full recovery costs the entrant nothing either");
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
/// deposits stay open at the unprovisioned price (decision 40b), redemptions price at zero and
/// refuse, and the borrower can still repay — the recovery reopens the exits.
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

    // At a zero provisioned NAV the exits refuse...
    let i = env.redeem_ix(&k, &investor.pubkey(), UNIT);
    env.expect_err(i, &investor, &[], E_WRONG_AMOUNT);
    // ...but a deposit is still open, priced at the unprovisioned NAV (the whole outstanding at
    // par): the entrant knowingly buys the impaired book, so the shares come from gross 2.0 — and
    // the entrant's own cash is what lifts the provisioned NAV off zero.
    let newcomer = env.investor(&k, UNIT);
    let supply = env.supply(&k);
    let minted = env.deposit(&k, &newcomer, UNIT).shares;
    assert_eq!(minted, (u128::from(UNIT) * u128::from(supply) / u128::from(2 * UNIT)) as u64);
    assert_eq!(env.nav(&k), UNIT, "the newcomer's cash is the provisioned NAV now");
    // The borrower's repayment still lands, and what it recovers reopens the pool.
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert!(ev.usdc_received > 0);
    assert!(env.nav(&k) > 0);
}
