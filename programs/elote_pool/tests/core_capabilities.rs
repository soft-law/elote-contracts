//! Step 6d prework: what Metaplex Core (the pinned `release/core@0.15.3` binary, through the
//! `mpl-core` 0.12.1 crate) lets the parties do with a pagaré asset after its creation. Condition
//! C9 of the legal brief: after an endoso en propiedad the pool, the admin and the attestor must
//! hold no power to move, freeze or burn the asset. These tests pin, as Core behaves today, whether
//! the permanent delegates' authorities can be handed off or extinguished, who can thaw, and whether
//! the asset's update authority can leave the collection. They drive Core alone, with keypairs
//! standing in for the pool PDA (the delegate and the collection's update authority are the same
//! PDA in production; two keys here so each power is seen on its own) and the shape of the asset
//! `draw` mints. A Core upgrade that changes any answer fails here first.
//!
//! Core error codes seen: 9 `InvalidAuthority` (also what a transfer rejected by the freeze
//! surfaces as), 26 `NoApprovals`, 27 `CannotRedelegate` ("Plugin Manager cannot redelegate a
//! delegated plugin without revoking first").

mod common;

use common::*;
use mpl_core::instructions::{
    AddPluginV1Builder, ApprovePluginAuthorityV1Builder, CreateV2Builder, RevokePluginAuthorityV1Builder,
    TransferV1Builder, UpdatePluginV1Builder, UpdateV1Builder, UpdateV2Builder,
};
use mpl_core::types::{
    AddBlocker, Attribute, Attributes, ImmutableMetadata, PermanentBurnDelegate, PermanentFreezeDelegate,
    PermanentTransferDelegate, Plugin, PluginAuthority, PluginAuthorityPair, PluginType, TransferDelegate, UpdateAuthority,
    UpdateDelegate,
};
use mpl_core::{AuthorityType, BaseAuthority};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

const CORE_INVALID_AUTHORITY: u32 = 9;
const CORE_NO_APPROVALS: u32 = 26;
const CORE_CANNOT_REDELEGATE: u32 = 27;

const PERMANENT: [PluginType; 3] =
    [PluginType::PermanentFreezeDelegate, PluginType::PermanentTransferDelegate, PluginType::PermanentBurnDelegate];

/// The parties: the collection's update authority (the pool PDA in production), the permanent
/// delegate (also the pool PDA in production), the asset's owner (the pool at issuance, the
/// endorsee after an endoso), and a stranger.
struct Parties {
    ua: Keypair,
    delegate: Keypair,
    owner: Keypair,
    stranger: Keypair,
    collection: Pubkey,
    asset: Pubkey,
}

/// An asset shaped like a pagaré: in `collection`, owned by `owner`, frozen, the three permanent
/// delegates held by `delegate`, immutable metadata, attributes nobody can change; with
/// `add_blocker`, an `AddBlocker` plugin too (not part of `draw` today).
fn pagare_like(env: &mut Env, add_blocker: bool) -> Parties {
    let ua = env.funded(1);
    let delegate = env.funded(1);
    let owner = env.funded(1);
    let stranger = env.funded(1);
    let collection = env.create_collection(&ua.pubkey());
    let asset = Keypair::new();
    let by_delegate = Some(PluginAuthority::Address { address: delegate.pubkey() });
    let nobody = Some(PluginAuthority::None);
    let mut plugins = vec![
        PluginAuthorityPair {
            plugin: Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true }),
            authority: by_delegate.clone(),
        },
        PluginAuthorityPair { plugin: Plugin::PermanentTransferDelegate(PermanentTransferDelegate {}), authority: by_delegate.clone() },
        PluginAuthorityPair { plugin: Plugin::PermanentBurnDelegate(PermanentBurnDelegate {}), authority: by_delegate },
        PluginAuthorityPair { plugin: Plugin::ImmutableMetadata(ImmutableMetadata {}), authority: nobody.clone() },
        PluginAuthorityPair {
            plugin: Plugin::Attributes(Attributes { attribute_list: vec![Attribute { key: "amount".into(), value: "1".into() }] }),
            authority: nobody.clone(),
        },
    ];
    if add_blocker {
        plugins.push(PluginAuthorityPair { plugin: Plugin::AddBlocker(AddBlocker {}), authority: nobody });
    }
    let ix = CreateV2Builder::new()
        .asset(asset.pubkey())
        .collection(Some(collection))
        .authority(Some(ua.pubkey()))
        .payer(ua.pubkey())
        .owner(Some(owner.pubkey()))
        .name("PAGARE 0-0".into())
        .uri(String::new())
        .plugins(plugins)
        .instruction();
    env.send(ix, &ua, &[&asset]);
    Parties { ua, delegate, owner, stranger, collection, asset: asset.pubkey() }
}

