//! The pagaré token: one Metaplex Core asset per disposición, minted by `draw`
//! into the series' collection, held frozen by the pool PDA, with a sidecar record. These tests
//! create through the mpl-core crate's builders and read back through its decoders against the
//! pinned program binary, which is the crate-vs-program compatibility proof.

mod common;

use common::*;
use elote_pool::events::{PagareCancelled, PagareIssued};
use elote_pool::state::LineStatus;
use mpl_core::types::{Key, UpdateAuthority};
use mpl_core::{AuthorityType, BaseAuthority};
use anchor_lang::Space;
use solana_signer::Signer;

fn pool_with(env: &mut Env, assets: u64) -> PoolKeys {
    let (k, _) = env.setup_pool();
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    k
}

fn open(env: &mut Env, k: &PoolKeys, borrower: &Keypair, p: &LineParams) -> u64 {
    env.open_line_with(k, borrower, p, None).0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One asset and one sidecar per draw, frozen, owned by the pool, in the pool's collection; the
/// sidecar and the event carry the draw's facts; the relayer paid the rent.
#[test]
fn every_draw_issues_one_frozen_pagare_held_by_the_pool() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let relayer = env.funded(1);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    let last_due = p.first_due_at() + 5 * p.interval();

    let before = env.account(&relayer.pubkey()).lamports;
    let ix = env.draw_ix_paid_by(&k, line_id, &borrower.pubkey(), &relayer.pubkey(), 4 * UNIT);
    let meta = env.send(ix, &relayer, &[&borrower]);
    let t0 = env.now();
    let hash0 = env.expected_pagare_hash(&k, line_id, 0, 4 * UNIT, t0);
    let ev: PagareIssued = find_event(&meta).expect("PagareIssued");
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.pagare_hash, ev.amount_mxn, ev.due_at, ev.holder),
        (k.pool, line_id, 0, asset0, hash0, 4 * UNIT, last_due, k.pool)
    );
    assert!(env.account(&relayer.pubkey()).lamports < before - 5_000_000, "the relayer paid the rent of two accounts");

    let sidecar = env.pagare(&k, line_id, 0);
    assert_eq!((sidecar.line, sidecar.draw_index, sidecar.asset), (line_key, 0, asset0));
    assert_eq!((sidecar.pagare_hash, sidecar.amount_mxn, sidecar.issued_at, sidecar.due_at), (hash0, 4 * UNIT, t0, last_due));
    assert_eq!((sidecar.cert_fingerprint, sidecar.signature_method), (CERT_FINGERPRINT, SIGNATURE_METHOD));
    assert_eq!((sidecar.status, sidecar.paid_at, sidecar.receipt_hash), (PAGARE_OUTSTANDING, 0, [0; 32]), "outstanding until cancel_pagare");

    let asset = env.asset_state(&asset0);
    assert_eq!(asset.base.key, Key::AssetV1);
    assert_eq!(asset.base.owner, k.pool, "the pool PDA is the holder");
    // Core expresses the update authority of an asset in a collection through the collection,
    // and the collection's update authority is the pool PDA: the pool controls the asset.
    assert_eq!(asset.base.update_authority, UpdateAuthority::Collection(k.collection));
    assert_eq!(env.collection_state(&k.collection).update_authority, k.pool);
    assert_eq!(asset.base.name, format!("PAGARE {line_id}-0"), "no series label, nothing a court could read as an offering");
    assert_eq!(asset.base.uri, "");
    let freeze = asset.plugin_list.permanent_freeze_delegate.as_ref().expect("PermanentFreezeDelegate");
    assert!(freeze.permanent_freeze_delegate.frozen, "frozen from creation");
    assert_eq!(env.collection_state(&k.collection).num_minted, 1);

    // A second tranche: a second asset and sidecar, index 1, the collection counts two.
    env.warp(t0 + 1_000);
    let ix = env.draw_ix_paid_by(&k, line_id, &borrower.pubkey(), &relayer.pubkey(), UNIT);
    let meta = env.send(ix, &relayer, &[&borrower]);
    let ev: PagareIssued = find_event(&meta).expect("PagareIssued");
    let hash1 = env.expected_pagare_hash(&k, line_id, 1, UNIT, t0 + 1_000);
    let asset1 = pagare_asset_pda(&line_key, 1).0;
    assert_eq!((ev.draw_index, ev.asset, ev.pagare_hash, ev.amount_mxn), (1, asset1, hash1, UNIT));
    let sidecar = env.pagare(&k, line_id, 1);
    assert_eq!((sidecar.draw_index, sidecar.issued_at, sidecar.due_at), (1, t0 + 1_000, last_due));
    assert_eq!(env.asset_state(&asset1).base.owner, k.pool);
    assert_eq!(env.collection_state(&k.collection).num_minted, 2);
    assert_eq!(env.line(&k, line_id).draws, 2);
}

/// The compatibility proof the coordinator asked for: every plugin set at creation is read back
/// through the crate's decoder, with its authority and its content.
#[test]
fn every_plugin_of_a_fresh_pagare_reads_back_as_set() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 3 * UNIT);
    env.send(ix, &borrower, &[]);
    let issued_at = env.now();
    let hash = env.expected_pagare_hash(&k, line_id, 0, 3 * UNIT, issued_at);
    let line_key = line_pda(&k.pool, line_id).0;
    let asset = env.asset_state(&pagare_asset_pda(&line_key, 0).0);
    let pool_authority = |t: &BaseAuthority| {
        assert_eq!(t.authority_type, AuthorityType::Address);
        assert_eq!(t.address, Some(k.pool));
    };

    // Frozen, held by the pool, and the pool is the update authority through the collection.
    assert_eq!(asset.base.owner, k.pool);
    assert_eq!(asset.base.update_authority, UpdateAuthority::Collection(k.collection));
    assert_eq!(env.collection_state(&k.collection).update_authority, k.pool);

    // Permanent freeze delegate: frozen, authority = UpdateAuthority (the collection, so the pool
    // PDA).
    let freeze = asset.plugin_list.permanent_freeze_delegate.as_ref().expect("PermanentFreezeDelegate");
    assert!(freeze.permanent_freeze_delegate.frozen);
    assert_eq!(freeze.base.authority.authority_type, AuthorityType::UpdateAuthority);
    // No transfer or burn delegate: the pool is the owner while the asset is in the pool, and only
    // the program signs for it.
    assert!(asset.plugin_list.permanent_transfer_delegate.is_none());
    assert!(asset.plugin_list.permanent_burn_delegate.is_none());
    // AddBlocker, nobody's: the collection's authority cannot add plugins later.
    let blocker = asset.plugin_list.add_blocker.as_ref().expect("AddBlocker");
    assert_eq!(blocker.base.authority.authority_type, AuthorityType::None);
    // Immutable metadata, nobody's.
    let immutable = asset.plugin_list.immutable_metadata.as_ref().expect("ImmutableMetadata");
    assert_eq!(immutable.base.authority.authority_type, AuthorityType::None);
    // Attributes, nobody's, with the eight facts and nothing personal (the certificate commitment is not one of them).
    let attributes = asset.plugin_list.attributes.as_ref().expect("Attributes");
    assert_eq!(attributes.base.authority.authority_type, AuthorityType::None);
    let list: Vec<(&str, &str)> =
        attributes.attributes.attribute_list.iter().map(|a| (a.key.as_str(), a.value.as_str())).collect();
    let amount = (3 * UNIT).to_string();
    let issued = issued_at.to_string();
    let due = p.term_end.to_string();
    let line_s = line_id.to_string();
    assert_eq!(
        list,
        vec![
            ("pagare_hash", hex(&hash).as_str()),
            ("amount_mxn", amount.as_str()),
            ("currency", "MXN"),
            ("issued_at", issued.as_str()),
            ("due_at", due.as_str()),
            ("line", line_s.as_str()),
            ("draw_index", "0"),
            ("signature_method", "1"),
        ]
    );
    // Nothing else was set.
    assert!(asset.plugin_list.royalties.is_none());
    assert!(asset.plugin_list.freeze_delegate.is_none());
    assert!(asset.plugin_list.transfer_delegate.is_none());
    assert!(asset.plugin_list.burn_delegate.is_none());
    assert!(asset.plugin_list.update_delegate.is_none());
    // No external adapter: the registry lives in the sidecar.
    let adapters = &asset.external_plugin_adapter_list;
    assert!(adapters.app_data.is_empty() && adapters.lifecycle_hooks.is_empty() && adapters.oracles.is_empty() && adapters.data_sections.is_empty());
    let _ = pool_authority;
}

