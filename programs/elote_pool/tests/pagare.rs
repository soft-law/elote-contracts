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
use solana_keypair::Keypair;
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

    let hash0 = [0x11; 32];
    let before = env.account(&relayer.pubkey()).lamports;
    let ix = env.draw_ix_paid_by(&k, line_id, &borrower.pubkey(), &relayer.pubkey(), 4 * UNIT, hash0);
    let attestor = env.attestor.insecure_clone();
    let meta = env.send(ix, &relayer, &[&borrower, &attestor]);
    let t0 = env.now();
    let ev: PagareIssued = find_event(&meta).expect("PagareIssued");
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.pagare_hash, ev.amount, ev.due_at, ev.holder),
        (k.pool, line_id, 0, asset0, hash0, 4 * UNIT, last_due, k.pool)
    );
    assert!(env.account(&relayer.pubkey()).lamports < before - 5_000_000, "the relayer paid the rent of two accounts");

    let sidecar = env.pagare(&k, line_id, 0);
    assert_eq!((sidecar.line, sidecar.draw_index, sidecar.asset), (line_key, 0, asset0));
    assert_eq!((sidecar.pagare_hash, sidecar.amount, sidecar.issued_at, sidecar.due_at), (hash0, 4 * UNIT, t0, last_due));
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
    let hash1 = [0x22; 32];
    env.warp(t0 + 1_000);
    let ix = env.draw_ix_paid_by(&k, line_id, &borrower.pubkey(), &relayer.pubkey(), UNIT, hash1);
    let meta = env.send(ix, &relayer, &[&borrower, &attestor]);
    let ev: PagareIssued = find_event(&meta).expect("PagareIssued");
    let asset1 = pagare_asset_pda(&line_key, 1).0;
    assert_eq!((ev.draw_index, ev.asset, ev.pagare_hash, ev.amount), (1, asset1, hash1, UNIT));
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
    let hash = [0xab; 32];
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 3 * UNIT, hash);
    env.send_with_attestor(ix, &borrower);
    let issued_at = env.now();
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
    // PDA), which a later version relinquishes with one approve.
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
    // Attributes, nobody's, with the nine facts and nothing personal.
    let attributes = asset.plugin_list.attributes.as_ref().expect("Attributes");
    assert_eq!(attributes.base.authority.authority_type, AuthorityType::None);
    let list: Vec<(&str, &str)> =
        attributes.attributes.attribute_list.iter().map(|a| (a.key.as_str(), a.value.as_str())).collect();
    let amount = (3 * UNIT).to_string();
    let currency = hex(k.asset_mint.as_ref());
    let issued = issued_at.to_string();
    let due = p.term_end.to_string();
    let line_s = line_id.to_string();
    assert_eq!(
        list,
        vec![
            ("pagare_hash", hex(&hash).as_str()),
            ("amount", amount.as_str()),
            ("currency", currency.as_str()),
            ("issued_at", issued.as_str()),
            ("due_at", due.as_str()),
            ("line", line_s.as_str()),
            ("draw_index", "0"),
            ("cert_fingerprint", hex(&CERT_FINGERPRINT).as_str()),
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
        assert_eq!((sidecar.amount, sidecar.drawn_through), (amount, total));
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
    let attestor = env.attestor.insecure_clone();
    let b = borrower.pubkey();
    for (fingerprint, method) in [([0; 32], 1), ([0; 32], 2), (CERT_FINGERPRINT, 3), ([0; 32], 3)] {
        let ix = env.draw_ix_signed(&k, line_id, &b, UNIT, DOCUMENT_HASH, fingerprint, method);
        env.expect_err(ix, &borrower, &[&attestor], E_SIGNATURE_METHOD_INVALID);
    }
    for (i, (fingerprint, method)) in [([0; 32], 0), (CERT_FINGERPRINT, 0), (CERT_FINGERPRINT, 2)].into_iter().enumerate() {
        let ix = env.draw_ix_signed(&k, line_id, &b, UNIT, DOCUMENT_HASH, fingerprint, method);
        env.send_with_attestor(ix, &borrower);
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

        let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
        env.send_with_attestor(ix, &borrower);
        // The payer (here the borrower) received both strays and paid the fee and both rents out
        // of them: about one SOL richer; the sidecar holds exactly its rent.
        assert!(env.account(&borrower.pubkey()).lamports > before + 990_000_000);
        assert_eq!(env.account(&sidecar).lamports, sidecar_rent);
        let asset_state = env.asset_state(&asset);
        assert_eq!((asset_state.base.key, asset_state.base.owner), (Key::AssetV1, k.pool));
        assert_eq!(env.pagare(&k, line_id, i as u16).pagare_hash, DOCUMENT_HASH);
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
    let attestor = env.attestor.insecure_clone();

    // The asset address of the next-but-one draw, or an arbitrary one.
    for wrong in [pagare_asset_pda(&line_key, 1).0, solana_pubkey::Pubkey::new_unique()] {
        let mut ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
        let i = ix.accounts.iter().position(|m| m.pubkey == pagare_asset_pda(&line_key, 0).0).unwrap();
        ix.accounts[i].pubkey = wrong;
        env.expect_err(ix, &borrower, &[&attestor], E_CONSTRAINT_SEEDS);
    }
    // A wrong bump for either account: the client derives both and the seeds constraints check
    // each with one `create_program_address`, so no bump search runs on chain.
    let (asset_bump, pagare_bump) = (pagare_asset_pda(&line_key, 0).1, pagare_pda(&line_key, 0).1);
    let b = borrower.pubkey();
    let ix = env.draw_ix_full(&k, line_id, &b, &b, UNIT, DOCUMENT_HASH, asset_bump.wrapping_sub(1), pagare_bump, CERT_FINGERPRINT, SIGNATURE_METHOD);
    env.expect_err(ix, &borrower, &[&attestor], E_CONSTRAINT_SEEDS);
    let ix = env.draw_ix_full(&k, line_id, &b, &b, UNIT, DOCUMENT_HASH, asset_bump, pagare_bump.wrapping_sub(1), CERT_FINGERPRINT, SIGNATURE_METHOD);
    env.expect_err(ix, &borrower, &[&attestor], E_CONSTRAINT_SEEDS);
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let mut ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == k.collection).unwrap();
    ix.accounts[i].pubkey = other;
    env.expect_err(ix, &borrower, &[&attestor], E_WRONG_ACCOUNT);
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
    let admin = env.admin.insecure_clone();
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), line.interest_due);
    env.repay(k, line_id, &borrower, line.drawn + line.interest_due);
    assert_eq!(env.line(k, line_id).status, LineStatus::Repaid);
    (borrower, line_id)
}

