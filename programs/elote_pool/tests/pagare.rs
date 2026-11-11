//! The pagaré token: one Metaplex Core asset per disposición, minted by `draw`
//! into the series' collection, held frozen by the pool PDA, with a sidecar record. These tests
//! create through the mpl-core crate's builders and read back through its decoders against the
//! pinned program binary, which is the crate-vs-program compatibility proof.

mod common;
mod reference;

use common::*;
use elote_pool::errors::PoolError;

const E_PAGARE_ALREADY_DELIVERED: u32 = ERR_BASE + PoolError::PagareAlreadyDelivered as u32;
const E_BALANCE_OUTSTANDING: u32 = ERR_BASE + PoolError::BalanceOutstanding as u32;
use elote_pool::events::PagareIssued;
use elote_pool::state::{LineStatus, KIND_ENTREGA, PAGARE_FLAG_DELIVERED};
use mpl_core::types::{Key, UpdateAuthority};
use mpl_core::{AuthorityType, BaseAuthority};
use anchor_lang::Space;
use solana_signer::Signer;

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One asset and one sidecar per draw, frozen, owned by the pool, in the pool's collection; the
/// sidecar and the event carry the draw's facts; the relayer paid the rent.
#[test]
fn every_draw_issues_one_frozen_pagare_held_by_the_pool() {
    let mut env = Env::new();
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
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

/// Decision 55 leaves the pagaré hash as it was: a draw's `pagare_hash`, in the record and the
/// event, is the independent reference's derivation over the approved template, the terms, the
/// line, the index, the amount and the time, whatever message the borrower signed.
#[test]
fn the_pagare_hash_is_derived_as_before_decision_55() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    for (i, amount) in [(0u16, UNIT), (1, 2 * UNIT)] {
        let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), amount);
        let ev: PagareIssued = find_event(&env.send(ix, &borrower, &[])).expect("PagareIssued");
        let line = env.line(&k, line_id);
        let expected = reference::pagare::pagare_hash(&line.pagare_template_hash, &line.terms_hash, &line_key.to_bytes(), i, amount, env.now());
        assert_eq!((ev.pagare_hash, env.pagare(&k, line_id, i).pagare_hash), (expected, expected));
        env.warp(env.now() + 60);
    }
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
    // Attributes, nobody's, with the nine facts and nothing personal (the certificate commitment is not one of them).
    let attributes = asset.plugin_list.attributes.as_ref().expect("Attributes");
    assert_eq!(attributes.base.authority.authority_type, AuthorityType::None);
    let list: Vec<(&str, &str)> =
        attributes.attributes.attribute_list.iter().map(|a| (a.key.as_str(), a.value.as_str())).collect();
    let amount = (3 * UNIT).to_string();
    let issued = issued_at.to_string();
    let due = p.term_end.to_string();
    let line_s = line_id.to_string();
    let signed = hex(&env.default_pagare_signature(&line_key, 0).hash);
    assert_eq!(
        list,
        vec![
            ("pagare_hash", hex(&hash).as_str()),
            ("signed_pagare_hash", signed.as_str()),
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
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
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
    let k = pool_capped(&mut env, 10 * UNIT, BPS); // a line of 6 of 10
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
    let payoff = env.payoff(k, line_id);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), payoff - env.line(k, line_id).drawn);
    env.repay(k, line_id, &borrower, payoff);
    assert_eq!(env.line(k, line_id).status, LineStatus::Repaid);
    (borrower, line_id)
}

/// The payment writes the PAGADO mark; the delivery hands the título over (decision 38): the pool
/// thaws the asset, renounces the freeze forever and transfers it to the borrower, whoever sends
/// the instruction. The record stays with an Entrega entry and the DELIVERED flag; the owner may
/// then burn the asset himself through `burn_pagare`, taking the rent, and the record still stays.
/// A second delivery is refused.
#[test]
fn a_paid_pagare_is_delivered_to_the_borrower_and_the_owner_may_burn_it() {
    use elote_pool::events::{PagareBurned, PagareDelivered};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let (borrower, line_id) = drawn_and_repaid(&mut env, &k, 4 * UNIT, 1_000, &[UNIT, 3 * UNIT]);
    let line_key = line_pda(&k.pool, line_id).0;
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    let sidecar_key = pagare_pda(&line_key, 0).0;
    let before = env.pagare(&k, line_id, 0);
    assert_eq!((before.status, before.flags & PAGARE_FLAG_DELIVERED), (PAGARE_PAID, 0), "paid at the repayment, not yet delivered");
    let sidecar_lamports = env.account(&sidecar_key).lamports;
    let asset_lamports = env.account(&asset0).lamports;
    assert_eq!(env.collection_state(&k.collection).current_size, 2);

    // A stranger delivers; the título still goes to the borrower.
    let relayer = env.funded(1);
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &relayer.pubkey(), &borrower.pubkey());
    let meta = env.send(ix, &relayer, &[]);
    let now = env.now();
    let ev: PagareDelivered = find_event(&meta).expect("PagareDelivered");
    assert_eq!(
        (ev.pool, ev.line_id, ev.draw_index, ev.asset, ev.recipient, ev.payer, ev.delivered_at),
        (k.pool, line_id, 0, asset0, borrower.pubkey(), relayer.pubkey(), now)
    );

    // The asset: the borrower's, thawed, nobody can freeze it again, still in the collection.
    let asset = env.asset_state(&asset0);
    assert_eq!(asset.base.owner, borrower.pubkey());
    let pfd = asset.plugin_list.permanent_freeze_delegate.expect("the PFD stays, powerless");
    assert!(!pfd.permanent_freeze_delegate.frozen);
    assert_eq!(pfd.base.authority.address, None, "renounced: authority None");
    assert_eq!(env.collection_state(&k.collection).current_size, 2, "a transfer shrinks nothing");

    // The record: the Entrega entry and the flag; everything else as issued, the rent kept.
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.status, after.flags & PAGARE_FLAG_DELIVERED != 0, after.last_kind), (PAGARE_PAID, true, KIND_ENTREGA));
    assert_eq!(after.entry_count, before.entry_count + 1);
    assert_eq!(
        (after.line, after.draw_index, after.asset, after.pagare_hash, after.amount_mxn, after.issued_at, after.due_at),
        (before.line, before.draw_index, before.asset, before.pagare_hash, before.amount_mxn, before.issued_at, before.due_at)
    );
    assert_eq!(env.account(&sidecar_key).lamports, sidecar_lamports, "the record is never closed");
    // Delivered is terminal.
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_ALREADY_DELIVERED);

    // The owner burns, optionally: tombstone, rent back, the collection one smaller, the record
    // stays with a Terminacion entry.
    let owner_before = env.account(&borrower.pubkey()).lamports;
    let ix = env.burn_pagare_ix(&k, line_id, 0, &borrower.pubkey(), [0x7a; 32]);
    let meta = env.send(ix, &borrower, &[]);
    let ev: PagareBurned = find_event(&meta).expect("PagareBurned");
    assert_eq!((ev.asset, ev.owner, ev.document_hash), (asset0, borrower.pubkey(), [0x7a; 32]));
    let tomb = env.account(&asset0);
    assert_eq!((tomb.data.len(), tomb.data[0], tomb.owner), (1, Key::Uninitialized as u8, MPL_CORE_ID), "the asset is burnt");
    let returned = asset_lamports - tomb.lamports;
    let owner_after = env.account(&borrower.pubkey()).lamports;
    assert!(returned > 2_000_000 && owner_after > owner_before && owner_after <= owner_before + returned, "the asset's rent returns to the owner");
    assert_eq!((env.collection_state(&k.collection).num_minted, env.collection_state(&k.collection).current_size), (2, 1));
    let record = env.pagare(&k, line_id, 0);
    assert_eq!((record.last_kind, record.entry_count), (elote_pool::state::KIND_TERMINACION, after.entry_count + 1), "the record outlives the asset");

    // The second pagaré delivers in turn.
    env.deliver_pagare(&k, line_id, 1);
    assert!(env.pagare(&k, line_id, 1).flags & PAGARE_FLAG_DELIVERED != 0);
}