/// `drawn_through` of pagaré `k` is the line's `drawn` after tranche `k`, monotonic across `k`: the
/// coverage rule of `cancel_pagare` reads it instead of summing earlier sidecars.
#[test]
fn drawn_through_is_the_lines_drawn_after_each_tranche() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(6 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let mut total = 0;
    for (i, amount) in [UNIT, 2 * UNIT, 3 * UNIT].into_iter().enumerate() {
        env.draw_amount(&k, line_id, &borrower, amount);
        total += amount;
        let sidecar = env.pagare(&k, line_id, i as u16);
        assert_eq!((sidecar.amount_mxn, sidecar.drawn_through), (amount, total));
        assert_eq!(sidecar.drawn_through, env.line(&k, line_id).drawn);
    }
}

/// Condition C6a: the certificate fingerprint may be zero only for a wallet-only signature
/// (method 0); methods above 2 do not exist.
#[test]
fn a_zero_fingerprint_is_allowed_only_for_a_wallet_only_signature() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(6 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let b = borrower.pubkey();
    for (fingerprint, method) in [([0; 32], 1), ([0; 32], 2), (CERT_FINGERPRINT, 3), ([0; 32], 3)] {
        let ix = env.draw_ix_signed(&k, line_id, &b, UNIT, fingerprint, method);
        env.expect_err(ix, &borrower, &[], E_SIGNATURE_METHOD_INVALID);
    }
    for (i, (fingerprint, method)) in [([0; 32], 0), (CERT_FINGERPRINT, 0), (CERT_FINGERPRINT, 2)].into_iter().enumerate() {
        let ix = env.draw_ix_signed(&k, line_id, &b, UNIT, fingerprint, method);
        env.send(ix, &borrower, &[]);
        let sidecar = env.pagare(&k, line_id, i as u16);
        assert_eq!((sidecar.cert_fingerprint, sidecar.signature_method), (fingerprint, method));
    }
}

/// The pagaré addresses are predictable once the line is open, so anyone can send lamports to
/// them ahead of the draw, and both `create_account`s refuse a funded address. The program moves
/// whatever is there to the payer first, so the draw goes through, the payer is not out of pocket
/// and nobody can brick a line that way.
#[test]
fn a_draw_survives_pre_funded_pagare_addresses() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    let sidecar_rent = env.svm.minimum_balance_for_rent_exemption(8 + elote_pool::state::Pagare::INIT_SPACE);
    // Below the rent-exempt minimum the runtime refuses the transfer itself, so the smallest
    // funding that lands is rent-exempt for no data (below the sidecar's rent); the other case is
    // more than the sidecar's rent.
    for (i, (asset_funding, sidecar_funding)) in [(1_000_000_000, 1_000_000), (1_000_000, 1_000_000_000)].into_iter().enumerate() {
        let asset = pagare_asset_pda(&line_key, i as u16).0;
        let sidecar = pagare_pda(&line_key, i as u16).0;
        env.svm.airdrop(&asset, asset_funding).unwrap();
        env.svm.airdrop(&sidecar, sidecar_funding).unwrap();
        let before = env.account(&borrower.pubkey()).lamports;

        let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
        env.send(ix, &borrower, &[]);
        // The payer (here the borrower) received both strays and paid the fee and both rents out
        // of them: about one SOL richer; the sidecar holds exactly its rent.
        assert!(env.account(&borrower.pubkey()).lamports > before + 990_000_000);
        assert_eq!(env.account(&sidecar).lamports, sidecar_rent);
        let asset_state = env.asset_state(&asset);
        assert_eq!((asset_state.base.key, asset_state.base.owner), (Key::AssetV1, k.pool));
        let expected = env.expected_pagare_hash(&k, line_id, i as u16, UNIT, env.now());
        assert_eq!(env.pagare(&k, line_id, i as u16).pagare_hash, expected);
    }
    assert_eq!(env.line(&k, line_id).draws, 2);
}

/// The asset's addresses are derived from the line and the draw index, so a stale or foreign
/// asset or sidecar account is refused by the seeds before the Core program runs.
#[test]
fn a_draw_with_the_wrong_pagare_accounts_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;

    // The asset address of the next-but-one draw, or an arbitrary one.
    for wrong in [pagare_asset_pda(&line_key, 1).0, solana_pubkey::Pubkey::new_unique()] {
        let mut ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
        let i = ix.accounts.iter().position(|m| m.pubkey == pagare_asset_pda(&line_key, 0).0).unwrap();
        ix.accounts[i].pubkey = wrong;
        env.expect_err(ix, &borrower, &[], E_CONSTRAINT_SEEDS);
    }
    // A wrong bump for either account: the client derives both and the seeds constraints check
    // each with one `create_program_address`, so no bump search runs on chain.
    let (asset_bump, pagare_bump) = (pagare_asset_pda(&line_key, 0).1, pagare_pda(&line_key, 0).1);
    let b = borrower.pubkey();
    let ix = env.draw_ix_full(&k, line_id, &b, &b, UNIT, asset_bump.wrapping_sub(1), pagare_bump, CERT_FINGERPRINT, SIGNATURE_METHOD);
    env.expect_err(ix, &borrower, &[], E_CONSTRAINT_SEEDS);
    let ix = env.draw_ix_full(&k, line_id, &b, &b, UNIT, asset_bump, pagare_bump.wrapping_sub(1), CERT_FINGERPRINT, SIGNATURE_METHOD);
    env.expect_err(ix, &borrower, &[], E_CONSTRAINT_SEEDS);
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let mut ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    let i = ix.accounts.iter().position(|m| m.pubkey == k.collection).unwrap();
    ix.accounts[i].pubkey = other;
    env.expect_err(ix, &borrower, &[], E_WRONG_ACCOUNT);
    assert_eq!(env.line(&k, line_id).draws, 0);
    assert_eq!(env.collection_state(&k.collection).num_minted, 0);
}

// -----------------------------------------------------------------------------------------------
// cancel_pagare (6c; legal brief C22, LGTOC 17 and 129, CCF 2088)
// -----------------------------------------------------------------------------------------------

/// Opens a bullet line of `principal` at `rate_bps`, draws `tranches` and repays in full, so every
/// pagaré is covered.
fn drawn_and_repaid(env: &mut Env, k: &PoolKeys, principal: u64, rate_bps: u16, tranches: &[u64]) -> (Keypair, u64) {
    let borrower = env.borrower(k);
    let p = env.line_params(principal, rate_bps);
    let line_id = open(env, k, &borrower, &p);
    for amount in tranches {
        env.draw_amount(k, line_id, &borrower, *amount);
    }
    let line = env.line(k, line_id);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), line.interest_due);
    env.repay(k, line_id, &borrower, line.drawn + line.interest_due);
    assert_eq!(env.line(k, line_id).status, LineStatus::Repaid);
    (borrower, line_id)
}