/// Extinction is an act of the holder with a receipt (C22): the sidecar becomes the lasting
/// "PAGADO" mark (status, `paid_at`, the receipt's hash; nothing else changes and it is not
/// closed), the event names the asset and the receipt, and the asset is burnt last through the
/// permanent burn delegate, its rent going back to the payer. A second cancellation is refused.
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

    let admin = env.admin.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let relayer = env.funded(1);
    let relayer_before = env.account(&relayer.pubkey()).lamports;
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &admin.pubkey(), &relayer.pubkey(), RECEIPT_HASH);
    let meta = env.send(ix, &relayer, &[&admin, &attestor]);
    let now = env.now();
    let ev: PagareCancelled = find_event(&meta).expect("PagareCancelled");
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.receipt_hash, ev.holder),
        (k.pool, line_id, 0, asset0, RECEIPT_HASH, admin.pubkey())
    );

    // The sidecar: the mark written, everything else as issued, the account kept with its rent.
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.status, after.paid_at, after.receipt_hash), (PAGARE_PAID, now, RECEIPT_HASH));
    assert_eq!(
        (after.line, after.draw_index, after.asset, after.pagare_hash, after.amount, after.issued_at, after.due_at),
        (before.line, before.draw_index, before.asset, before.pagare_hash, before.amount, before.issued_at, before.due_at)
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
    // The other pagaré is untouched and cancels in turn.
    assert_eq!(env.pagare(&k, line_id, 1).status, PAGARE_OUTSTANDING);
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
    let admin = env.admin.insecure_clone();

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
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), UNIT);
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
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), UNIT);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (UNIT, 0));
    env.expect_cancel_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.cancel_pagare(&k, line_id, 0);
}