/// The coverage rule: pagaré `k` is paid once the line's principal repaid reaches its
/// `drawn_through`, so repayments cover the pagarés oldest first and a later one cannot be
/// marked paid while an earlier one is uncovered. Interest covers nothing by itself: `repay`
/// settles it before principal, so a payment the size of the pagaré leaves principal owed, and a
/// título is never marked paid while any of its principal is owed. A covered pagaré still waits
/// for its line to owe nothing (`BalanceOutstanding`, decision 49); an uncovered one is refused
/// first (`PagareNotCovered`).
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
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT - 1);
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 1);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    env.expect_deliver_err(&k, line_id, 2, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 2 * UNIT);
    assert_eq!(env.pagare(&k, line_id, 1).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 1, E_BALANCE_OUTSTANDING);
    env.expect_deliver_err(&k, line_id, 2, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    // Order among covered pagarés is free: the last one first.
    env.deliver_pagare(&k, line_id, 2);
    env.deliver_pagare(&k, line_id, 1);
    env.deliver_pagare(&k, line_id, 0);
    assert_eq!(env.collection_state(&k.collection).current_size, 3, "delivered, not burnt");

    // At 10%, two tranches of 1.0 with 0.1 of interest each, at the term: a repayment of 1.0
    // settles the 0.2 of interest accrued and only 0.8 of principal, so the first pagaré is not
    // covered until 0.2 more arrives.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.warp(p.term_end);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (200_000, 800_000));
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, 200_000);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);

    // At 100% for a year the interest equals the principal: a payment the size of the pagaré is
    // all interest and covers nothing; the second payment repays the principal and closes the line.
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 10_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    // At the term: a year of 100% on 1.0 has accrued 1.0.
    env.warp(env.line(&k, line_id).term_end);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), UNIT);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part), (UNIT, 0));
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
}

/// A título leaves only when its line owes nothing (decision 49, LGTOC 17 and 130), whatever the
/// line's status: a pagaré a payment covered stays with the pool while its line is in collection,
/// until a payment settles the line, and while a written-off line still owes: the write-off is the
/// pool's loss, not the debtor's release, and a recovery that settles it releases them
/// (decision 51). The pagarés no payment covered stay in force for the collection. A cancelled line
/// never drew, so there is nothing to deliver.
#[test]
fn no_titulo_leaves_a_line_that_owes_whatever_its_status() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();

    // In collection: a payment after the mark covers pagaré 0; it is delivered, the line stays in
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
    let cover0 = env.arrears(&k, line_id).late_due + env.interest_unpaid(&k, line_id) + UNIT;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), cover0);
    env.repay(&k, line_id, &borrower, cover0);
    assert_eq!(env.line(&k, line_id).status, LineStatus::InCollection);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    // The payment that settles the line in collection releases both.
    let rest = env.payoff(&k, line_id);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), rest);
    env.repay(&k, line_id, &borrower, rest);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 1);
    assert!(env.pagare(&k, line_id, 0).flags & env.pagare(&k, line_id, 1).flags & PAGARE_FLAG_DELIVERED != 0);

    // Written off: the covered pagaré stays with the pool, and the other outlives the loss.
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let cover0 = env.interest_unpaid(&k, line_id) + UNIT;
    env.repay(&k, line_id, &borrower, cover0);
    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &a);
    env.send(ix, &council, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    assert_eq!(env.pagare(&k, line_id, 1).status, PAGARE_OUTSTANDING);
    // A recovery reaches the written-off line (decision 51): a part leaves the títulos with the
    // pool, the rest settles the line and both leave.
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 2 * UNIT);
    let part = env.payoff(&k, line_id) - UNIT / 2;
    env.repay(&k, line_id, &borrower, part);
    assert_eq!(env.line(&k, line_id).status, LineStatus::WrittenOff);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    let rest = env.payoff(&k, line_id);
    env.repay(&k, line_id, &borrower, rest);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 1);

    // Cancelled: no sidecar was ever created.
    let borrower = env.borrower(&k);
    let p = env.line_params(UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let ix = env.cancel_line_ix(&k, line_id, &council.pubkey());
    env.send(ix, &council, &[]);
    let line_key = line_pda(&k.pool, line_id).0;
    let mut ix = env.deliver_pagare_ix_raw(&k, line_id, 0, &a, &borrower.pubkey(), pagare_asset_pda(&line_key, 0).0, k.collection);
    ix.program_id = PROGRAM_ID;
    env.expect_err(ix, &council, &[], E_ACCOUNT_NOT_INITIALIZED);
}

/// Decision 49 (LGTOC 17 and 130): a pagaré whose principal is repaid while its line still owes
/// is held: the covered título stays (`BalanceOutstanding`) while a later tranche owes principal,
/// and leaves once the line owes nothing. With interest taken first (decision 48) a payment that
/// covers a pagaré leaves no interest accrued unpaid; what holds the título back is the principal
/// of the later tranche and the interest it accrues.
#[test]
fn a_covered_pagare_waits_until_the_line_owes_nothing() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.warp(env.now() + 30 * 86_400);
    env.mint_to(&k.asset_mint, &council, &b, UNIT);
    // Thirty days of 10% on 2.0: 2_000_000 x 1_000 x 2_592_000 / 315_360_000_000 = 16_438.3.
    let ev = env.repay(&k, line_id, &borrower, UNIT + 16_438);
    assert_eq!((ev.interest_part, ev.principal_part), (16_438, UNIT));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.principal_repaid, env.pagare(&k, line_id, 0).status), (LineStatus::Active, UNIT, PAGARE_PAID));
    assert_eq!(line.interest_unpaid(env.now()), Some(0), "nothing accrued is left unpaid");
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);

    // A day later the line owes pagaré 1 and a day of 10% on it, 273.97, with the 0.36 the first
    // thirty days left over: 16_712.3 accrued in all, 16_438 paid, 274 to pay (rounded once).
    env.warp(env.now() + 86_400);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    let ev = env.repay(&k, line_id, &borrower, UNIT + 274);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (274, UNIT, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.owed(env.now())), (LineStatus::Repaid, Some(0)));
    env.deliver_pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 1);
    assert_eq!(env.asset_state(&env.pagare(&k, line_id, 0).asset).base.owner, b);
}