/// Extinction is an act of the holder with a receipt: the record stays as the PAGADO mark (status,
/// the cancellation's receipt; it is not closed), the event names the asset and the receipt, and
/// the asset is thawed and burnt last by its owner, its rent going back to the payer. A second
/// cancellation is refused.
#[test]
fn cancel_pagare_marks_the_sidecar_paid_and_burns_the_asset() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let (_, line_id) = drawn_and_repaid(&mut env, &k, 4 * UNIT, 1_000, &[UNIT, 3 * UNIT]);
    let line_key = line_pda(&k.pool, line_id).0;
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    let sidecar_key = pagare_pda(&line_key, 0).0;
    let before = env.pagare(&k, line_id, 0);
    let sidecar_lamports = env.account(&sidecar_key).lamports;
    let asset_lamports = env.account(&asset0).lamports;
    assert_eq!(env.collection_state(&k.collection).current_size, 2);

    let council = env.council.insecure_clone();
    let relayer = env.funded(1);
    let relayer_before = env.account(&relayer.pubkey()).lamports;
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &council.pubkey(), &relayer.pubkey(), RECEIPT_HASH);
    let meta = env.send(ix, &relayer, &[&council]);
    let now = env.now();
    let ev: PagareCancelled = find_event(&meta).expect("PagareCancelled");
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.receipt_hash, ev.holder, ev.by_borrower),
        (k.pool, line_id, 0, asset0, RECEIPT_HASH, council.pubkey(), false)
    );

    // The sidecar: the mark written, everything else as issued, the account kept with its rent.
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.status, after.paid_at, after.receipt_hash), (PAGARE_PAID, now, RECEIPT_HASH));
    assert_eq!(
        (after.line, after.draw_index, after.asset, after.pagare_hash, after.amount_mxn, after.issued_at, after.due_at),
        (before.line, before.draw_index, before.asset, before.pagare_hash, before.amount_mxn, before.issued_at, before.due_at)
    );
    assert_eq!(
        (after.drawn_through, after.cert_fingerprint, after.signature_method, after.bump, after.procurador),
        (before.drawn_through, before.cert_fingerprint, before.signature_method, before.bump, before.procurador)
    );
    assert_eq!(env.account(&sidecar_key).lamports, sidecar_lamports, "the PAGADO mark is never closed");
    // The asset: burnt. Core leaves a one-byte `Uninitialized` tombstone (so the address cannot be
    // re-created as an asset) and sends the rest of the rent to the payer; the collection is one
    // smaller.
    let tomb = env.account(&asset0);
    assert_eq!((tomb.data.len(), tomb.data[0], tomb.owner), (1, Key::Uninitialized as u8, MPL_CORE_ID), "the asset is burnt");
    let returned = asset_lamports - tomb.lamports;
    let relayer_after = env.account(&relayer.pubkey()).lamports;
    assert!(returned > 2_000_000 && relayer_after > relayer_before && relayer_after <= relayer_before + returned, "the asset's rent returns to the payer");
    let collection = env.collection_state(&k.collection);
    assert_eq!((collection.num_minted, collection.current_size), (2, 1));
    // The other pagaré, marked paid by the repayment, cancels in turn.
    assert_eq!(env.pagare(&k, line_id, 1).status, elote_pool::state::PAGARE_PAID_PENDING_CANCEL);
    env.cancel_pagare(&k, line_id, 1);
    assert_eq!(env.pagare(&k, line_id, 1).status, PAGARE_PAID);
    assert_eq!(env.collection_state(&k.collection).current_size, 0);
    // Paid is terminal: the asset is gone and the mark says so.
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_ALREADY_PAID);
    assert_eq!(env.pagare(&k, line_id, 0).receipt_hash, RECEIPT_HASH);
}

/// The coverage rule: pagaré `k` is paid once the line's principal repaid reaches its
/// `drawn_through`, so repayments cover the pagarés oldest first and a later one cannot be
/// cancelled while an earlier one is uncovered. Interest covers nothing by itself: `repay` settles
/// it before principal, so a payment the size of the pagaré leaves principal owed, and a título is
/// never extinguished while any of its principal is.
#[test]
fn pagares_are_covered_oldest_first_by_the_principal_repaid() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();

    // At 0%: coverage is the principal repaid, to the unit.
    let borrower = env.borrower(&k);
    let p = env.line_params(6 * UNIT, 0);
    let line_id = open(&mut env, &k, &borrower, &p);
    for amount in [UNIT, 2 * UNIT, 3 * UNIT] {
        env.draw_amount(&k, line_id, &borrower, amount);
    }
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT - 1);
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 1);
    env.cancel_pagare(&k, line_id, 0);
    env.expect_cancel_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    env.expect_cancel_err(&k, line_id, 2, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 2 * UNIT);
    env.expect_cancel_err(&k, line_id, 2, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    // Order among covered pagarés is free: the last one first.
    env.cancel_pagare(&k, line_id, 2);
    env.cancel_pagare(&k, line_id, 1);
    assert_eq!(env.collection_state(&k.collection).current_size, 0);

    // At 10%, two tranches of 1.0 with 0.1 of interest each: a repayment of 1.0 settles the 0.2
    // of interest and only 0.8 of principal, so the first pagaré is not covered until 0.2 more
    // arrives.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (200_000, 800_000));
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 200_000);
    env.cancel_pagare(&k, line_id, 0);
    env.expect_cancel_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);

    // At 100% for a year the interest equals the principal: a payment the size of the pagaré is
    // all interest and covers nothing; the second payment repays the principal and closes the line.
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 10_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (UNIT, 0));
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.cancel_pagare(&k, line_id, 0);
}

/// Cancellation follows the sidecar's PAGADO mark, not the line's status (re-audit F-06): a
/// pagaré a payment covered cancels while its line is in collection or written off; the pagarés no payment covered stay in force for the
/// collection. A cancelled line never drew, so it has no sidecar to cancel.
#[test]
fn cancel_pagare_follows_the_mark_whatever_the_line_status() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();

    // In collection: a payment after the mark covers pagaré 0; it cancels, the line stays in
    // collection, pagaré 1 stays in force.
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let t0 = env.now();
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, 5 * UNIT);
    env.warp(t0 + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &keeper, &[]);
    let line = env.line(&k, line_id);
    let cover0 = env.arrears(&k, line_id).late_due + line.interest_due - line.interest_repaid + UNIT;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), cover0);
    env.repay(&k, line_id, &borrower, cover0);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    assert_eq!(env.pagare(&k, line_id, 0).status, elote_pool::state::PAGARE_PAID_PENDING_CANCEL);
    env.cancel_pagare(&k, line_id, 0);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_cancel_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);

    // Written off: the covered pagaré is cancelled; the other outlives the loss.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let cover0 = env.line(&k, line_id).interest_due + UNIT;
    env.repay(&k, line_id, &borrower, cover0);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &a);
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    env.cancel_pagare(&k, line_id, 0);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_cancel_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    assert_eq!(env.pagare(&k, line_id, 1).status, PAGARE_OUTSTANDING);

    // Cancelled: no sidecar was ever created.
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let ix = env.cancel_line_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    let line_key = line_pda(&k.pool, line_id).0;
    let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, pagare_asset_pda(&line_key, 0).0, k.collection, None);
    env.expect_err(ix, &council, &[], E_ACCOUNT_NOT_INITIALIZED);
}