/// A pagaré is cancelled only while its line is `Active` or `Repaid`: in collection or written
/// off the títulos stay in force for the collection (and a line in collection that pays in full
/// is repaid again, so its pagarés cancel then); a cancelled line never drew, so it has no
/// sidecar to cancel.
#[test]
fn cancel_pagare_is_refused_in_collection_after_write_off_and_without_a_sidecar() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let admin = env.admin.insecure_clone();

    // In collection: refused even for a covered pagaré; repaid in full afterwards: allowed.
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let t0 = env.now();
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, 5 * UNIT);
    env.repay(&k, line_id, &borrower, UNIT);
    env.warp(t0 + 3 * p.interval() + 1);
    let keeper = env.funded(1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    env.send(ix, &keeper, &[]);
    env.expect_cancel_err(&k, line_id, 0, E_LINE_STATUS);
    let line = env.line(&k, line_id);
    let owed = line.drawn + line.interest_due - line.principal_repaid - line.interest_repaid + env.arrears(&k, line_id).late_due;
    env.mint_to(&k.asset_mint, &admin, &borrower.pubkey(), owed);
    env.repay(&k, line_id, &borrower, owed);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.cancel_pagare(&k, line_id, 0);
    env.cancel_pagare(&k, line_id, 1);

    // Written off: refused, the pagaré outlives the loss.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.repay(&k, line_id, &borrower, UNIT);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    env.expect_cancel_err(&k, line_id, 0, E_LINE_STATUS);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_OUTSTANDING);

    // Cancelled: no sidecar was ever created.
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let ix = env.cancel_line_ix(&k, line_id, &admin.pubkey());
    env.send(ix, &admin, &[]);
    let line_key = line_pda(&k.pool, line_id).0;
    let attestor = env.attestor.insecure_clone();
    let a = admin.pubkey();
    let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, pagare_asset_pda(&line_key, 0).0, k.collection);
    env.expect_err(ix, &admin, &[&attestor], E_ACCOUNT_NOT_INITIALIZED);
}

/// The holder (the admin while the asset is in the pool) and the attestor sign, the receipt is
/// required, and the sidecar, asset and collection must be the pagaré's own.
#[test]
fn cancel_pagare_requires_the_holder_the_attestor_the_receipt_and_the_right_accounts() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let (borrower, line_id) = drawn_and_repaid(&mut env, &k, 2 * UNIT, 1_000, &[UNIT, UNIT]);
    let line_key = line_pda(&k.pool, line_id).0;
    let admin = env.admin.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let a = admin.pubkey();

    // The borrower is not the holder.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &borrower.pubkey(), &a, RECEIPT_HASH);
    env.expect_err(ix, &admin, &[&borrower, &attestor], E_NOT_HOLDER);
    // Without the attestor.
    let stranger = env.funded(1);
    let mut ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == attestor.pubkey()).unwrap();
    ix.accounts[i].pubkey = stranger.pubkey();
    env.expect_err(ix, &admin, &[&stranger], E_ATTESTOR_REQUIRED);
    // Without a receipt.
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, [0; 32]);
    env.expect_err(ix, &admin, &[&attestor], E_DOCUMENT_HASH_REQUIRED);
    // Another pagaré's asset, or a stranger's.
    for wrong in [pagare_asset_pda(&line_key, 1).0, solana_pubkey::Pubkey::new_unique()] {
        let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, wrong, k.collection);
        env.expect_err(ix, &admin, &[&attestor], E_WRONG_ACCOUNT);
    }
    // Another pagaré's sidecar for this index.
    let mut ix = env.cancel_pagare_ix(&k, line_id, 0, &a, &a, RECEIPT_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == pagare_pda(&line_key, 0).0).unwrap();
    ix.accounts[i].pubkey = pagare_pda(&line_key, 1).0;
    env.expect_err(ix, &admin, &[&attestor], E_CONSTRAINT_SEEDS);
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let ix = env.cancel_pagare_ix_full(&k, line_id, 0, &a, &a, RECEIPT_HASH, pagare_asset_pda(&line_key, 0).0, other);
    env.expect_err(ix, &admin, &[&attestor], E_WRONG_ACCOUNT);

    // Nothing happened.
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_OUTSTANDING);
    assert_eq!(env.asset_state(&pagare_asset_pda(&line_key, 0).0).base.owner, k.pool);
    assert_eq!(env.collection_state(&k.collection).current_size, 2);
}