/// Decision 49 tests the line's balance, not its status: an active line that can still draw and
/// owes nothing (here at 0%) delivers its paid título. A later draw makes the line owe again; the
/// delivered título stays delivered and the new one waits for its own coverage.
#[test]
fn an_active_line_that_owes_nothing_delivers_and_draws_on() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 0);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.repay(&k, line_id, &borrower, UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.owed(env.now())), (LineStatus::Active, Some(0)));
    assert!(line.drawn < line.principal && env.now() < line.availability_end, "the line can still draw");
    env.deliver_pagare(&k, line_id, 0);
    assert_eq!(env.asset_state(&env.pagare(&k, line_id, 0).asset).base.owner, borrower.pubkey());

    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.expect_deliver_err(&k, line_id, 0, E_PAGARE_ALREADY_DELIVERED);
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    env.repay(&k, line_id, &borrower, UNIT);
    env.deliver_pagare(&k, line_id, 1);
}

/// The delivery takes no permission but demands the facts: the PAGADO mark, the right recipient
/// (the line's borrower, never the sender), the pagaré's own asset and the pool's collection. No
/// court hold may be in force.
#[test]
fn deliver_pagare_requires_the_mark_the_recipient_and_the_right_accounts() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let (borrower, line_id) = drawn_and_repaid(&mut env, &k, 2 * UNIT, 1_000, &[UNIT, UNIT]);
    let line_key = line_pda(&k.pool, line_id).0;
    let council = env.council.insecure_clone();
    let a = council.pubkey();
    let stranger = env.funded(1);

    // The recipient must be the borrower: the sender, a stranger or the council are refused.
    for wrong in [stranger.pubkey(), a] {
        let ix = env.deliver_pagare_ix(&k, line_id, 0, &stranger.pubkey(), &wrong);
        env.expect_err(ix, &stranger, &[], E_WRONG_ACCOUNT);
    }
    // Another pagaré's asset, or a stranger's.
    for wrong in [pagare_asset_pda(&line_key, 1).0, solana_pubkey::Pubkey::new_unique()] {
        let ix = env.deliver_pagare_ix_raw(&k, line_id, 0, &a, &borrower.pubkey(), wrong, k.collection);
        env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
    }
    // Another pagaré's sidecar for this index.
    let mut ix = env.deliver_pagare_ix(&k, line_id, 0, &a, &borrower.pubkey());
    let i = ix.accounts.iter().position(|m| m.pubkey == pagare_pda(&line_key, 0).0).unwrap();
    ix.accounts[i].pubkey = pagare_pda(&line_key, 1).0;
    env.expect_err(ix, &council, &[], E_CONSTRAINT_SEEDS);
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let ix = env.deliver_pagare_ix_raw(&k, line_id, 0, &a, &borrower.pubkey(), pagare_asset_pda(&line_key, 0).0, other);
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);

    // Court acts annotate and gate nothing (decision 38-g: the judicial stage runs off chain);
    // a paid, undelivered título still takes them, and the delivery below proceeds regardless.
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, elote_pool::state::KIND_ANOTACION_JUDICIAL, [0x66; 32]);
    env.send(ix, &council, &[]);

    // Nothing happened yet: the title keeps the repayment's mark and its asset; now it delivers,
    // a stranger sending, to the borrower.
    assert_eq!(env.pagare(&k, line_id, 0).status, PAGARE_PAID);
    assert_eq!(env.asset_state(&pagare_asset_pda(&line_key, 0).0).base.owner, k.pool);
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &stranger.pubkey(), &borrower.pubkey());
    env.send(ix, &stranger, &[]);
    assert_eq!(env.asset_state(&pagare_asset_pda(&line_key, 0).0).base.owner, borrower.pubkey());

    // The burn: only after the delivery, and only by the owner — a burn delegate the owner
    // approved through Core can burn there, but the Terminacion entry is the owner's act, so the
    // program refuses the delegate before any CPI (the entry would lie about who ended the título).
    let ix = env.burn_pagare_ix(&k, line_id, 1, &borrower.pubkey(), [0; 32]);
    env.expect_err(ix, &borrower, &[], E_PAGARE_NOT_COVERED);
    let ix = env.burn_pagare_ix(&k, line_id, 0, &stranger.pubkey(), [0; 32]);
    env.expect_err(ix, &stranger, &[], E_NOT_HOLDER);
    let delegate = env.funded(1);
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    let add = mpl_core::instructions::AddPluginV1Builder::new()
        .asset(asset0)
        .collection(Some(k.collection))
        .payer(borrower.pubkey())
        .plugin(mpl_core::types::Plugin::BurnDelegate(mpl_core::types::BurnDelegate {}))
        .init_authority(mpl_core::types::PluginAuthority::Address { address: delegate.pubkey() })
        .instruction();
    env.send(add, &borrower, &[]);
    let ix = env.burn_pagare_ix(&k, line_id, 0, &delegate.pubkey(), [0; 32]);
    env.expect_err(ix, &delegate, &[], E_NOT_HOLDER);
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

/// What a payment entry commits (decision 38): domain, receipt, amounts, balance, who paid.
fn annotation(receipt: &[u8; 32], principal_applied: u64, accessory_applied: u64, principal_balance: u64, by_borrower: bool) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"elote.pago.v2");
    h.update(receipt);
    h.update(principal_applied.to_le_bytes());
    h.update(accessory_applied.to_le_bytes());
    h.update(principal_balance.to_le_bytes());
    h.update([u8::from(by_borrower)]);
    h.finalize().into()
}