/// The holder (the council while the asset is in the pool) signs with its multisig, the receipt
/// is required, and the sidecar, asset and collection must be the pagaré's own. The borrower waits
/// for the cancel grace; anyone else is not a holder.
#[test]
fn cancel_pagare_requires_the_holder_the_receipt_and_the_right_accounts() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let (borrower, line_id) = drawn_and_repaid(&mut env, &k, 2 * UNIT, 1_000, &[UNIT, UNIT]);
    let line_key = line_pda(&k.pool, line_id).0;
    let council = env.council.insecure_clone();
    let a = council.pubkey();

    // The borrower before the cancel grace; a stranger at any time.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &borrower.pubkey(), &a, RECEIPT_HASH);
    env.expect_err(ix, &council, &[&borrower], ERR_BASE + elote_pool::errors::PoolError::CancelGraceNotElapsed as u32);
    let stranger = env.funded(1);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &stranger.pubkey(), &a, RECEIPT_HASH);
    env.expect_err(ix, &council, &[&stranger], E_NOT_HOLDER);
    // A multisig whose threshold fell below a majority after the pool was created.
    let multisig = env.multisig;
    let mut account = env.account(&multisig);
    let saved = account.clone();
    account.data[72] = 1;
    env.svm.set_account(multisig, account).unwrap();
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    env.expect_err(ix, &council, &[], ERR_BASE + elote_pool::errors::PoolError::CouncilInvalid as u32);
    env.svm.set_account(multisig, saved).unwrap();
    // Without a receipt.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    // Another pagaré's asset, or a stranger's.
    for wrong in [pagare_asset_pda(&line_key, 1).0, solana_pubkey::Pubkey::new_unique()] {
        let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, wrong, k.collection, None);
        env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
    }
    // Another pagaré's sidecar for this index.
    let mut ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == pagare_pda(&line_key, 0).0).unwrap();
    ix.accounts[i].pubkey = pagare_pda(&line_key, 1).0;
    env.expect_err(ix, &council, &[], E_CONSTRAINT_SEEDS);
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, pagare_asset_pda(&line_key, 0).0, other, None);
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);

    // Nothing happened: the title keeps the repayment's mark and its asset.
    assert_eq!(env.pagare(&k, line_id, 0).status, elote_pool::state::PAGARE_PAID_PENDING_CANCEL);
    assert_eq!(env.asset_state(&pagare_asset_pda(&line_key, 0).0).base.owner, k.pool);
    assert_eq!(env.collection_state(&k.collection).current_size, 2);
}

// -----------------------------------------------------------------------------------------------
// the registry
// -----------------------------------------------------------------------------------------------

/// The registry's hash: `sha256(asset || kind || from || to || hash || stated_at LE || entry_count LE)`.
fn entry_hash(asset: &solana_pubkey::Pubkey, kind: u8, from: &solana_pubkey::Pubkey, to: &solana_pubkey::Pubkey, hash: &[u8; 32], stated_at: i64, count: u32) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(asset.as_ref());
    h.update([kind]);
    h.update(from.as_ref());
    h.update(to.as_ref());
    h.update(hash);
    h.update(stated_at.to_le_bytes());
    h.update(count.to_le_bytes());
    h.finalize().into()
}

fn chain(root: &[u8; 32], entry: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(root);
    h.update(entry);
    h.finalize().into()
}

/// Every draw opens the pagaré's registry with the issuance entry: the sidecar's chain has one
/// entry, kind `Emision`, from and to the pool, over the pagaré's hash, and the event carries its
/// full content so the chain can be replayed off chain.
#[test]
fn a_draw_writes_the_issuance_entry_of_the_registry() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::KIND_EMISION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let sidecar = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&sidecar.asset, KIND_EMISION, &k.pool, &k.pool, &env.expected_pagare_hash(&k, line_id, 0, UNIT, now), now, 1);
    assert_eq!((sidecar.entry_count, sidecar.last_kind, sidecar.flags), (1, KIND_EMISION, 0));
    assert_eq!(sidecar.log_root, chain(&[0; 32], &expected), "genesis: the issuance entry over an empty root");
    assert_eq!(sidecar.cancel_receipt_hash, [0; 32]);
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.kind, ev.from, ev.to),
        (k.pool, line_id, 0, sidecar.asset, KIND_EMISION, k.pool, k.pool)
    );
    assert_eq!((ev.entry_hash, ev.entry_count, ev.recorded_at, ev.stated_at), (expected, 1, now, now));
    // A second pagaré starts its own chain.
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let second = env.pagare(&k, line_id, 1);
    assert_eq!(second.entry_count, 1);
    assert_ne!(second.log_root, sidecar.log_root);
}

/// Endoso en procuración (the only endoso the program records): the pool
/// stays the owner, the entry goes into the registry, and the power to extinguish the pagaré is lent
/// through an owner-managed `BurnDelegate` whose authority is a program PDA nobody holds a key for,
/// so the burn can only happen inside `cancel_pagare`. A second procuración after a revocation
/// approves the existing plugin instead of adding it (the sidecar's flag says it exists).
#[test]
fn an_endoso_en_procuracion_appends_the_entry_and_lends_the_burn_to_a_program_address() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_EMISION, KIND_PROCURACION, PAGARE_FLAG_BURN_DELEGATE, PAGARE_FLAG_PROCURACION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let before = env.pagare(&k, line_id, 0);
    assert_eq!((before.entry_count, before.last_kind, before.flags), (1, KIND_EMISION, 0));
    let procurador = env.funded(1);
    let (pda, _) = procurador_pda(&pagare_pda(&line_pda(&k.pool, line_id).0, 0).0, &procurador.pubkey());

    let meta = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_PROCURACION, &k.pool, &procurador.pubkey(), &ENDOSO_HASH, now, 2);
    assert_eq!((after.entry_count, after.last_kind, after.flags), (2, KIND_PROCURACION, PAGARE_FLAG_BURN_DELEGATE | PAGARE_FLAG_PROCURACION));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    assert_eq!((after.status, after.asset, after.drawn_through), (before.status, before.asset, before.drawn_through), "nothing else moves");
    assert_eq!((before.procurador, after.procurador), (solana_pubkey::Pubkey::default(), procurador.pubkey()), "the sidecar names the procurador");
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count, ev.recorded_at, ev.stated_at), (KIND_PROCURACION, k.pool, procurador.pubkey(), expected, 2, now, now));
    // The asset: still the pool's, still frozen, with a BurnDelegate held by the program PDA.
    let asset = env.asset_state(&after.asset);
    assert_eq!(asset.base.owner, k.pool);
    assert!(asset.plugin_list.permanent_freeze_delegate.as_ref().unwrap().permanent_freeze_delegate.frozen);
    let burn = asset.plugin_list.burn_delegate.as_ref().expect("BurnDelegate");
    assert_eq!((burn.base.authority.authority_type, burn.base.authority.address), (AuthorityType::Address, Some(pda)));
    // The procurador's own key cannot burn, and a second procuración while one is in force is refused.
    let ix = mpl_core::instructions::BurnV1Builder::new().asset(after.asset).collection(Some(k.collection)).payer(procurador.pubkey()).authority(Some(procurador.pubkey())).instruction();
    assert!(env.try_send(ix, &procurador, &[]).is_err());
    let council = env.council.insecure_clone();
    let other = env.funded(1);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &council.pubkey(), KIND_PROCURACION, other.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &council, &[], E_PROCURACION_STATE);
}

/// The refusals of `endorse_pagare`: only a procuración by the pool, signed by the council; the
/// endorsee named and a third party (not the pool, the council, a curator or the borrower); a hash; a line not written off; a pagaré outstanding.
#[test]
fn endorse_pagare_refuses_other_kinds_bad_endorsees_and_the_wrong_signers() {
    use elote_pool::state::{KIND_ABONO, KIND_PROCURACION, KIND_RECIBO};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let procurador = env.funded(1);
    let a = council.pubkey();
    // 1 is the reserved code of the endoso en propiedad.
    for kind in [1, KIND_ABONO, KIND_RECIBO, KIND_RECIBO + 1] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, kind, procurador.pubkey(), ENDOSO_HASH);
        env.expect_err(ix, &council, &[], E_ENDOSO_KIND_INVALID);
    }
    for to in [solana_pubkey::Pubkey::default(), k.pool, a, env.curators[2].pubkey(), borrower.pubkey()] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, to, ENDOSO_HASH);
        env.expect_err(ix, &council, &[], E_ENDORSEE_INVALID);
    }
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    // The borrower is not the holder.
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &borrower.pubkey(), KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &borrower, &[], E_NOT_HOLDER);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 1, "nothing was appended");
    // A written-off line keeps its títulos out of any endoso.
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &a);
    env.send(ix, &council, &[]);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
}

