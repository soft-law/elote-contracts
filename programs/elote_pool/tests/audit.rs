//! Regression tests for the findings of the 2026-11-01 audit (docs/audits/2026-11-01-elote-pool-audit.md).

mod common;

use common::*;
use elote_pool::state::LineStatus;
use solana_signer::Signer;

/// F-01: a permissionless `repay` used to close an `Active` line whose draw window was still open,
/// forfeiting the borrower's undrawn commitment for the price of its outstanding debt (one base
/// unit here). Now a line with no balance stays `Active` while it can still draw; it becomes
/// `Repaid` only once nothing is due and nothing can be drawn (the window closed, or the
/// commitment is fully drawn), and a repayment of a zero debt is refused until then.
#[test]
fn f01_a_third_party_repayment_does_not_close_a_line_that_can_still_draw() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);

    // The borrower draws one base unit; its interest rounds to zero.
    env.draw_amount(&k, line_id, &borrower, 1);
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.drawn, line.interest_due), (LineStatus::Active, 1, 0));

    // A stranger repays it: the debt is gone, the line is not.
    let stranger = env.investor(&k, 2);
    let ev = env.repay(&k, line_id, &stranger, 1);
    assert_eq!((ev.payer, ev.principal_part, ev.remaining), (stranger.pubkey(), 1, 0));
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::Active, "nothing is due, but the window is open");
    assert_eq!((line.closed_at, line.available()), (0, Some(4 * UNIT - 1)));
    assert_eq!(env.pool(&k).outstanding_principal, 0);

    // Nothing is due and the line can still draw: a repayment is refused.
    let ix = env.repay_ix(&k, line_id, &stranger.pubkey(), 1);
    env.expect_err(ix, &stranger, &[], E_WRONG_AMOUNT);

    // The borrower keeps drawing against its commitment.
    env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).drawn, UNIT + 1);
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), UNIT);
    let interest = env.line(&k, line_id).interest_due;
    env.repay(&k, line_id, &borrower, UNIT + interest);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Active, "paid again, still open");

    // Once the window has closed, a repayment with nothing due closes the line and moves nothing.
    env.warp(p.term_end);
    let before = env.token_balance(&k.asset_ata(&stranger.pubkey()));
    let ev = env.repay(&k, line_id, &stranger, 1);
    assert_eq!((ev.principal_part, ev.interest_part, ev.remaining), (0, 0, 0));
    assert_eq!(env.token_balance(&k.asset_ata(&stranger.pubkey())), before, "nothing moved");
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.closed_at), (LineStatus::Repaid, env.now()));
}

/// F-01, the other exit: a line drawn in full closes with its last unit, window open or not, and a
/// line whose window the admin suspended closes with its last unit too.
#[test]
fn f01_a_full_repayment_closes_a_line_that_cannot_draw_any_more() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let admin = env.admin.insecure_clone();

    // Fully drawn: nothing left to draw, so the last unit repaid closes it.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let (full, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, full, &borrower);
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), UNIT);
    let interest = env.line(&k, full).interest_due;
    env.repay(&k, full, &borrower, 2 * UNIT + interest);
    assert_eq!(env.line(&k, full).status, LineStatus::Repaid);

    // Half drawn, then the admin closes the window: the last unit repaid closes it.
    let borrower = env.borrower(&k);
    let (half, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, half, &borrower, UNIT);
    let interest = env.line(&k, half).interest_due;
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), UNIT);
    env.repay(&k, half, &borrower, UNIT + interest);
    assert_eq!(env.line(&k, half).status, LineStatus::Active);
    let ix = env.suspend_draws_ix(&k, half, &admin.pubkey());
    env.send(ix, &admin, &[]);
    env.repay(&k, half, &borrower, 1);
    assert_eq!(env.line(&k, half).status, LineStatus::Repaid);
}

/// F-02: Core applies a collection's permanent plugins to every asset in it, so a collection
/// created with a collection-level `PermanentTransferDelegate` or `PermanentBurnDelegate` held by
/// a third key would let that key move or burn every pagaré once thawed. `init_pool` now accepts
/// only a bare collection: no plugin header (the account is exactly the base record) and nothing
/// minted into it yet.
#[test]
fn f02_init_pool_refuses_a_collection_with_plugins_or_minted_assets() {
    use mpl_core::types::{PermanentBurnDelegate, PermanentTransferDelegate, Plugin, PluginAuthority, PluginAuthorityPair};
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let stranger = env.funded(1);
    let by_stranger = Some(PluginAuthority::Address { address: stranger.pubkey() });

    // A collection with collection-level permanent delegates held by a stranger.
    let asset_mint = env.create_mint(&admin.pubkey(), 6);
    let mut k = env.pool_keys_for(&asset_mint);
    k.collection = env.create_collection_with(
        &k.pool,
        vec![
            PluginAuthorityPair { plugin: Plugin::PermanentTransferDelegate(PermanentTransferDelegate {}), authority: by_stranger.clone() },
            PluginAuthorityPair { plugin: Plugin::PermanentBurnDelegate(PermanentBurnDelegate {}), authority: by_stranger },
        ],
    );
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);

    // A collection with any plugin at all, even a harmless one.
    k.collection = env.create_collection_with(
        &k.pool,
        vec![PluginAuthorityPair { plugin: Plugin::ImmutableMetadata(mpl_core::types::ImmutableMetadata {}), authority: Some(PluginAuthority::None) }],
    );
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);

    // A bare collection that already holds an asset minted by its authority.
    let authority = env.funded(1);
    let minted = env.create_collection(&authority.pubkey());
    let asset = solana_keypair::Keypair::new();
    let ix = mpl_core::instructions::CreateV2Builder::new()
        .asset(asset.pubkey())
        .collection(Some(minted))
        .authority(Some(authority.pubkey()))
        .payer(authority.pubkey())
        .owner(Some(authority.pubkey()))
        .name("x".into())
        .uri(String::new())
        .instruction();
    env.send(ix, &authority, &[&asset]);
    // Hand it to the pool (the collection's update authority can move to the pool PDA).
    let ix = mpl_core::instructions::UpdateCollectionV1Builder::new()
        .collection(minted)
        .payer(authority.pubkey())
        .authority(Some(authority.pubkey()))
        .new_update_authority(Some(k.pool))
        .instruction();
    env.send(ix, &authority, &[]);
    k.collection = minted;
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.expect_err(ix, &admin, &[], E_COLLECTION_NOT_ALLOWED);

    // A bare, empty collection is accepted.
    k.collection = env.create_collection(&k.pool);
    let ix = env.init_pool_ix_for(&k, 0, BPS);
    env.send(ix, &admin, &[]);
    assert_eq!(env.pool(&k).collection, k.collection);
}