// -----------------------------------------------------------------------------------------------
// the registry (6d part 1)
// -----------------------------------------------------------------------------------------------

/// The registry's hash: `sha256(asset || kind || from || to || hash || stated_at LE || entry_count LE)`.
fn entry_hash(asset: &solana_pubkey::Pubkey, kind: u8, from: &solana_pubkey::Pubkey, to: &solana_pubkey::Pubkey, hash: &[u8; 32], stated_at: i64, count: u16) -> [u8; 32] {
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
    use elote_pool::state::{Pagare, KIND_EMISION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    assert_eq!(8 + Pagare::INIT_SPACE, 353, "245 before the registry");

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
    let meta = env.send_with_attestor(ix, &borrower);
    let now = env.now();
    let sidecar = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&sidecar.asset, KIND_EMISION, &k.pool, &k.pool, &DOCUMENT_HASH, now, 1);
    assert_eq!((sidecar.entry_count, sidecar.last_kind, sidecar.flags), (1, KIND_EMISION, 0));
    assert_eq!(sidecar.log_root, chain(&[0; 32], &expected), "genesis: the issuance entry over an empty root");
    assert_eq!((sidecar.cancel_receipt_hash, sidecar.requested_at, sidecar.requested_receipt_hash), ([0; 32], 0, [0; 32]));
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

/// Endoso en procuración (LGTOC 35; decision 29: the only endoso the vehicle makes in v1): the pool
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
    let admin = env.admin.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let other = env.funded(1);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &admin.pubkey(), KIND_PROCURACION, other.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &admin, &[&attestor], E_PROCURACION_STATE);
}