/// A line of four pagarés settles in two payments (decision 48). The payment on account takes the
/// interest accrued, then principal; the pagarés it covers are paid with no interest left unpaid,
/// so their entries reserve nothing, and none leaves the pool until the line owes nothing
/// (decision 49). The settling payment, with its own receipt, releases all four; each `Entrega`
/// entry commits its record's own `receipt_hash`. The principal repaid early lowers the interest
/// that follows: the two payments together cost what the reference says, less than one payoff at
/// the end would.
#[test]
fn a_split_settlement_delivers_every_titulo_once_the_line_owes_nothing() {
    use elote_pool::events::{PagareDelivered, PagarePayment};
    use elote_pool::state::KIND_PAGADO;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let b = borrower.pubkey();
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    for _ in 0..4 {
        env.draw_amount(&k, line_id, &borrower, UNIT);
    }
    let line = env.line(&k, line_id);
    let mut r = reference::RefLine::new(1_000, line.term_end);
    r.draw(line.drawn_at, 4 * UNIT);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &b, UNIT);
    let half = line.drawn_at + (line.term_end - line.drawn_at) / 2;
    env.warp(half);
    let balance = env.token_balance(&k.asset_ata(&b));
    let (on_account, settling) = ([0xa1; 32], [0xb2; 32]);

    // On account: half a year of 10% on 4.0, 200_000, and two and a half pagarés.
    let before0 = env.pagare(&k, line_id, 0);
    let amount = 2 * UNIT + UNIT / 2 + 200_000;
    let ix = env.repay_ix_full(&k, line_id, &b, amount, on_account, &env.payment_sidecars(&k, line_id, amount));
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.interest_part, ev.principal_part), (200_000, 2 * UNIT + UNIT / 2));
    assert_eq!(r.pay(now, amount, 0), (0, 200_000, 2 * UNIT + UNIT / 2));
    let line = env.line(&k, line_id);
    assert_eq!((line.pagares_paid, line.interest_unpaid(now)), (2, Some(0)));
    let p0 = env.pagare(&k, line_id, 0);
    let pagado = annotation(&on_account, UNIT, 200_000, 0, true);
    assert_eq!(p0.log_root, chain(&before0.log_root, &entry_hash(&p0.asset, KIND_PAGADO, &b, &k.pool, &pagado, now, 2)));

    // Nothing leaves between the payments: the paid ones wait for the line, the others for cover.
    for i in 0..2 {
        assert_eq!(env.pagare(&k, line_id, i).status, PAGARE_PAID);
        env.expect_deliver_err(&k, line_id, i, E_BALANCE_OUTSTANDING);
    }
    for i in 2..4 {
        env.expect_deliver_err(&k, line_id, i, E_PAGARE_NOT_COVERED);
    }

    // The settling payment, a day later: a day of 10% on the 1.5 left, 410.9, and the principal.
    env.warp(env.now() + 86_400);
    let ix = env.repay_ix_full(&k, line_id, &b, 4 * UNIT, settling, &env.payment_sidecars(&k, line_id, 4 * UNIT));
    let meta = env.send(ix, &borrower, &[]);
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.interest_part, ev.principal_part), (410, UNIT + UNIT / 2));
    assert_eq!(r.pay(env.now(), 4 * UNIT, 0), (0, 410, UNIT + UNIT / 2));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.pagares_paid), (LineStatus::Repaid, 4));
    assert_eq!(balance - env.token_balance(&k.asset_ata(&b)), 4 * UNIT + 200_410);
    // One payoff at the same second would have paid 4.0 for the half year and the day: 201_095.
    assert_eq!(4_000_000u128 * 1_000 * (YEAR as u128 / 2 + 86_400) / (10_000 * YEAR as u128), 201_095);

    // All four leave; each Entrega entry is over the receipt of the payment that paid its pagaré.
    for (i, receipt) in [(0u16, on_account), (1, on_account), (2, settling), (3, settling)] {
        let before = env.pagare(&k, line_id, i);
        assert_eq!(before.receipt_hash, receipt);
        let meta = env.deliver_pagare(&k, line_id, i);
        let now = env.now();
        let ev: PagareDelivered = find_event(&meta).expect("PagareDelivered");
        assert_eq!((ev.draw_index, ev.recipient, ev.receipt_hash, ev.entry_count), (i, b, receipt, before.entry_count + 1));
        let after = env.pagare(&k, line_id, i);
        let entrega = entry_hash(&after.asset, KIND_ENTREGA, &k.pool, &b, &receipt, now, before.entry_count + 1);
        assert_eq!(after.log_root, chain(&before.log_root, &entrega), "Entrega commits the record's receipt_hash");
    }
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
    // Decision 55: the issuance states the pagaré hash bound to the hash of the signed message.
    let signed = env.default_pagare_signature(&line_pda(&k.pool, line_id).0, 0).hash;
    let issuance = reference::pagare::issuance_hash(&env.expected_pagare_hash(&k, line_id, 0, UNIT, now), &signed);
    let expected = entry_hash(&sidecar.asset, KIND_EMISION, &k.pool, &k.pool, &issuance, now, 1);
    assert_eq!((sidecar.entry_count, sidecar.last_kind, sidecar.flags), (1, KIND_EMISION, 0));
    assert_eq!(sidecar.log_root, chain(&[0; 32], &expected), "genesis: the issuance entry over an empty root");
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

/// Endoso en procuración (the only endoso the program records): the pool stays the owner, the
/// entry goes into the registry and the sidecar names the procurador — and that is all. No power
/// over the asset is lent (decision 38): the procurador collects through `repay` and litigates
/// with the entry; the asset carries no `BurnDelegate` and stays frozen with the pool.
#[test]
fn an_endoso_en_procuracion_appends_the_entry_and_lends_no_power_over_the_asset() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_EMISION, KIND_PROCURACION, PAGARE_FLAG_PROCURACION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let before = env.pagare(&k, line_id, 0);
    assert_eq!((before.entry_count, before.last_kind, before.flags), (1, KIND_EMISION, 0));
    let procurador = env.funded(1);

    let meta = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let now = env.now();
    let after = env.pagare(&k, line_id, 0);
    let expected = entry_hash(&after.asset, KIND_PROCURACION, &k.pool, &procurador.pubkey(), &ENDOSO_HASH, now, 2);
    assert_eq!((after.entry_count, after.last_kind, after.flags), (2, KIND_PROCURACION, PAGARE_FLAG_PROCURACION));
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    assert_eq!((after.status, after.asset, after.drawn_through), (before.status, before.asset, before.drawn_through), "nothing else moves");
    assert_eq!((before.procurador, after.procurador), (solana_pubkey::Pubkey::default(), procurador.pubkey()), "the sidecar names the procurador");
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count, ev.recorded_at, ev.stated_at), (KIND_PROCURACION, k.pool, procurador.pubkey(), expected, 2, now, now));
    // The asset: still the pool's, still frozen, and no BurnDelegate exists.
    let asset = env.asset_state(&after.asset);
    assert_eq!(asset.base.owner, k.pool);
    assert!(asset.plugin_list.permanent_freeze_delegate.as_ref().unwrap().permanent_freeze_delegate.frozen);
    assert!(asset.plugin_list.burn_delegate.is_none(), "no power over the asset is lent");
    // The procurador's key cannot burn, and a second procuración while one is in force is refused.
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

/// The endorsee is a third party to the pool and the line: neither the pool's creator, nor its FIX
/// feeder, nor the line's rent payer, nor its sponsor (`EndorseeInvalid`). A stranger is accepted.
#[test]
fn endorse_pagare_refuses_the_creator_the_feeder_the_payer_and_the_sponsor() {
    use elote_pool::state::{KIND_PROCURACION, SPONSOR_GUARANTOR};
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let council = env.council.insecure_clone();
    let a = council.pubkey();
    let feeder = env.funded(1);
    let ix = env.set_fx_feeder_ix(&k, &a, &feeder.pubkey());
    env.send(ix, &council, &[]);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let mut p = env.line_params(4 * UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());
    p.sponsor_role = SPONSOR_GUARANTOR;
    // The rent payer is a key of its own, as the backend's would be.
    let payer = env.funded(1);
    env.ensure_fix(&k);
    let line_id = env.pool(&k).line_count;
    let ix = env.request_line_ix(&k, &payer.pubkey(), line_id, &borrower.pubkey(), &p, Some(sponsor.pubkey()));
    env.send(ix, &payer, &[&borrower, &sponsor]);
    env.approve_line(&k, line_id, &borrower.pubkey(), &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let line = env.line(&k, line_id);
    let creator = env.pool(&k).creator;
    assert_eq!(line.payer, payer.pubkey());
    for to in [creator, feeder.pubkey(), line.payer, sponsor.pubkey()] {
        let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, to, ENDOSO_HASH);
        env.expect_err(ix, &council, &[], E_ENDORSEE_INVALID);
    }
    assert_eq!(env.pagare(&k, line_id, 0).entry_count, 1, "nothing was appended");
    let procurador = env.funded(1);
    let ix = env.endorse_pagare_ix(&k, line_id, 0, &a, KIND_PROCURACION, procurador.pubkey(), ENDOSO_HASH);
    env.send(ix, &council, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).procurador, procurador.pubkey());
}