/// The revocation of a procuración (LGTOC 41): the owner alone appends the revocation entry and
/// takes the burn delegate back to `Owner`; the in-force flag clears, the plugin flag stays, and a
/// later procuración approves the existing plugin instead of adding it.
#[test]
fn revoke_procuracion_appends_the_entry_and_takes_the_delegate_back() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_PROCURACION, KIND_REVOCACION, PAGARE_FLAG_BURN_DELEGATE, PAGARE_FLAG_PROCURACION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let procurador = env.funded(1);
    let a = council.pubkey();
    let revocation = [0x7e; 32];

    // Nothing to revoke yet.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    env.expect_err(ix, &council, &[], E_PROCURACION_STATE);

    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let before = env.pagare(&k, line_id, 0);
    // The owner alone; a zero hash and a stranger are refused.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &borrower.pubkey(), revocation);
    env.expect_err(ix, &borrower, &[], E_NOT_HOLDER);
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    let meta = env.send(ix, &council, &[]);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_REVOCACION, &k.pool, &procurador.pubkey(), &revocation, now, 3);
    assert_eq!((after.entry_count, after.last_kind, after.flags), (3, KIND_REVOCACION, PAGARE_FLAG_BURN_DELEGATE));
    assert_eq!(after.procurador, solana_pubkey::Pubkey::default(), "nobody collects now");
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count), (KIND_REVOCACION, k.pool, procurador.pubkey(), expected, 3));
    let asset = env.asset_state(&after.asset);
    let burn = asset.plugin_list.burn_delegate.as_ref().expect("the plugin stays");
    assert_eq!(burn.base.authority.authority_type, AuthorityType::Owner);
    // Revoking twice is refused; a new procuración to another procurador approves the existing plugin.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    env.expect_err(ix, &council, &[], E_PROCURACION_STATE);
    let other = env.funded(1);
    env.procuracion(&k, line_id, 0, &other.pubkey(), [0x5f; 32]);
    let again = env.pagare(&k, line_id, 0);
    assert_eq!((again.entry_count, again.last_kind, again.flags), (4, KIND_PROCURACION, PAGARE_FLAG_BURN_DELEGATE | PAGARE_FLAG_PROCURACION));
    assert_eq!(again.procurador, other.pubkey());
    let (pda, _) = procurador_pda(&pagare_pda(&line_pda(&k.pool, line_id).0, 0).0, &other.pubkey());
    let asset = env.asset_state(&again.asset);
    let burn = asset.plugin_list.burn_delegate.as_ref().unwrap();
    assert_eq!(burn.base.authority.address, Some(pda));
}

/// A court's act on a title (embargo, suspension, cancellation; the constancia of art. 28): the
/// council records the document's hash as an entry; nothing moves on the sidecar or
/// the asset. Allowed on any line status, written-off included, and beside a procuración in force;
/// refused only on a burnt title, which the sidecar records as `PAGARE_PAID`.
#[test]
fn court_annotations_append_the_entry_and_move_nothing() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_ANOTACION_JUDICIAL, KIND_CONSTANCIA_JUDICIAL, KIND_PROCURACION, KIND_RECIBO};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();
    let embargo = [0x6a; 32];

    // Refusals: a kind that is not a court's, a zero hash, a non-holder.
    for kind in [KIND_PROCURACION, KIND_RECIBO, KIND_RECIBO + 1] {
        let ix = env.court_annotation_ix(&k, line_id, 0, &a, kind, embargo);
        env.expect_err(ix, &council, &[], E_ANNOTATION_KIND_INVALID);
    }
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_ANOTACION_JUDICIAL, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    let ix = env.court_annotation_ix(&k, line_id, 0, &borrower.pubkey(), KIND_ANOTACION_JUDICIAL, embargo);
    env.expect_err(ix, &borrower, &[], E_NOT_HOLDER);

    // The embargo: one entry, from and to the pool (no counterparty), the rest untouched.
    let before = env.pagare(&k, line_id, 0);
    let meta = env.court_annotation(&k, line_id, 0, KIND_ANOTACION_JUDICIAL, embargo);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_ANOTACION_JUDICIAL, &k.pool, &k.pool, &embargo, now, 2);
    assert_eq!((after.entry_count, after.last_kind), (2, KIND_ANOTACION_JUDICIAL));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    assert_eq!(
        (after.status, after.flags, after.procurador, after.paid_at, after.receipt_hash, after.amount_mxn, after.due_at),
        (before.status, before.flags, before.procurador, before.paid_at, before.receipt_hash, before.amount_mxn, before.due_at),
        "nothing else on the sidecar moves"
    );
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!(
        (ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count, ev.asset, ev.stated_at),
        (KIND_ANOTACION_JUDICIAL, k.pool, k.pool, expected, 2, after.asset, now)
    );
    let asset = env.asset_state(&after.asset);
    assert_eq!(asset.base.owner, k.pool, "the asset stays with the pool");
    assert!(asset.plugin_list.permanent_freeze_delegate.as_ref().unwrap().permanent_freeze_delegate.frozen, "and stays frozen");

    // The constancia (art. 28) chains on, and both coexist with a procuración in force.
    env.court_annotation(&k, line_id, 0, KIND_CONSTANCIA_JUDICIAL, [0x6b; 32]);
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    env.court_annotation(&k, line_id, 0, KIND_ANOTACION_JUDICIAL, [0x6c; 32]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.entry_count, p0.last_kind, p0.procurador), (5, KIND_ANOTACION_JUDICIAL, procurador.pubkey()));

    // A written-off line: the title outlives the loss and a court may still act on it.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &a);
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    env.court_annotation(&k, line_id, 0, KIND_ANOTACION_JUDICIAL, embargo);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 2);

    // A burnt title is refused; its sibling on the repaid line is not.
    let (_, line_id) = drawn_and_repaid(&mut env, &k, 4 * UNIT, 1_000, &[UNIT, 3 * UNIT]);
    env.cancel_pagare(&k, line_id, 0);
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_CONSTANCIA_JUDICIAL, embargo);
    env.expect_err(ix, &council, &[], E_PAGARE_ALREADY_PAID);
    env.court_annotation(&k, line_id, 1, KIND_CONSTANCIA_JUDICIAL, embargo);
    assert_eq!(env.pagare(&k, line_id, 1).last_kind, KIND_CONSTANCIA_JUDICIAL);
}

