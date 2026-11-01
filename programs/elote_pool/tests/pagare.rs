//! The pagaré token (decision 24): one Metaplex Core asset per disposición, minted by `draw`
//! into the series' collection, held frozen by the pool PDA, with a sidecar record. These tests
//! create through the mpl-core crate's builders and read back through its decoders against the
//! pinned program binary, which is the crate-vs-program compatibility proof.

mod common;

use common::*;
use elote_pool::events::PagareIssued;
use mpl_core::types::{ExternalPluginAdapterSchema, Key, UpdateAuthority};
use mpl_core::{AuthorityType, BaseAuthority};
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

    let asset = env.asset_state(&asset0);
    assert_eq!(asset.base.key, Key::AssetV1);
    assert_eq!(asset.base.owner, k.pool, "the pool PDA is the holder");
    // Core expresses the update authority of an asset in a collection through the collection,
    // and the collection's update authority is the pool PDA: the pool controls the asset.
    assert_eq!(asset.base.update_authority, UpdateAuthority::Collection(k.collection));
    assert_eq!(env.collection_state(&k.collection).update_authority, k.pool);
    assert_eq!(asset.base.name, format!("Pagare {line_id}-0"));
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

    // Permanent freeze delegate: frozen, authority = the pool PDA.
    let freeze = asset.plugin_list.permanent_freeze_delegate.as_ref().expect("PermanentFreezeDelegate");
    assert!(freeze.permanent_freeze_delegate.frozen);
    pool_authority(&freeze.base.authority);
    // Permanent transfer delegate: authority = the pool PDA.
    let transfer = asset.plugin_list.permanent_transfer_delegate.as_ref().expect("PermanentTransferDelegate");
    pool_authority(&transfer.base.authority);
    // Immutable metadata, nobody's.
    let immutable = asset.plugin_list.immutable_metadata.as_ref().expect("ImmutableMetadata");
    assert_eq!(immutable.base.authority.authority_type, AuthorityType::None);
    // Attributes, nobody's, with the seven facts and nothing personal.
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
        ]
    );
    // Nothing else was set.
    assert!(asset.plugin_list.royalties.is_none());
    assert!(asset.plugin_list.freeze_delegate.is_none());
    assert!(asset.plugin_list.transfer_delegate.is_none());
    assert!(asset.plugin_list.burn_delegate.is_none());
    assert!(asset.plugin_list.permanent_burn_delegate.is_none());
    assert!(asset.plugin_list.update_delegate.is_none());
    // One empty AppData section, binary, data authority = the pool PDA.
    let adapters = &asset.external_plugin_adapter_list;
    assert_eq!(adapters.app_data.len(), 1);
    let app = &adapters.app_data[0];
    assert_eq!(app.base.data_authority, mpl_core::types::PluginAuthority::Address { address: k.pool });
    assert_eq!(app.base.schema, ExternalPluginAdapterSchema::Binary);
    assert_eq!(app.data_len, 0);
    assert!(adapters.lifecycle_hooks.is_empty() && adapters.oracles.is_empty() && adapters.data_sections.is_empty());
}

/// The pagaré addresses are predictable once the line is open, so anyone can send lamports to
/// them ahead of the draw. Neither the sidecar's `init` nor the Core program's account creation
/// refuses a pre-funded address: the draw still goes through and nobody can brick a line that way.
#[test]
fn a_draw_survives_pre_funded_pagare_addresses() {
    let mut env = Env::new();
    let k = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let line_id = open(&mut env, &k, &borrower, &p);
    let line_key = line_pda(&k.pool, line_id).0;
    let asset0 = pagare_asset_pda(&line_key, 0).0;
    let sidecar0 = pagare_pda(&line_key, 0).0;
    // Below the rent-exempt minimum the runtime refuses the transfer itself, so the only funding
    // that lands is a rent-exempt one.
    env.svm.airdrop(&asset0, 1_000_000_000).unwrap();
    env.svm.airdrop(&sidecar0, 1_000_000_000).unwrap();
    let before = env.account(&borrower.pubkey()).lamports;

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
    env.send_with_attestor(ix, &borrower);
    // The stray lamports on the asset address went to the payer (here the borrower), who paid
    // the fee and the rent of both accounts out of them; the sidecar's were kept as its rent.
    assert!(env.account(&borrower.pubkey()).lamports > before + 990_000_000);
    let asset = env.asset_state(&asset0);
    assert_eq!((asset.base.key, asset.base.owner), (Key::AssetV1, k.pool));
    assert_eq!(env.pagare(&k, line_id, 0).pagare_hash, DOCUMENT_HASH);
    assert_eq!(env.line(&k, line_id).draws, 1);
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
        assert!(env.try_send(ix, &borrower, &[&attestor]).is_err());
    }
    // A collection that is not the pool's.
    let other = env.create_collection(&k.pool);
    let mut ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT, DOCUMENT_HASH);
    let i = ix.accounts.iter().position(|m| m.pubkey == k.collection).unwrap();
    ix.accounts[i].pubkey = other;
    env.expect_err(ix, &borrower, &[&attestor], E_WRONG_ACCOUNT);
    assert_eq!(env.line(&k, line_id).draws, 0);
    assert_eq!(env.collection_state(&k.collection).num_minted, 0);
}