/// The revocation of a procuración (LGTOC 41): the owner alone appends the revocation entry and
/// takes the burn delegate back to `Owner`; the in-force flag clears, the plugin flag stays, and a
/// later procuración approves the existing plugin instead of adding it.
#[test]
fn revoke_procuracion_appends_the_entry_and_clears_the_record() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_PROCURACION, KIND_REVOCACION, PAGARE_FLAG_PROCURACION};
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
    assert_eq!((after.entry_count, after.last_kind, after.flags), (3, KIND_REVOCACION, 0));
    assert_eq!(after.procurador, solana_pubkey::Pubkey::default(), "nobody collects now");
    assert_eq!(after.log_root, chain(&before.log_root, &expected));
    let ev: PagareEntry = find_event(&meta).expect("PagareEntry");
    assert_eq!((ev.kind, ev.from, ev.to, ev.entry_hash, ev.entry_count), (KIND_REVOCACION, k.pool, procurador.pubkey(), expected, 3));
    // There was never any power over the asset to take back.
    let asset = env.asset_state(&after.asset);
    assert!(asset.plugin_list.burn_delegate.is_none());
    // Revoking twice is refused; a new procuración to another procurador just records it.
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &a, revocation);
    env.expect_err(ix, &council, &[], E_PROCURACION_STATE);
    let other = env.funded(1);
    env.procuracion(&k, line_id, 0, &other.pubkey(), [0x5f; 32]);
    let again = env.pagare(&k, line_id, 0);
    assert_eq!((again.entry_count, again.last_kind, again.flags), (4, KIND_PROCURACION, PAGARE_FLAG_PROCURACION));
    assert_eq!(again.procurador, other.pubkey());
    assert!(env.asset_state(&again.asset).plugin_list.burn_delegate.is_none());
}

/// A court's act on a title (embargo, suspension, cancellation; the constancia of art. 28): the
/// council records the document's hash as an entry; nothing moves on the sidecar or
/// the asset. Allowed on any line status, written-off included, and beside a procuración in force;
/// refused only on a delivered title, which is the owner's. No court act gates anything: the
/// judicial stage of a collection runs off chain (decision 38-g).
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

    // A delivered title is the owner's and refused; its paid, undelivered sibling still takes a
    // court act (the hold can land between the payment and the delivery).
    let (_, line_id) = drawn_and_repaid(&mut env, &k, 4 * UNIT, 1_000, &[UNIT, 3 * UNIT]);
    env.deliver_pagare(&k, line_id, 0);
    let ix = env.court_annotation_ix(&k, line_id, 0, &a, KIND_CONSTANCIA_JUDICIAL, embargo);
    env.expect_err(ix, &council, &[], E_PAGARE_ALREADY_PAID);
    env.court_annotation(&k, line_id, 1, KIND_ANOTACION_JUDICIAL, embargo);
    assert_eq!(env.pagare(&k, line_id, 1).last_kind, KIND_ANOTACION_JUDICIAL);
    env.deliver_pagare(&k, line_id, 1);
}

/// The procurador collects through `repay`, like any payer, and the settling payment itself ends
/// the procuración: a `Terminacion` entry from the pool to the procurador follows the `Pagado`,
/// the sidecar clears its record, and the sibling's procuración stands (decision 38; LGTOC 35
/// gives the procurador the collection, which the payment is).
#[test]
fn the_payment_ends_the_procuracion_and_the_procurador_keeps_no_record() {
    use elote_pool::events::PagareEntry;
    use elote_pool::state::{KIND_PAGADO, KIND_TERMINACION, PAGARE_FLAG_PROCURACION};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let procurador = env.funded(1);
    let pr = procurador.pubkey();

    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.procuracion(&k, line_id, 0, &pr, ENDOSO_HASH);
    env.procuracion(&k, line_id, 1, &pr, ENDOSO_HASH);
    env.warp(p.term_end);
    let interest = 200_000; // a year of 10% on 2.0
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    let meta = env.send(env.repay_ix_full(&k, line_id, &borrower.pubkey(), UNIT + interest, RECEIPT_HASH, &env.payment_sidecars(&k, line_id, UNIT + interest)), &borrower, &[]);
    assert_eq!(env.line(&k, line_id).principal_repaid, UNIT);

    // Pagaré 0: Emision, Procuracion, Pagado, Terminacion — the payment wrote the last two.
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(
        (p0.status, p0.last_kind, p0.entry_count, p0.flags & PAGARE_FLAG_PROCURACION, p0.procurador),
        (PAGARE_PAID, KIND_TERMINACION, 4, 0, solana_pubkey::Pubkey::default()),
        "the procuración ends with the payment"
    );
    let entries: Vec<_> = find_events::<PagareEntry>(&meta).into_iter().map(|e| (e.draw_index, e.kind, e.from, e.to, e.entry_count)).collect();
    assert!(entries.contains(&(0, KIND_PAGADO, borrower.pubkey(), k.pool, 3)), "the Pagado is entry 3");
    assert!(entries.contains(&(0, KIND_TERMINACION, k.pool, pr, 4)), "the Terminacion chains after it");
    // The PaymentEntry names the Pagado entry, not the Terminacion that chained after it: the
    // annotation preimages must replay, and annotate_payment follows this number.
    let payment: elote_pool::events::PagarePayment = find_event(&meta).expect("PagarePayment");
    let pagado: Vec<_> = payment.entries.iter().map(|e| (e.draw_index, e.kind, e.entry_count)).collect();
    assert_eq!(pagado, vec![(0, KIND_PAGADO, 3)]);
    assert_eq!(env.pagare(&k, line_id, 1).procurador, pr, "the sibling's procuración stands");

    // The delivery goes to the borrower as always, once the line owes nothing (decision 49); the
    // ex-procurador has no claim on the asset.
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    env.deliver_pagare(&k, line_id, 0);
    assert_eq!(env.asset_state(&env.pagare(&k, line_id, 0).asset).base.owner, borrower.pubkey());
}

/// The council's annulment of a título issued in error (decision 38): a `Terminacion` entry over
/// the annulment document's own hash, nothing burnt, no state moved — the record is the
/// annotation, and the debt still follows the line.
#[test]
fn the_council_annuls_an_outstanding_titulo_by_annotation_alone() {
    use elote_pool::events::PagareAnnulled;
    use elote_pool::state::KIND_TERMINACION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);

    // A document hash is required, and a stranger is no holder.
    let ix = env.annul_pagare_ix(&k, line_id, 0, [0; 32]);
    env.expect_err(ix, &council, &[], E_DOCUMENT_HASH_REQUIRED);
    let stranger = env.funded(1);
    let mut ix = env.annul_pagare_ix(&k, line_id, 0, [0x3c; 32]);
    ix.accounts[0].pubkey = stranger.pubkey();
    env.expect_err(ix, &stranger, &[], E_NOT_HOLDER);

    let before = env.pagare(&k, line_id, 0);
    let ix = env.annul_pagare_ix(&k, line_id, 0, [0x3c; 32]);
    let meta = env.send(ix, &council, &[]);
    let ev: PagareAnnulled = find_event(&meta).expect("PagareAnnulled");
    assert_eq!((ev.pool, ev.line_id, ev.draw_index, ev.document_hash), (k.pool, line_id, 0, [0x3c; 32]));
    let after = env.pagare(&k, line_id, 0);
    assert_eq!((after.status, after.last_kind, after.entry_count), (PAGARE_OUTSTANDING, KIND_TERMINACION, before.entry_count + 1));
    assert_eq!(env.asset_state(&after.asset).base.owner, k.pool, "nothing burnt, nothing moved");

    // The line is still the line: the borrower renounces the rest, pays it off and the título
    // delivers.
    let ix = env.renounce_availability_ix(&k, line_id, &borrower.pubkey());
    env.send(ix, &borrower, &[]);
    let interest = env.interest_unpaid(&k, line_id);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, UNIT + interest);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    // A paid título cannot be annulled: it is the debtor's.
    let ix = env.annul_pagare_ix(&k, line_id, 0, [0x3d; 32]);
    env.expect_err(ix, &council, &[], E_PAGARE_ALREADY_PAID);
    env.deliver_pagare(&k, line_id, 0);
}