/// The procurador's branch of `cancel_pagare` (LGTOC 35, decision 29): the procurador of record
/// signs alone, the same receipt and coverage checks apply, the `Terminacion` entry is
/// written from the procurador, the procuración ends with the title, and the burn goes through the
/// program address the delegate was approved to. The owner's branch writes the same entry from the
/// pool, and its extinction ends a procuración in force too.
#[test]
fn the_procurador_extinguishes_the_title_through_the_program_address() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_TERMINACION, PAGARE_FLAG_BURN_DELEGATE};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let procurador = env.funded(1);
    let pr = procurador.pubkey();
    let relayer = env.funded(1);
    let r = relayer.pubkey();
    // The endoso comes before the payment: a paid title has nothing left to collect.
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    env.procuracion(&k, line_id, 0, &pr, ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 4 * UNIT + interest);

    // Refusals: a stranger; the procurador on a title it does not hold; no receipt;
    // a program address derived for someone else, or none.
    let stranger = env.funded(1);
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &stranger.pubkey(), &r, RECEIPT_HASH);
    env.expect_err(ix, &relayer, &[&stranger], E_NOT_HOLDER);
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 1, &pr, &r, RECEIPT_HASH);
    env.expect_err(ix, &relayer, &[&procurador], E_NOT_HOLDER);
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &pr, &r, [0; 32]);
    env.expect_err(ix, &relayer, &[&procurador], E_DOCUMENT_HASH_REQUIRED);
    let pagare_key = pagare_pda(&line_key, 0).0;
    let pda = procurador_pda(&pagare_key, &pr).0;
    for wrong in [procurador_pda(&pagare_key, &stranger.pubkey()).0, PROGRAM_ID] {
        let mut ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &pr, &r, RECEIPT_HASH);
        let i = ix.accounts.iter().position(|m| m.pubkey == pda).unwrap();
        ix.accounts[i].pubkey = wrong;
        env.expect_err(ix, &relayer, &[&procurador], E_WRONG_ACCOUNT);
    }
    assert_eq!(env.pagare(&k, line_id, 0).status, elote_pool::state::PAGARE_PAID_PENDING_CANCEL, "marked by the payment, not yet extinguished");

    // The extinction: the entry from the procurador, the mark, the procuración ended, the asset
    // burnt through the program address, the rent to the payer.
    let before = env.pagare(&k, line_id, 0);
    let relayer_before = env.account(&r).lamports;
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &pr, &r, RECEIPT_HASH);
    let meta = env.send(ix, &relayer, &[&procurador]);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    // Emision, Procuracion, Pagado (the payment's), Terminacion.
    let expected = entry_hash(&after.asset, KIND_TERMINACION, &pr, &k.pool, &RECEIPT_HASH, now, 4);
    assert_eq!((after.status, after.paid_at, after.receipt_hash, after.cancel_receipt_hash), (PAGARE_PAID, now, RECEIPT_HASH, RECEIPT_HASH));
    assert_eq!(
        (after.entry_count, after.last_kind, after.flags, after.procurador),
        (4, KIND_TERMINACION, PAGARE_FLAG_BURN_DELEGATE, solana_pubkey::Pubkey::default()),
        "the procuración ends with the title"
    );
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count), (KIND_TERMINACION, pr, k.pool, expected, 4));
    let ev: PagareCancelled = find_event(&meta).expect("PagareCancelled");
    assert_eq!((ev.holder, ev.receipt_hash, ev.asset, ev.by_borrower), (pr, RECEIPT_HASH, after.asset, false));
    let tomb = env.account(&after.asset);
    assert_eq!((tomb.data.len(), tomb.owner), (1, MPL_CORE_ID), "burnt through the program address");
    assert!(env.account(&r).lamports > relayer_before, "the rent goes to the payer");
    assert_eq!(env.collection_state(&k.collection).current_size, 1);
    // Paid is terminal: the ex-procurador is nobody now (the procuración ended with the title),
    // and the owner finds the mark.
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &pr, &r, RECEIPT_HASH);
    env.expect_err(ix, &relayer, &[&procurador], E_NOT_HOLDER);
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_ALREADY_PAID);

    // The owner's branch writes the same entry, from the pool.
    let before = env.pagare(&k, line_id, 1);
    let meta = env.cancel_pagare(&k, line_id, 1);
    let now = env.now();
    let after = env.pagare(&k, line_id, 1);
    let expected = entry_hash(&after.asset, KIND_TERMINACION, &k.pool, &k.pool, &RECEIPT_HASH, now, 3);
    assert_eq!((after.entry_count, after.last_kind, after.cancel_receipt_hash), (3, KIND_TERMINACION, RECEIPT_HASH));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to), (KIND_TERMINACION, k.pool, k.pool));

    // The PAGADO mark binds the procurador too (the unpaid title is refused), and the owner's
    // extinction ends a procuración in force.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.procuracion(&k, line_id, 0, &pr, ENDOSO_HASH);
    env.procuracion(&k, line_id, 1, &pr, ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, UNIT + interest);
    assert_eq!(env.line(&k, line_id).principal_repaid, UNIT);
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 1, &pr, &r, RECEIPT_HASH);
    env.expect_err(ix, &relayer, &[&procurador], E_PAGARE_NOT_COVERED);
    env.cancel_pagare(&k, line_id, 0);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(
        (p0.status, p0.flags, p0.procurador, p0.last_kind),
        (PAGARE_PAID, PAGARE_FLAG_BURN_DELEGATE, solana_pubkey::Pubkey::default(), KIND_TERMINACION)
    );
    assert_eq!(env.pagare(&k, line_id, 1).procurador, pr, "the sibling's procuración stands");
}

/// The borrower's exit's parameter: `cancel_grace_seconds` is set at `init_pool` with a 30-day
/// floor and a one-year ceiling (`MAX_CANCEL_GRACE_SECONDS`), and changed only by the council
/// (with its multisig), within the same bounds, with an event.
#[test]
fn the_cancel_grace_is_a_pool_parameter_between_thirty_days_and_a_year() {
    use elote_pool::events::CancelGraceSet;
    use elote_pool::state::MAX_CANCEL_GRACE_SECONDS;
    assert_eq!(MAX_CANCEL_GRACE_SECONDS, 365 * 86_400);
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let asset_mint = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_for(&asset_mint);
    for (seconds, code) in [(CANCEL_GRACE - 1, E_CANCEL_GRACE_TOO_SHORT), (MAX_CANCEL_GRACE_SECONDS + 1, E_CANCEL_GRACE_TOO_LONG)] {
        let mut params = env.init_params(0, BPS, COLLECTION_PERIOD);
        params.cancel_grace_seconds = seconds;
        let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), params);
        env.expect_err(ix, &council, &[], code);
    }
    let (k, meta) = env.setup_pool();
    let ev: elote_pool::events::PoolInitialized = find_event(&meta).expect("PoolInitialized");
    assert_eq!((ev.pool, ev.cancel_grace_seconds, env.pool(&k).cancel_grace_seconds), (k.pool, CANCEL_GRACE, CANCEL_GRACE));
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), CANCEL_GRACE - 1);
    env.expect_err(ix, &council, &[], E_CANCEL_GRACE_TOO_SHORT);
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), MAX_CANCEL_GRACE_SECONDS + 1);
    env.expect_err(ix, &council, &[], E_CANCEL_GRACE_TOO_LONG);
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), u32::MAX);
    env.expect_err(ix, &council, &[], E_CANCEL_GRACE_TOO_LONG);
    let stranger = env.funded(1);
    let ix = env.set_cancel_grace_ix(&k, &stranger.pubkey(), 2 * CANCEL_GRACE);
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), 2 * CANCEL_GRACE);
    let ev: CancelGraceSet = find_event(&env.send(ix, &council, &[])).expect("CancelGraceSet");
    assert_eq!((ev.pool, ev.cancel_grace_seconds), (k.pool, 2 * CANCEL_GRACE));
    assert_eq!(env.pool(&k).cancel_grace_seconds, 2 * CANCEL_GRACE);
    // The ceiling itself is accepted.
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), MAX_CANCEL_GRACE_SECONDS);
    env.send(ix, &council, &[]);
    assert_eq!(env.pool(&k).cancel_grace_seconds, MAX_CANCEL_GRACE_SECONDS);
}