/// Sends `ix` signed by `signer` and returns `Ok(())` or the Core error code.
fn attempt(env: &mut Env, ix: Instruction, signer: &Keypair) -> Result<(), u32> {
    env.try_send(ix, signer, &[]).map(|_| ()).map_err(|e| custom_error_code(&e.err).unwrap_or(u32::MAX))
}

fn approve_ix(p: &Parties, by: &Keypair, plugin: &PluginType, to: PluginAuthority) -> Instruction {
    ApprovePluginAuthorityV1Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(by.pubkey())
        .authority(Some(by.pubkey()))
        .plugin_type(plugin.clone())
        .new_authority(to)
        .instruction()
}

fn revoke_ix(p: &Parties, by: &Keypair, plugin: &PluginType) -> Instruction {
    RevokePluginAuthorityV1Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(by.pubkey())
        .authority(Some(by.pubkey()))
        .plugin_type(plugin.clone())
        .instruction()
}

fn set_frozen_ix(p: &Parties, by: &Keypair, frozen: bool) -> Instruction {
    UpdatePluginV1Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(by.pubkey())
        .authority(Some(by.pubkey()))
        .plugin(Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen }))
        .instruction()
}

fn transfer_ix(p: &Parties, by: &Keypair, to: &Pubkey) -> Instruction {
    TransferV1Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(by.pubkey())
        .authority(Some(by.pubkey()))
        .new_owner(*to)
        .instruction()
}

fn add_plugin_ix(p: &Parties, by: &Keypair, plugin: Plugin) -> Instruction {
    AddPluginV1Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(by.pubkey())
        .authority(Some(by.pubkey()))
        .plugin(plugin)
        .init_authority(PluginAuthority::Address { address: by.pubkey() })
        .instruction()
}

fn authority_of(env: &Env, p: &Parties, plugin: &PluginType) -> BaseAuthority {
    let asset = env.asset_state(&p.asset);
    let list = &asset.plugin_list;
    match plugin {
        PluginType::PermanentFreezeDelegate => list.permanent_freeze_delegate.as_ref().unwrap().base.authority,
        PluginType::PermanentTransferDelegate => list.permanent_transfer_delegate.as_ref().unwrap().base.authority,
        PluginType::PermanentBurnDelegate => list.permanent_burn_delegate.as_ref().unwrap().base.authority,
        _ => unreachable!(),
    }
}

fn frozen(env: &Env, p: &Parties) -> bool {
    env.asset_state(&p.asset).plugin_list.permanent_freeze_delegate.as_ref().unwrap().permanent_freeze_delegate.frozen
}

fn held_by(address: Pubkey) -> BaseAuthority {
    BaseAuthority { authority_type: AuthorityType::Address, address: Some(address) }
}

const BY_UPDATE_AUTHORITY: BaseAuthority = BaseAuthority { authority_type: AuthorityType::UpdateAuthority, address: None };
const BY_NOBODY: BaseAuthority = BaseAuthority { authority_type: AuthorityType::None, address: None };