/// The aval who pays a título receives it alive (LGTOC 115, decision 38-f): the covered pagaré
/// gets a `PagadoAval` entry and the `PAID_BY_AVAL` flag, and the delivery goes to the sponsor —
/// the borrower is refused as recipient — once the line owes nothing (decision 49): the aval's
/// título waits while the borrower still owes the rest, and each título then goes to whoever paid
/// it. What the aval then collects from the suscriptor is a judge's call; the registry's entries
/// state who paid what, and that is all the pool asserts.
#[test]
fn the_aval_who_pays_receives_the_live_titulo_once_the_line_owes_nothing() {
    use elote_pool::state::{KIND_PAGADO_AVAL, PAGARE_FLAG_PAID_BY_AVAL, SPONSOR_GUARANTOR};
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let council = env.council.insecure_clone();
    let borrower = env.borrower(&k);
    let aval = env.funded(1);
    env.create_ata(&aval.pubkey(), &k.asset_mint);
    let mut p = env.line_params(2 * UNIT, 1_000);
    p.sponsor = Some(aval.pubkey());
    p.sponsor_role = SPONSOR_GUARANTOR;
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, Some(&aval));
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.warp(env.line(&k, line_id).term_end);

    // The aval pays the interest, a year of 10% on 2.0, and the first pagaré.
    let first = UNIT + 200_000;
    env.mint_to(&k.asset_mint, &council, &aval.pubkey(), first);
    env.repay(&k, line_id, &aval, first);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.last_kind, p0.flags & PAGARE_FLAG_PAID_BY_AVAL != 0), (PAGARE_PAID, KIND_PAGADO_AVAL, true));
    // The line still owes the second pagaré: the aval's título waits.
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &council.pubkey(), &aval.pubkey());
    env.expect_err(ix, &council, &[], E_BALANCE_OUTSTANDING);

    // The borrower settles the rest; the line owes nothing.
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    assert_eq!(env.pagare(&k, line_id, 1).flags & PAGARE_FLAG_PAID_BY_AVAL, 0);

    // Pagaré 0 goes to the aval; the borrower is not the recipient of this one.
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &council.pubkey(), &borrower.pubkey());
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
    let ix = env.deliver_pagare_ix(&k, line_id, 0, &council.pubkey(), &aval.pubkey());
    env.send(ix, &council, &[]);
    // Pagaré 1, which the borrower paid, goes to the borrower and never to the aval.
    let ix = env.deliver_pagare_ix(&k, line_id, 1, &council.pubkey(), &aval.pubkey());
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
    env.deliver_pagare(&k, line_id, 1);
    assert_eq!(env.asset_state(&env.pagare(&k, line_id, 1).asset).base.owner, borrower.pubkey());
    let asset = env.asset_state(&p0.asset);
    assert_eq!(asset.base.owner, aval.pubkey(), "the título is the aval's, alive");
    assert!(!asset.plugin_list.permanent_freeze_delegate.as_ref().unwrap().permanent_freeze_delegate.frozen);
    // The aval may transfer it to the suscriptor off-program once paid; Core alone governs now.
    let ix = mpl_core::instructions::TransferV1Builder::new()
        .asset(p0.asset)
        .collection(Some(k.collection))
        .payer(aval.pubkey())
        .new_owner(borrower.pubkey())
        .instruction();
    env.send(ix, &aval, &[]);
    assert_eq!(env.asset_state(&p0.asset).base.owner, borrower.pubkey());
}