/// The borrower's date is fixed when the pagaré is paid: `repay` copies the pool's
/// `cancel_grace_seconds` onto each pagaré it marks `PAGARE_PAID_PENDING_CANCEL`, and the
/// borrower's `cancel_pagare` waits `paid_at + Pagare.cancel_grace_seconds`. A later
/// `set_cancel_grace`, up or down, applies only to pagarés paid after it.
#[test]
fn the_cancel_grace_is_fixed_on_the_pagare_at_payment() {
    use elote_pool::state::{MAX_CANCEL_GRACE_SECONDS, PAGARE_PAID_PENDING_CANCEL};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let grace = i64::from(CANCEL_GRACE);
    let max = i64::from(MAX_CANCEL_GRACE_SECONDS);

    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!(env.pagare(&k, line_id, 0).cancel_grace_seconds, 0, "nothing is fixed while outstanding");
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &b, interest + 2 * UNIT);
    env.repay(&k, line_id, &borrower, UNIT + interest);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.cancel_grace_seconds), (PAGARE_PAID_PENDING_CANCEL, CANCEL_GRACE));
    assert_eq!(env.pagare(&k, line_id, 1).cancel_grace_seconds, 0, "the pagaré the payment did not cover");

    // The council raises the grace to the ceiling after the payment: pagaré 0 keeps its date.
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), MAX_CANCEL_GRACE_SECONDS);
    env.send(ix, &council, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).cancel_grace_seconds, CANCEL_GRACE);
    env.warp(p0.paid_at + grace);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    env.expect_err(ix, &borrower, &[], E_CANCEL_GRACE_NOT_ELAPSED);
    env.warp(p0.paid_at + grace + 1);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);

    // Pagaré 1, paid after the change, carries the ceiling; lowering the pool's grace afterwards
    // does not shorten it.
    env.repay(&k, line_id, &borrower, 2 * UNIT);
    let p1 = env.pagare(&k, line_id, 1);
    assert_eq!((p1.status, p1.cancel_grace_seconds), (PAGARE_PAID_PENDING_CANCEL, MAX_CANCEL_GRACE_SECONDS));
    let ix = env.set_cancel_grace_ix(&k, &council.pubkey(), CANCEL_GRACE);
    env.send(ix, &council, &[]);
    for at in [p1.paid_at + grace + 1, p1.paid_at + max] {
        env.warp(at);
        let ix = env.cancel_pagare_ix(&k, line_id, 1, &b, &b, RECEIPT_HASH);
        env.expect_err(ix, &borrower, &[], E_CANCEL_GRACE_NOT_ELAPSED);
    }
    env.warp(p1.paid_at + max + 1);
    let ix = env.cancel_pagare_ix(&k, line_id, 1, &b, &b, RECEIPT_HASH);
    env.send(ix, &borrower, &[]);
    let p1 = env.pagare(&k, line_id, 1);
    assert_eq!((p1.status, p1.cancel_grace_seconds), (PAGARE_PAID, MAX_CANCEL_GRACE_SECONDS), "the grace stays as history");
}

/// The debtor's exit that does not depend on the council (research's legal review, point 4): once a
/// payment wrote the PAGADO mark, the borrower alone cancels the pagaré after
/// `paid_at + cancel_grace_seconds`, with a receipt; the entry is the borrower's, the event says
/// `by_borrower`, and the pool PDA burns the asset as its owner, also under a procuración in force.
#[test]
fn the_borrower_cancels_alone_after_the_cancel_grace() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_TERMINACION, PAGARE_FLAG_BURN_DELEGATE};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let grace = i64::from(CANCEL_GRACE);

    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &b, interest);
    env.repay(&k, line_id, &borrower, UNIT + interest);
    let paid_at = env.pagare(&k, line_id, 0).paid_at;

    // Before the grace, and at its last second: refused.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    env.expect_err(ix, &borrower, &[], E_CANCEL_GRACE_NOT_ELAPSED);
    env.warp(paid_at + grace);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    env.expect_err(ix, &borrower, &[], E_CANCEL_GRACE_NOT_ELAPSED);
    env.warp(paid_at + grace + 1);
    // No receipt; a pagaré no payment covered.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, [0; 32]);
    env.expect_err(ix, &borrower, &[], E_DOCUMENT_HASH_REQUIRED);
    let ix = env.cancel_pagare_ix(&k, line_id, 1, &b, &b, RECEIPT_HASH);
    env.expect_err(ix, &borrower, &[], E_PAGARE_NOT_COVERED);

    let before = env.pagare(&k, line_id, 0);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.status, after.paid_at, after.cancel_receipt_hash, after.last_kind), (PAGARE_PAID, paid_at, RECEIPT_HASH, KIND_TERMINACION));
    let expected = entry_hash(&after.asset, KIND_TERMINACION, &b, &k.pool, &RECEIPT_HASH, now, before.entry_count + 1);
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to), (KIND_TERMINACION, b, k.pool));
    let ev: PagareCancelled = find_event(&meta).expect("PagareCancelled");
    assert_eq!((ev.holder, ev.receipt_hash, ev.by_borrower), (b, RECEIPT_HASH, true));
    assert_eq!(env.account(&after.asset).data.len(), 1, "burnt");

    // Under a procuración in force: the borrower still cancels and the pool PDA burns as owner.
    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &b, interest);
    env.repay(&k, line_id, &borrower, 2 * UNIT + interest);
    env.warp(env.pagare(&k, line_id, 0).paid_at + grace + 1);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &b, &b, RECEIPT_HASH);
    env.send(ix, &borrower, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(
        (p0.status, p0.flags, p0.procurador),
        (PAGARE_PAID, PAGARE_FLAG_BURN_DELEGATE, solana_pubkey::Pubkey::default()),
        "the procuración ends with the title"
    );
    assert_eq!(env.account(&p0.asset).data.len(), 1, "burnt");
}

/// The payment entries (art. 130, "en un solo acto"): every `repay` appends one entry to each
/// sidecar it touches, oldest first: `Pagado` where the payment brings `principal_repaid` to the
/// pagaré's `drawn_through` (the PAGADO mark, `paid_at` and `receipt_hash` at the moment of
/// payment, only the burn waiting), `Abono` on the first one it leaves uncovered; the cursor
/// `pagares_paid` moves past the paid ones. A zero receipt hash is allowed and means "to follow".
#[test]
fn a_payment_writes_its_entries_on_the_sidecars_it_touches() {
    use elote_pool::events::{PagareEntry, PagarePayment};
    use elote_pool::state::{KIND_ABONO, KIND_PAGADO, PAGARE_PAID_PENDING_CANCEL};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, 2 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.draws, line.pagares_paid), (3, 0));
    let interest = line.interest_due;
    assert!(interest > 1);
    env.mint_to(&k.asset_mint, &council, &b, interest);

    // An interest-only payment: recorded in `PagarePayment`, no entry on any pagaré, nothing marked.
    let half = interest / 2;
    let before = env.pagare(&k, line_id, 0);
    let ix = env.repay_ix_full(&k, line_id, &b, half, [0; 32], &env.payment_sidecars(&k, line_id, half));
    let meta = env.send(ix, &borrower, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.entry_count, p0.log_root, p0.status), (1, before.log_root, PAGARE_OUTSTANDING));
    assert!(find_event::<PagareEntry>(&meta).is_none());
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.payer, ev.receipt_hash, ev.late_part, ev.interest_part, ev.principal_part), (b, [0; 32], 0, half, 0));
    assert!(ev.entries.is_empty());
    assert_eq!((env.pagare(&k, line_id, 1).entry_count, env.line(&k, line_id).pagares_paid), (1, 0));

    // The rest of the interest and one and a half pagarés of principal: 0 is paid, 1 gets an
    // Abono, 2 is untouched, the cursor moves past 0.
    let before1 = env.pagare(&k, line_id, 1);
    let amount = (interest - half) + UNIT + UNIT / 2;
    let ix = env.repay_ix_full(&k, line_id, &b, amount, RECEIPT_HASH, &env.payment_sidecars(&k, line_id, amount));
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let p0 = env.pagare(&k, line_id, 0);
    let p1 = env.pagare(&k, line_id, 1);
    assert_eq!(
        (p0.status, p0.paid_at, p0.receipt_hash, p0.last_kind, p0.entry_count),
        (PAGARE_PAID_PENDING_CANCEL, now, RECEIPT_HASH, KIND_PAGADO, 2),
        "the PAGADO mark at the moment of payment"
    );
    assert_eq!((p1.status, p1.last_kind, p1.entry_count), (PAGARE_OUTSTANDING, KIND_ABONO, 2));
    assert_eq!(p1.log_root, chain(&before1.log_root, &entry_hash(&p1.asset, KIND_ABONO, &b, &k.pool, &RECEIPT_HASH, now, 2)));
    assert_eq!(env.pagare(&k, line_id, 2).entry_count, 1, "untouched");
    assert_eq!(env.line(&k, line_id).pagares_paid, 1);
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.interest_part, ev.principal_part, ev.receipt_hash), (interest - half, UNIT + UNIT / 2, RECEIPT_HASH));
    let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.entry_count, e.principal_applied)).collect();
    assert_eq!(e, vec![(0, KIND_PAGADO, 2, UNIT), (1, KIND_ABONO, 2, UNIT / 2)]);

    // The rest: 1 and 2 are paid, the cursor passes the last draw and the line closes.
    env.repay(&k, line_id, &borrower, UNIT / 2 + 2 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.pagares_paid, line.status), (3, LineStatus::Repaid));
    let (p1, p2) = (env.pagare(&k, line_id, 1), env.pagare(&k, line_id, 2));
    assert_eq!((p1.status, p1.last_kind, p1.entry_count), (PAGARE_PAID_PENDING_CANCEL, KIND_PAGADO, 3));
    assert_eq!((p2.status, p2.last_kind, p2.entry_count, p2.paid_at), (PAGARE_PAID_PENDING_CANCEL, KIND_PAGADO, 2, env.now()));
}