/// F-05: the two-signature model (an independent engine attests what the admin or the borrower
/// asserts) must not collapse into one key: the attestor cannot be the admin, at `init_pool` or at
/// `set_attestor`, and the borrower cannot be the attestor.
#[test]
fn f05_the_attestor_is_neither_the_admin_nor_a_borrower() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let asset_mint = env.create_mint(&admin.pubkey(), 6);
    let k = env.pool_keys_for(&asset_mint);
    let ix = env.init_pool_ix_with_attestor(&k, 0, BPS, admin.pubkey());
    env.expect_err(ix, &admin, &[], E_ATTESTOR_IS_ADMIN);

    let (k, _) = env.setup_pool();
    let ix = env.set_attestor_ix(&k, &admin.pubkey(), &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_ATTESTOR_IS_ADMIN);

    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let attestor = env.attestor.insecure_clone();
    env.create_ata(&attestor.pubkey(), &k.asset_mint);
    let p = env.line_params(UNIT, 1_000);
    let ix = env.open_line_ix(&k, &admin.pubkey(), 0, &attestor.pubkey(), &p, None);
    env.expect_err(ix, &admin, &[&attestor], E_BORROWER_IS_ATTESTOR);
    assert_eq!(env.pool(&k).line_count, 0);
}

/// F-03: the write-off of a line in collection used to be available the instant the keeper marked
/// it, since the grace of the third missed installment implies the grace of the first. The pool
/// now carries a collection period (at least 30 days, set at `init_pool`, changed by the admin),
/// and `write_off` on a line in collection waits for it from the mark, on top of the gates it had.
#[test]
fn f03_write_off_waits_for_the_pools_collection_period_from_the_mark() {
    use elote_pool::events::CollectionPeriodSet;
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();

    // Below the floor, the pool is not created and the period is not changed.
    let asset_mint = env.create_mint(&admin.pubkey(), 6);
    let k = env.pool_keys_for(&asset_mint);
    let ix = env.init_pool_ix_full(&k, 0, BPS, env.attestor.pubkey(), COLLECTION_PERIOD - 1, CANCEL_DELAY);
    env.expect_err(ix, &admin, &[], E_COLLECTION_PERIOD_TOO_SHORT);
    let (k, meta) = env.setup_pool();
    let ev: CollectionPeriodSet = find_event(&meta).expect("CollectionPeriodSet");
    assert_eq!((ev.pool, ev.collection_period_seconds), (k.pool, COLLECTION_PERIOD));
    assert_eq!(env.pool(&k).collection_period_seconds(), COLLECTION_PERIOD);
    let ix = env.set_collection_period_ix(&k, &admin.pubkey(), COLLECTION_PERIOD - 1);
    env.expect_err(ix, &admin, &[], E_COLLECTION_PERIOD_TOO_SHORT);
    let stranger = env.funded(1);
    let ix = env.set_collection_period_ix(&k, &stranger.pubkey(), 2 * COLLECTION_PERIOD);
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);

    // A line marked after three missed installments: the write-off waits 30 days from the mark.
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 10_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let t0 = env.line(&k, line_id).drawn_at;
    env.warp(t0 + 3 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &borrower, &[]);
    let marked_at = env.now();
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD));
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_WRITE_OFF_TOO_EARLY);

    // The admin lengthens the period: the wait grows with it, for a line already marked too.
    let ix = env.set_collection_period_ix(&k, &admin.pubkey(), 2 * COLLECTION_PERIOD);
    let meta = env.send(ix, &admin, &[]);
    let ev: CollectionPeriodSet = find_event(&meta).expect("CollectionPeriodSet");
    assert_eq!(ev.collection_period_seconds, 2 * COLLECTION_PERIOD);
    env.warp(marked_at + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.expect_err(ix, &admin, &[], E_WRITE_OFF_TOO_EARLY);
    env.warp(marked_at + 2 * i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
}