/// The payment entries (art. 130, "en un solo acto"): every `repay` appends one entry to each
/// sidecar it touches, oldest first: `Pagado` where the payment brings `principal_repaid` to the
/// pagaré's `drawn_through` (the PAGADO mark, `paid_at` and `receipt_hash` at the moment of
/// payment, only the burn waiting), `Abono` on the first one it leaves uncovered; the cursor
/// `pagares_paid` moves past the paid ones. A zero receipt hash is allowed and means "to follow".
#[test]
fn a_payment_writes_its_entries_on_the_sidecars_it_touches() {
    use elote_pool::events::{PagareEntry, PagarePayment};
    use elote_pool::state::{KIND_ABONO, KIND_PAGADO};
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
    env.warp(p.term_end);
    let line = env.line(&k, line_id);
    assert_eq!((line.draws, line.pagares_paid), (3, 0));
    let interest = 400_000; // a year of 10% on 4.0
    env.mint_to(&k.asset_mint, &council, &b, interest);

    // An interest-only payment leaves its Abono on the oldest outstanding pagaré (decision 38,
    // LGTOC 17 "o en lo accesorio"): principal applied zero, the accessories stated, the balance
    // intact, nothing marked.
    let half = interest / 2;
    let before = env.pagare(&k, line_id, 0);
    let ix = env.repay_ix_full(&k, line_id, &b, half, [0; 32], &env.payment_sidecars(&k, line_id, half));
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.entry_count, p0.status, p0.last_kind), (2, PAGARE_OUTSTANDING, KIND_ABONO));
    assert_eq!(p0.log_root, chain(&before.log_root, &entry_hash(&p0.asset, KIND_ABONO, &b, &k.pool, &annotation(&[0; 32], 0, half, UNIT, true), now, 2)));
    assert!(find_event::<PagareEntry>(&meta).is_some());
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.payer, ev.receipt_hash, ev.late_part, ev.interest_part, ev.principal_part), (b, [0; 32], 0, half, 0));
    let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.entry_count, e.principal_applied, e.accessory_applied, e.principal_balance, e.by_borrower)).collect();
    assert_eq!(e, vec![(0, KIND_ABONO, 2, 0, half, UNIT, true)]);
    assert_eq!((env.pagare(&k, line_id, 1).entry_count, env.line(&k, line_id).pagares_paid), (1, 0));

    // The rest of the interest and one and a half pagarés of principal: 0 is paid (the
    // accessories annotated on it, the oldest touched), 1 gets an Abono, 2 is untouched, the
    // cursor moves past 0.
    let before1 = env.pagare(&k, line_id, 1);
    let amount = (interest - half) + UNIT + UNIT / 2;
    let ix = env.repay_ix_full(&k, line_id, &b, amount, RECEIPT_HASH, &env.payment_sidecars(&k, line_id, amount));
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let p0 = env.pagare(&k, line_id, 0);
    let p1 = env.pagare(&k, line_id, 1);
    assert_eq!(
        (p0.status, p0.paid_at, p0.receipt_hash, p0.last_kind, p0.entry_count),
        (PAGARE_PAID, now, RECEIPT_HASH, KIND_PAGADO, 3),
        "the PAGADO mark at the moment of payment"
    );
    assert_eq!((p1.status, p1.last_kind, p1.entry_count), (PAGARE_OUTSTANDING, KIND_ABONO, 2));
    assert_eq!(p1.log_root, chain(&before1.log_root, &entry_hash(&p1.asset, KIND_ABONO, &b, &k.pool, &annotation(&RECEIPT_HASH, UNIT / 2, 0, UNIT / 2, true), now, 2)));
    assert_eq!(env.pagare(&k, line_id, 2).entry_count, 1, "untouched");
    assert_eq!(env.line(&k, line_id).pagares_paid, 1);
    let ev: PagarePayment = find_event(&meta).expect("PagarePayment");
    assert_eq!((ev.interest_part, ev.principal_part, ev.receipt_hash), (interest - half, UNIT + UNIT / 2, RECEIPT_HASH));
    let e: Vec<_> = ev.entries.iter().map(|e| (e.draw_index, e.kind, e.entry_count, e.principal_applied, e.accessory_applied, e.principal_balance, e.by_borrower)).collect();
    assert_eq!(e, vec![(0, KIND_PAGADO, 3, UNIT, interest - half, 0, true), (1, KIND_ABONO, 2, UNIT / 2, 0, UNIT / 2, true)]);

    // The rest: 1 and 2 are paid, the cursor passes the last draw and the line closes.
    env.repay(&k, line_id, &borrower, UNIT / 2 + 2 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!((line.pagares_paid, line.status), (3, LineStatus::Repaid));
    let (p1, p2) = (env.pagare(&k, line_id, 1), env.pagare(&k, line_id, 2));
    assert_eq!((p1.status, p1.last_kind, p1.entry_count), (PAGARE_PAID, KIND_PAGADO, 3));
    assert_eq!((p2.status, p2.last_kind, p2.entry_count, p2.paid_at), (PAGARE_PAID, KIND_PAGADO, 2, env.now()));
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
    env.warp(env.now() + 30 * 86_400);
    let interest = env.interest_unpaid(&k, line_id);
    assert!(interest > 0);
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

/// After the payment entries, `deliver_pagare` requires the PAGADO mark in place of the coverage
/// rule, and a line that owes nothing (decision 49); the payment's receipt and time stay on the
/// sidecar, and the Entrega entry chains over that receipt, not the settling payment's.
#[test]
fn deliver_pagare_requires_the_payments_pagado_mark() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    env.draw_amount(&k, line_id, &borrower, UNIT);
    let interest = env.interest_unpaid(&k, line_id);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    let payment = [0x11; 32];
    let amount = UNIT + interest;
    let ix = env.repay_ix_full(&k, line_id, &borrower.pubkey(), amount, payment, &env.payment_sidecars(&k, line_id, amount));
    env.send(ix, &borrower, &[]);
    let paid_at = env.now();
    env.warp(paid_at + 3_600);
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!((p0.status, p0.paid_at, p0.receipt_hash), (PAGARE_PAID, paid_at, payment));
    // The uncovered one has no mark: no delivery; the marked one waits for the line.
    env.expect_deliver_err(&k, line_id, 1, E_PAGARE_NOT_COVERED);
    env.expect_deliver_err(&k, line_id, 0, E_BALANCE_OUTSTANDING);
    let settling = [0x22; 32];
    // The principal left and the hour of interest accrued on it since.
    let rest = env.payoff(&k, line_id);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), rest);
    let ix = env.repay_ix_full(&k, line_id, &borrower.pubkey(), rest, settling, &env.payment_sidecars(&k, line_id, rest));
    env.send(ix, &borrower, &[]);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    // The marked one delivers; the payment's receipt and time stay, the Entrega chains over them.
    let before = env.pagare(&k, line_id, 0);
    env.deliver_pagare(&k, line_id, 0);
    let now = env.now();
    let p0 = env.pagare(&k, line_id, 0);
    assert_eq!(
        (p0.status, p0.paid_at, p0.receipt_hash, p0.flags & PAGARE_FLAG_DELIVERED != 0, p0.last_kind, p0.entry_count),
        (PAGARE_PAID, paid_at, payment, true, KIND_ENTREGA, 3)
    );
    let entrega = entry_hash(&p0.asset, KIND_ENTREGA, &k.pool, &borrower.pubkey(), &payment, now, 3);
    assert_eq!(p0.log_root, chain(&before.log_root, &entrega));
    assert_eq!(env.pagare(&k, line_id, 1).receipt_hash, settling);
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
    let amount = env.interest_unpaid(&k, line_id) + 1;
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

// -----------------------------------------------------------------------------------------------
// decision 55: the message the borrower signed
// -----------------------------------------------------------------------------------------------

const E_PAGARE_SIGNATURE_MISSING: u32 = ERR_BASE + PoolError::PagareSignatureMissing as u32;
const E_SIGNED_DAY_MISMATCH: u32 = ERR_BASE + PoolError::SignedDayMismatch as u32;
const E_SIGNED_DAY_INVALID: u32 = ERR_BASE + PoolError::SignedDayInvalid as u32;
/// 2026-11-04T00:00 in Mexico City, 06:00Z: ten hours after `START_TIME`.
const MX_MIDNIGHT_NOV_4: i64 = 1_793_772_000;

/// The borrower's real ed25519 signature over `message`, with its SHA-256 and `signed_on`.
fn signed_by(borrower: &Keypair, message: &[u8], signed_on: u32) -> PagareSignature {
    use sha2::{Digest, Sha256};
    let signature: [u8; 64] = borrower.sign_message(message).as_ref().try_into().unwrap();
    PagareSignature { hash: Sha256::digest(message).into(), signature, signed_on }
}

fn signed_fields(p: &elote_pool::state::Pagare) -> PagareSignature {
    PagareSignature { hash: p.signed_pagare_hash, signature: p.pagare_signature, signed_on: p.signed_on }
}

/// The draw keeps the hash, the signature and the day exactly as the borrower sent them; the event
/// carries the hash and the day, the Core asset the hash, and the issuance entry binds it through
/// `sha256(pagare_hash || signed_pagare_hash)`. The signature verifies against the borrower's key
/// for whoever holds the text; the program does not check it.
#[test]
fn a_draw_keeps_the_signed_pagare_message_and_the_issuance_commits_to_it() {
    use elote_pool::state::KIND_EMISION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let message = "Debo y pagaré incondicionalmente… pagaré 0 de la línea, texto completo".as_bytes();
    let signed = signed_by(&borrower, message, 20261103);
    assert_eq!(mexico_city_date(env.now()), 20261103);

    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed);
    let meta = env.send(ix, &borrower, &[]);
    let now = env.now();
    let record = env.pagare(&k, line_id, 0);
    assert_eq!(signed_fields(&record), signed, "stored as given");
    assert!(solana_signature::Signature::from(record.pagare_signature).verify(borrower.pubkey().as_ref(), message));

    let ev: PagareIssued = find_event(&meta).expect("PagareIssued");
    assert_eq!((ev.signed_pagare_hash, ev.signed_on, ev.pagare_hash), (signed.hash, signed.signed_on, record.pagare_hash));

    let asset = env.asset_state(&record.asset);
    let attributes = &asset.plugin_list.attributes.as_ref().expect("Attributes").attributes.attribute_list;
    let value = attributes.iter().find(|a| a.key == "signed_pagare_hash").map(|a| a.value.clone());
    assert_eq!(value, Some(hex(&signed.hash)));

    let issuance = reference::pagare::issuance_hash(&record.pagare_hash, &signed.hash);
    let entry = reference::pagare::entry_hash(&record.asset.to_bytes(), KIND_EMISION, &k.pool.to_bytes(), &k.pool.to_bytes(), &issuance, now, 1);
    assert_eq!(record.log_root, reference::pagare::genesis_root(&entry));
}