/// Question 1 (C9): can the authority of a permanent delegate be reassigned after creation, and by
/// whom? Answer: not directly. `ApprovePluginAuthorityV1` on a delegated permanent plugin is
/// refused for everyone, the delegate and the collection's update authority included
/// (`CannotRedelegate`). `RevokePluginAuthorityV1` by the delegate or by the collection's update
/// authority resets the plugin to `UpdateAuthority` (the collection, so the pool PDA in
/// production); from that state only the collection's update authority can approve a new address
/// or `None`, and `None` is terminal: no approve, no revoke after it. A handoff is therefore a
/// two-step act of the pool (revoke, then approve as the collection's authority), and extinction
/// of a power is irreversible.
#[test]
fn permanent_delegate_authority_is_handed_off_only_by_revoke_then_approve_from_the_collection_authority() {
    let mut env = Env::new();
    let p = pagare_like(&mut env, false);
    let holder = Pubkey::new_unique();
    for plugin in &PERMANENT {
        assert_eq!(authority_of(&env, &p, plugin), held_by(p.delegate.pubkey()));
        // Delegated: nobody can redelegate, not even the delegate or the collection's authority.
        for by in [&p.stranger, &p.owner, &p.ua, &p.delegate] {
            let to = PluginAuthority::Address { address: holder };
            assert_eq!(attempt(&mut env, approve_ix(&p, by, plugin, to), by), Err(CORE_CANNOT_REDELEGATE));
        }
        assert_eq!(authority_of(&env, &p, plugin), held_by(p.delegate.pubkey()));
        // Revoke: not a stranger or the owner; the delegate itself can, back to the update authority.
        assert_eq!(attempt(&mut env, revoke_ix(&p, &p.stranger, plugin), &p.stranger), Err(CORE_NO_APPROVALS));
        assert_eq!(attempt(&mut env, revoke_ix(&p, &p.owner, plugin), &p.owner), Err(CORE_NO_APPROVALS));
        assert_eq!(attempt(&mut env, revoke_ix(&p, &p.delegate, plugin), &p.delegate), Ok(()));
        assert_eq!(authority_of(&env, &p, plugin), BY_UPDATE_AUTHORITY);
        // From there the former delegate is nobody; the collection's authority hands the power to
        // a key, can take it back, and can extinguish it.
        let to = PluginAuthority::Address { address: holder };
        assert_eq!(attempt(&mut env, approve_ix(&p, &p.delegate, plugin, to.clone()), &p.delegate), Err(CORE_NO_APPROVALS));
        assert_eq!(attempt(&mut env, approve_ix(&p, &p.ua, plugin, to), &p.ua), Ok(()));
        assert_eq!(authority_of(&env, &p, plugin), held_by(holder));
        assert_eq!(attempt(&mut env, revoke_ix(&p, &p.ua, plugin), &p.ua), Ok(()), "the collection's authority revokes a delegation it made");
        assert_eq!(authority_of(&env, &p, plugin), BY_UPDATE_AUTHORITY);
        assert_eq!(attempt(&mut env, approve_ix(&p, &p.ua, plugin, PluginAuthority::None), &p.ua), Ok(()));
        assert_eq!(authority_of(&env, &p, plugin), BY_NOBODY);
        // None is terminal.
        let to = PluginAuthority::Address { address: holder };
        assert_eq!(attempt(&mut env, approve_ix(&p, &p.ua, plugin, to), &p.ua), Err(CORE_CANNOT_REDELEGATE));
        assert_eq!(attempt(&mut env, revoke_ix(&p, &p.ua, plugin), &p.ua), Err(CORE_INVALID_AUTHORITY));
        assert_eq!(authority_of(&env, &p, plugin), BY_NOBODY);
    }
}

/// Question 2 (C9): who can thaw or re-freeze through the permanent freeze delegate? Only its
/// authority; the collection's update authority, the owner and a stranger get `NoApprovals`.
/// Consequence for the endoso: once the freeze delegate's authority is `None`, nobody can thaw, so
/// a frozen asset handed off that way is immovable forever (the owner's transfer is refused, which
/// Core reports as `InvalidAuthority`); thawed first, the asset becomes an ordinary one its owner transfers freely.
#[test]
fn only_the_permanent_freeze_delegate_thaws_and_a_frozen_asset_with_no_delegate_is_stuck() {
    let mut env = Env::new();
    // The right order: thaw, then give up the delegate, then the owner moves it.
    let p = pagare_like(&mut env, false);
    for by in [&p.stranger, &p.owner, &p.ua] {
        assert_eq!(attempt(&mut env, set_frozen_ix(&p, by, false), by), Err(CORE_NO_APPROVALS));
        assert!(frozen(&env, &p));
    }
    assert_eq!(attempt(&mut env, transfer_ix(&p, &p.owner, &p.stranger.pubkey()), &p.owner), Err(CORE_INVALID_AUTHORITY));
    assert_eq!(attempt(&mut env, set_frozen_ix(&p, &p.delegate, false), &p.delegate), Ok(()));
    assert!(!frozen(&env, &p));
    assert_eq!(attempt(&mut env, set_frozen_ix(&p, &p.delegate, true), &p.delegate), Ok(()), "and it can re-freeze while it holds the power");
    assert_eq!(attempt(&mut env, set_frozen_ix(&p, &p.delegate, false), &p.delegate), Ok(()));
    assert_eq!(attempt(&mut env, revoke_ix(&p, &p.delegate, &PluginType::PermanentFreezeDelegate), &p.delegate), Ok(()));
    assert_eq!(attempt(&mut env, approve_ix(&p, &p.ua, &PluginType::PermanentFreezeDelegate, PluginAuthority::None), &p.ua), Ok(()));
    assert_eq!(attempt(&mut env, set_frozen_ix(&p, &p.ua, true), &p.ua), Err(CORE_NO_APPROVALS), "nobody can freeze it again");
    assert_eq!(attempt(&mut env, set_frozen_ix(&p, &p.delegate, true), &p.delegate), Err(CORE_NO_APPROVALS));
    assert_eq!(attempt(&mut env, transfer_ix(&p, &p.owner, &p.stranger.pubkey()), &p.owner), Ok(()));
    assert_eq!(env.asset_state(&p.asset).base.owner, p.stranger.pubkey(), "an ordinary asset now");

    // The wrong order: give up the delegate while frozen, and the asset is stuck for good.
    let q = pagare_like(&mut env, false);
    assert_eq!(attempt(&mut env, revoke_ix(&q, &q.delegate, &PluginType::PermanentFreezeDelegate), &q.delegate), Ok(()));
    assert_eq!(attempt(&mut env, approve_ix(&q, &q.ua, &PluginType::PermanentFreezeDelegate, PluginAuthority::None), &q.ua), Ok(()));
    for by in [&q.stranger, &q.owner, &q.ua, &q.delegate] {
        assert_eq!(attempt(&mut env, set_frozen_ix(&q, by, false), by), Err(CORE_NO_APPROVALS));
    }
    assert!(frozen(&env, &q));
    assert_eq!(attempt(&mut env, transfer_ix(&q, &q.owner, &q.stranger.pubkey()), &q.owner), Err(CORE_INVALID_AUTHORITY));
}