/// The refusals of `endorse_pagare` in v1: only a procuración by the pool, with the attestor; the
/// endorsee named and not the pool; a hash; a line not written off; a pagaré outstanding.
#[test]
fn endorse_pagare_refuses_other_kinds_bad_endorsees_and_the_wrong_signers() {
    use elote_pool::state::{KIND_ABONO, KIND_PROCURACION, KIND_PROPIEDAD, KIND_RECIBO};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let admin = env.admin.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let procurador = env.funded(1);
    let a = admin.pubkey();
    for kind in [KIND_PROPIEDAD, KIND_ABONO, KIND_RECIBO, KIND_RECIBO + 1] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, kind, procurador.pubkey(), ENDOSO_HASH);
        env.expect_err(ix, &admin, &[&attestor], E_ENDOSO_KIND_INVALID);
    }
    for to in [solana_pubkey::Pubkey::default(), k.pool] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, to, ENDOSO_HASH);
        env.expect_err(ix, &admin, &[&attestor], E_ENDORSEE_INVALID);
    }
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), [0; 32]);
    env.expect_err(ix, &admin, &[&attestor], E_DOCUMENT_HASH_REQUIRED);
    // The borrower is not the holder; without the attestor nothing happens.
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &borrower.pubkey(), KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &borrower, &[&attestor], E_NOT_HOLDER);
    let stranger = env.funded(1);
    let mut ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == attestor.pubkey()).unwrap();
    ix.accounts[i].pubkey = stranger.pubkey();
    env.expect_err(ix, &admin, &[&stranger], E_ATTESTOR_REQUIRED);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 1, "nothing was appended");
    // A written-off line keeps its títulos out of any endoso.
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &a);
    env.send(ix, &admin, &[]);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    env.expect_err(ix, &admin, &[&attestor], E_LINE_STATUS);
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
    let admin = env.admin.insecure_clone();
    let procurador = env.funded(1);
    let a = admin.pubkey();
    let revocation = [0x7e; 32];

    // Nothing to revoke yet.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    env.expect_err(ix, &admin, &[], E_PROCURACION_STATE);

    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let before = env.pagare(&k, line_id, 0);
    // The owner alone; a zero hash and a stranger are refused.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, [0; 32]);
    env.expect_err(ix, &admin, &[], E_DOCUMENT_HASH_REQUIRED);
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &borrower.pubkey(), revocation);
    env.expect_err(ix, &borrower, &[], E_NOT_HOLDER);
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    let meta = env.send(ix, &admin, &[]);
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
    env.expect_err(ix, &admin, &[], E_PROCURACION_STATE);
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
/// holder and the attestor record the document's hash as an entry; nothing moves on the sidecar or
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
    let admin = env.admin.insecure_clone();
    let attestor = env.attestor.insecure_clone();
    let a = admin.pubkey();
    let embargo = [0x6a; 32];

    // Refusals: a kind that is not a court's, a zero hash, a non-holder, a stranger for attestor.
    for kind in [KIND_PROCURACION, KIND_RECIBO, KIND_RECIBO + 1] {
        let ix = env.court_annotation_ix(&k, line_id, 0, &a, kind, embargo);
        env.expect_err(ix, &admin, &[&attestor], E_ANNOTATION_KIND_INVALID);
    }
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_ANOTACION_JUDICIAL, [0; 32]);
    env.expect_err(ix, &admin, &[&attestor], E_DOCUMENT_HASH_REQUIRED);
    let ix = env.court_annotation_ix(&k, line_id, 0, &borrower.pubkey(), KIND_ANOTACION_JUDICIAL, embargo);
    env.expect_err(ix, &borrower, &[&attestor], E_NOT_HOLDER);
    let stranger = env.funded(1);
    let mut ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_ANOTACION_JUDICIAL, embargo);
    let i = ix.accounts.iter().position(|m| m.pubkey == attestor.pubkey()).unwrap();
    ix.accounts[i].pubkey = stranger.pubkey();
    env.expect_err(ix, &admin, &[&stranger], E_ATTESTOR_REQUIRED);

    // The embargo: one entry, from and to the pool (no counterparty), the rest untouched.
    let before = env.pagare(&k, line_id, 0);
    let meta = env.court_annotation(&k, line_id, 0, KIND_ANOTACION_JUDICIAL, embargo);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_ANOTACION_JUDICIAL, &k.pool, &k.pool, &embargo, now, 2);
    assert_eq!((after.entry_count, after.last_kind), (2, KIND_ANOTACION_JUDICIAL));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    assert_eq!(
        (after.status, after.flags, after.procurador, after.paid_at, after.receipt_hash, after.amount, after.due_at),
        (before.status, before.flags, before.procurador, before.paid_at, before.receipt_hash, before.amount, before.due_at),
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
    env.send(ix, &admin, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    env.court_annotation(&k, line_id, 0, KIND_ANOTACION_JUDICIAL, embargo);
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 2);

    // A burnt title is refused; its sibling on the repaid line is not.
    let (_, line_id) = drawn_and_repaid(&mut env, &k, 4 * UNIT, 1_000, &[UNIT, 3 * UNIT]);
    env.cancel_pagare(&k, line_id, 0);
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_CONSTANCIA_JUDICIAL, embargo);
    env.expect_err(ix, &admin, &[&attestor], E_PAGARE_ALREADY_PAID);
    env.court_annotation(&k, line_id, 1, KIND_CONSTANCIA_JUDICIAL, embargo);
    assert_eq!(env.pagare(&k, line_id, 1).last_kind, KIND_CONSTANCIA_JUDICIAL);
}