/// `repay` takes exactly the sidecars `[pagares_paid ..= last_touched]`, consecutive and verified
/// against their seeds; anything else is `PagareAccountsMismatch`, and a line with nothing left
/// to mark passes none.
#[test]
fn repay_requires_exactly_the_sidecars_the_payment_touches() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let council = env.council.insecure_clone();
    let p = env.line_params(3 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    for _ in 0..3 {
        env.draw_amount(&k, line_id, &borrower, UNIT);
    }
    let line_key = line_pda(&k.pool, line_id).0;
    let s: Vec<_> = (0..3).map(|i| pagare_pda(&line_key, i).0).collect();
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &b, interest);
    let other = env.borrower(&k);
    let other_id = open(&mut env, &k, &other, &p);
    env.draw_amount(&k, other_id, &other, UNIT);
    let foreign = pagare_pda(&line_pda(&k.pool, other_id).0, 0).0;

    // Interest only touches pagaré 0: none, two, the wrong one, another line's, twice, a stranger's account.
    for wrong in [vec![], vec![s[0], s[1]], vec![s[1]], vec![foreign], vec![s[0], s[0]], vec![b], vec![s[0], foreign]] {
        let ix = env.repay_ix_full(&k, line_id, &b, interest, RECEIPT_HASH, &wrong);
        env.expect_err(ix, &borrower, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    }
    assert_eq!(env.payment_sidecars(&k, line_id, interest), vec![s[0]]);
    let ix = env.repay_ix_full(&k, line_id, &b, interest, RECEIPT_HASH, &[s[0]]);
    env.send(ix, &borrower, &[]);
    // One pagaré of principal and one unit more: 0 is paid and 1 touched.
    for wrong in [vec![s[0]], vec![s[0], s[1], s[2]], vec![s[1]], vec![s[1], s[0]]] {
        let ix = env.repay_ix_full(&k, line_id, &b, UNIT + 1, RECEIPT_HASH, &wrong);
        env.expect_err(ix, &borrower, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    }
    assert_eq!(env.payment_sidecars(&k, line_id, UNIT + 1), vec![s[0], s[1]]);
    env.repay(&k, line_id, &borrower, UNIT + 1);
    assert_eq!(env.line(&k, line_id).pagares_paid, 1);
    // From the cursor: 1 and 2; 0 is behind it.
    for wrong in [vec![s[0], s[1], s[2]], vec![s[1]], vec![s[2]]] {
        let ix = env.repay_ix_full(&k, line_id, &b, 2 * UNIT - 1, RECEIPT_HASH, &wrong);
        env.expect_err(ix, &borrower, &[], E_PAGARE_ACCOUNTS_MISMATCH);
    }
    env.repay(&k, line_id, &borrower, 2 * UNIT - 1);
    assert_eq!((env.line(&k, line_id).pagares_paid, env.line(&k, line_id).status), (3, LineStatus::Repaid));

    // A line that never drew is not repayable at all (it is not Active): the line check comes
    // first, and the harness computes no sidecar for it. The empty set is otherwise required only
    // once every pagaré is marked, which leaves nothing to pay.
    let idle = env.borrower(&k);
    let idle_id = open(&mut env, &k, &idle, &p);
    assert_eq!(env.payment_sidecars(&k, idle_id, 1), Vec::<solana_pubkey::Pubkey>::new());
    let ix = env.repay_ix_full(&k, idle_id, &idle.pubkey(), 1, [0; 32], &[foreign]);
    env.expect_err(ix, &idle, &[], E_LINE_STATUS);
}

/// After the payment entries, `cancel_pagare` requires the PAGADO mark
/// (`PAGARE_PAID_PENDING_CANCEL`) in place of the coverage rule; the payment's receipt and time
/// stay on the sidecar and the cancellation's receipt is kept beside them.
#[test]
fn cancel_pagare_requires_the_payments_pagado_mark() {
    use elote_pool::state::{KIND_TERMINACION, PAGARE_PAID_PENDING_CANCEL};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let a = council.pubkey();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    let payment = [0x11; 32];
    let amount = UNIT + interest;
    let ix = env.repay_ix_full(&k, line_id, &borrower.pubkey(), amount, payment, &env.payment_sidecars(&k, line_id, amount));
    env.send(ix, &borrower, &[]);
    let paid_at = env.now();
    env.warp(paid_at + 3_600);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.paid_at, p0.receipt_hash), (PAGARE_PAID_PENDING_CANCEL, paid_at, payment));
    // The uncovered one has no mark: no cancellation.
    env.expect_cancel_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    // The marked one cancels; the payment's receipt and time stay, the cancellation's receipt beside.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(
        (p0.status, p0.paid_at, p0.receipt_hash, p0.cancel_receipt_hash, p0.last_kind, p0.entry_count),
        (PAGARE_PAID, paid_at, payment, RECEIPT_HASH, KIND_TERMINACION, 3)
    );
}

/// `annotate_payment`: the council (the holder) attaches the real receipt to a payment entry
/// whose hash was zero or wrong, as a `Recibo` entry referencing it; the repair path for
/// third-party payments.
#[test]
fn annotate_payment_attaches_the_receipt_to_a_payment_entry() {
    use elote_pool::events::{PagareEntry, PagareReceiptAnnotated};
    use elote_pool::state::{KIND_ABONO, KIND_RECIBO};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let amount = env.line(&k, line_id).interest_due + 1;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), amount);
    // A payment with no receipt hash: the Abono says "receipt to follow".
    let ix = env.repay_ix_full(&k, line_id, &borrower.pubkey(), amount, [0; 32], &env.payment_sidecars(&k, line_id, amount));
    env.send(ix, &borrower, &[]);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.last_kind, p0.entry_count), (KIND_ABONO, 2));
    // A stranger, a zero hash and a reference to no entry are refused.
    let stranger = env.funded(1);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &stranger.pubkey(), 2, RECEIPT_HASH);
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), 2, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    for reference in [0u32, 3] {
        let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), reference, RECEIPT_HASH);
        env.expect_err(ix, &council, &[], E_ENTRY_NOT_FOUND);
    }
    // The council attaches the receipt to entry 2.
    let before = env.pagare(&k, line_id, 0);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), 2, RECEIPT_HASH);
    let meta = env.send(ix, &council, &[]);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_RECIBO, &council.pubkey(), &k.pool, &RECEIPT_HASH, now, 3);
    assert_eq!((after.entry_count, after.last_kind, after.status), (3, KIND_RECIBO, PAGARE_OUTSTANDING));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count), (KIND_RECIBO, council.pubkey(), k.pool, expected, 3));
    let ev: PagareReceiptAnnotated = find_event(&meta).expect("PagareReceiptAnnotated");
    assert_eq!((ev.draw_index, ev.entry_count, ev.payment_entry_count, ev.receipt_hash), (0, 3, 2, RECEIPT_HASH));
    // A second annotation appends again.
    let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), 2, [0x22; 32]);
    env.send(ix, &council, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 4);
}