/// A zero hash or a zero signature is no subscription: refused before anything is written.
#[test]
fn a_draw_without_the_signed_hash_or_the_signature_is_refused_and_writes_nothing() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    let good = signed_by(&borrower, b"the pagare text", mexico_city_date(env.now()));
    let vault = env.token_balance(&k.vault);
    for signed in [PagareSignature { hash: [0; 32], ..good }, PagareSignature { signature: [0; 64], ..good }] {
        let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed);
        env.expect_err(ix, &borrower, &[], E_PAGARE_SIGNATURE_MISSING);
    }
    assert_eq!((env.line(&k, line_id).draws, env.token_balance(&k.vault)), (0, vault));
    assert!(env.svm.get_account(&pagare_pda(&line_key, 0).0).is_none());
    assert!(env.svm.get_account(&pagare_asset_pda(&line_key, 0).0).is_none());
}

/// The pagaré is subscribed the day of the draw, in Mexico City: yesterday and tomorrow are
/// refused, and a date that does not exist is invalid.
#[test]
fn a_pagare_signed_another_day_in_mexico_city_is_refused() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    // START_TIME is 2026-11-03T20:00Z, 14:00 on the 3rd in Mexico City.
    for (day, code) in [(20261102, E_SIGNED_DAY_MISMATCH), (20261104, E_SIGNED_DAY_MISMATCH), (20261131, E_SIGNED_DAY_INVALID), (0, E_SIGNED_DAY_INVALID)] {
        let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed_by(&borrower, b"text", day));
        env.expect_err(ix, &borrower, &[], code);
    }
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed_by(&borrower, b"text", 20261103));
    env.send(ix, &borrower, &[]);
    assert_eq!(env.pagare(&k, line_id, 0).signed_on, 20261103);
}

/// Mexico City's midnight is 06:00Z (UTC-6): a second before it the 3rd is today and the 4th
/// refused; at it the 4th is today and the 3rd refused.
#[test]
fn the_signed_day_turns_at_mexico_city_midnight() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    for (at, today, other) in [(MX_MIDNIGHT_NOV_4 - 1, 20261103, 20261104), (MX_MIDNIGHT_NOV_4, 20261104, 20261103)] {
        env.warp(at);
        let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed_by(&borrower, b"text", other));
        env.expect_err(ix, &borrower, &[], E_SIGNED_DAY_MISMATCH);
        let draws = env.line(&k, line_id).draws;
        let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed_by(&borrower, b"text", today));
        env.send(ix, &borrower, &[]);
        assert_eq!(env.pagare(&k, line_id, draws).signed_on, today);
    }
}

/// The program keeps no index of signed messages: the same hash, signature and day pass again on
/// a retry the same day, and even on a second draw. Uniqueness is the app's and the expediente's.
#[test]
fn the_program_does_not_track_uniqueness_a_signed_message_reused_the_same_day_passes() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let signed = signed_by(&borrower, b"one text", mexico_city_date(env.now()));
    // A first attempt fails for another reason (more than the line has) and the retry passes.
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), 5 * UNIT, signed);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed);
    env.send(ix, &borrower, &[]);
    env.warp(env.now() + 60);
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed);
    env.send(ix, &borrower, &[]);
    assert_eq!((signed_fields(&env.pagare(&k, line_id, 0)), signed_fields(&env.pagare(&k, line_id, 1))), (signed, signed));
}

/// The registry's first root of a fresh pagaré depends on the signed hash: the same draw facts
/// with another signed hash would give another root.
#[test]
fn the_registry_root_of_a_fresh_pagare_changes_with_the_signed_hash_alone() {
    use elote_pool::state::KIND_EMISION;
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let signed = signed_by(&borrower, b"text A", mexico_city_date(env.now()));
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, signed);
    env.send(ix, &borrower, &[]);
    let now = env.now();
    let r = env.pagare(&k, line_id, 0);
    let root = |signed_hash: &[u8; 32]| {
        let issuance = reference::pagare::issuance_hash(&r.pagare_hash, signed_hash);
        let entry = reference::pagare::entry_hash(&r.asset.to_bytes(), KIND_EMISION, &k.pool.to_bytes(), &k.pool.to_bytes(), &issuance, now, 1);
        reference::pagare::genesis_root(&entry)
    };
    let mut other = signed.hash;
    other[31] ^= 1;
    assert_eq!(r.log_root, root(&signed.hash));
    assert_ne!(root(&other), root(&signed.hash));
    // Before decision 55 the entry stated the pagaré hash alone: not what the record holds now.
    let bare = reference::pagare::entry_hash(&r.asset.to_bytes(), KIND_EMISION, &k.pool.to_bytes(), &k.pool.to_bytes(), &r.pagare_hash, now, 1);
    assert_ne!(r.log_root, reference::pagare::genesis_root(&bare));
}

/// No instruction after `draw` rewrites the signed message: an Abono, a receipt annotation, a
/// procuración and its revocation, a court annotation, the payoff, the delivery and the owner's
/// burn leave the three fields as the draw wrote them (the quita: tests/quita.rs).
#[test]
fn no_instruction_after_the_draw_rewrites_the_signed_message() {
    use elote_pool::state::KIND_ANOTACION_JUDICIAL;
    let mut env = Env::new();
    let k = pool_with(&mut env, 20 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(2 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let s0 = signed_by(&borrower, b"pagare 0", mexico_city_date(env.now()));
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, s0);
    env.send(ix, &borrower, &[]);
    env.warp(env.now() + 3_600);
    let s1 = signed_by(&borrower, b"pagare 1", mexico_city_date(env.now()));
    let ix = env.draw_ix_with_signature(&k, line_id, &borrower.pubkey(), UNIT, s1);
    env.send(ix, &borrower, &[]);
    let check = |env: &Env, step: &str| {
        assert_eq!(signed_fields(&env.pagare(&k, line_id, 0)), s0, "pagaré 0 after {step}");
        assert_eq!(signed_fields(&env.pagare(&k, line_id, 1)), s1, "pagaré 1 after {step}");
    };

    env.warp(env.now() + 30 * 86_400);
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 10 * UNIT);
    let amount = env.interest_unpaid(&k, line_id) + 1;
    let ix = env.repay_ix_full(&k, line_id, &borrower.pubkey(), amount, [0; 32], &env.payment_sidecars(&k, line_id, amount));
    env.send(ix, &borrower, &[]);
    check(&env, "an Abono");
    let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), 2, RECEIPT_HASH);
    env.send(ix, &council, &[]);
    check(&env, "a receipt annotation");
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    check(&env, "a procuración");
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &council.pubkey(), [0x7e; 32]);
    env.send(ix, &council, &[]);
    check(&env, "its revocation");
    env.court_annotation(&k, line_id, 1, KIND_ANOTACION_JUDICIAL, [0x6a; 32]);
    check(&env, "a court annotation");
    let payoff = env.payoff(&k, line_id);
    env.repay(&k, line_id, &borrower, payoff);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    check(&env, "the payoff");
    for i in 0..2 {
        if env.pagare(&k, line_id, i).flags & PAGARE_FLAG_DELIVERED == 0 {
            env.deliver_pagare(&k, line_id, i);
        }
    }
    check(&env, "the delivery");
    let ix = env.burn_pagare_ix(&k, line_id, 0, &borrower.pubkey(), [0x7a; 32]);
    env.send(ix, &borrower, &[]);
    check(&env, "the owner's burn");
}