/// Question 3 (C9): can the update authority of an asset inside a collection be moved off the
/// collection? No: `UpdateV1` to an address or to `None` and `UpdateV2` to an address are refused
/// for everyone, the collection's own authority included (`InvalidAuthority`); the asset stays in
/// the collection and the pool PDA stays its update authority through it. What that authority can
/// still do to the asset afterwards: not add a permanent plugin (creation only, `InvalidAuthority`),
/// not add an owner-managed plugin (`NoApprovals`), but add an authority-managed plugin such as
/// `UpdateDelegate`, unless an `AddBlocker` was set at creation, which closes that door too.
#[test]
fn an_assets_update_authority_cannot_leave_its_collection_and_add_blocker_closes_the_rest() {
    let mut env = Env::new();
    let p = pagare_like(&mut env, false);
    let holder = Pubkey::new_unique();
    let v1 = |by: &Keypair, to: UpdateAuthority| {
        UpdateV1Builder::new()
            .asset(p.asset)
            .collection(Some(p.collection))
            .payer(by.pubkey())
            .authority(Some(by.pubkey()))
            .new_update_authority(to)
            .instruction()
    };
    for by in [&p.stranger, &p.owner, &p.delegate, &p.ua] {
        assert_eq!(attempt(&mut env, v1(by, UpdateAuthority::Address(holder)), by), Err(CORE_INVALID_AUTHORITY));
        assert_eq!(attempt(&mut env, v1(by, UpdateAuthority::None), by), Err(CORE_INVALID_AUTHORITY));
    }
    let v2 = UpdateV2Builder::new()
        .asset(p.asset)
        .collection(Some(p.collection))
        .payer(p.ua.pubkey())
        .authority(Some(p.ua.pubkey()))
        .new_update_authority(UpdateAuthority::Address(holder))
        .instruction();
    assert_eq!(attempt(&mut env, v2, &p.ua), Err(CORE_INVALID_AUTHORITY));
    assert_eq!(env.asset_state(&p.asset).base.update_authority, UpdateAuthority::Collection(p.collection));
    assert_eq!(env.collection_state(&p.collection).current_size, 1);

    // What the collection's authority can still add after creation.
    let permanent = Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true });
    assert_eq!(attempt(&mut env, add_plugin_ix(&p, &p.ua, permanent), &p.ua), Err(CORE_INVALID_AUTHORITY));
    let owner_managed = Plugin::TransferDelegate(TransferDelegate {});
    assert_eq!(attempt(&mut env, add_plugin_ix(&p, &p.ua, owner_managed), &p.ua), Err(CORE_NO_APPROVALS));
    let authority_managed = || Plugin::UpdateDelegate(UpdateDelegate { additional_delegates: vec![] });
    assert_eq!(attempt(&mut env, add_plugin_ix(&p, &p.ua, authority_managed()), &p.ua), Ok(()));
    assert!(env.asset_state(&p.asset).plugin_list.update_delegate.is_some());

    // With an AddBlocker from creation, not even that.
    let b = pagare_like(&mut env, true);
    let r = attempt(&mut env, add_plugin_ix(&b, &b.ua, authority_managed()), &b.ua);
    assert!(r.is_err(), "AddBlocker refuses the update authority's plugin");
    assert!(env.asset_state(&b.asset).plugin_list.update_delegate.is_none());
}
