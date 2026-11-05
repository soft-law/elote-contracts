//! The council of curators: constants and states fixed by the design (PR 4).

use elote_pool::state::{
    LineStatus, COUNCIL_GENERIC, COUNCIL_SQUADS, MAX_COUNCIL_MEMBERS, MIN_ACCELERATION_NOTICE_SECONDS, MIN_CANCEL_GRACE_SECONDS,
    REQUEST_TTL_SECONDS, SQUADS_PROGRAM_ID,
};

#[test]
fn council_constants_match_the_design() {
    assert_eq!(MAX_COUNCIL_MEMBERS, 10);
    assert_eq!(REQUEST_TTL_SECONDS, 15 * 86_400);
    assert_eq!(MIN_ACCELERATION_NOTICE_SECONDS, 7 * 86_400);
    assert_eq!(MIN_CANCEL_GRACE_SECONDS, 30 * 86_400);
    assert_eq!((COUNCIL_SQUADS, COUNCIL_GENERIC), (0, 1));
    assert_eq!(SQUADS_PROGRAM_ID.to_string(), "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
}

#[test]
fn a_requested_line_comes_before_an_approved_one() {
    assert_eq!(LineStatus::Requested as u8, 0);
    assert_eq!(LineStatus::Approved as u8, 1);
}

mod common;

use common::*;
use elote_pool::errors::PoolError;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use anchor_spl::token_2022::spl_token_2022;

const E_COUNCIL_INVALID: u32 = ERR_BASE + PoolError::CouncilInvalid as u32;
const E_COUNCIL_TOO_LARGE: u32 = ERR_BASE + PoolError::CouncilTooLarge as u32;
/// Anchor's `AccountNotSigner`.
const E_ACCOUNT_NOT_SIGNER: u32 = 3010;

/// A multisig of `members` (all voters unless `masks` says otherwise) at a fresh address.
fn multisig_with(env: &mut Env, threshold: u16, masks: &[u8], config_authority: Pubkey, rent_collector: Option<Pubkey>) -> (Pubkey, u8) {
    let create_key = Pubkey::new_unique();
    let multisig = multisig_pda(&create_key).0;
    let members: Vec<(Pubkey, u8)> = masks.iter().map(|m| (Pubkey::new_unique(), *m)).collect();
    put_squads_account(&mut env.svm, &multisig, multisig_data(&create_key, &config_authority, threshold, rent_collector, &members));
    (multisig, squads_vault_pda(&multisig).1)
}

/// The keys and `init_pool` of a pool the env's creator pays for, naming `multisig`.
fn init_ix(env: &mut Env, multisig: &Pubkey) -> (PoolKeys, solana_instruction::Instruction) {
    let creator = env.creator.pubkey();
    let council = env.council.insecure_clone();
    let asset = env.create_mint(&council.pubkey(), 6);
    let k = env.pool_keys_by(&creator, &asset);
    let p = env.init_params(BPS, COLLECTION_PERIOD);
    (k, env.init_pool_ix_params(&k, multisig, p))
}

fn refused(masks: &[u8], threshold: u16, config_authority: Pubkey, code: u32) {
    let mut env = Env::new();
    let (multisig, _) = multisig_with(&mut env, threshold, masks, config_authority, None);
    let (_, ix) = init_ix(&mut env, &multisig);
    let creator = env.creator.insecure_clone();
    env.expect_err(ix, &creator, &[], code);
}

fn accepted(masks: &[u8], threshold: u16, rent_collector: Option<Pubkey>) {
    let mut env = Env::new();
    let (multisig, _) = multisig_with(&mut env, threshold, masks, Pubkey::default(), rent_collector);
    let (k, ix) = init_ix(&mut env, &multisig);
    let creator = env.creator.insecure_clone();
    env.send(ix, &creator, &[]);
    let pool = env.pool(&k);
    assert_eq!(pool.council, squads_vault_pda(&multisig).0);
}

#[test]
fn init_pool_makes_vault_0_of_the_multisig_the_council_and_the_creator_only_pays() {
    let mut env = Env::new();
    let multisig = env.multisig;
    let bump = env.council_vault_bump;
    let (k, ix) = init_ix(&mut env, &multisig);
    let creator = env.creator.insecure_clone();
    env.send(ix, &creator, &[]);
    let pool = env.pool(&k);
    assert_eq!(pool.council, env.council.pubkey());
    assert_eq!((pool.creator, pool.multisig, pool.council_vault_bump), (creator.pubkey(), multisig, bump));
    assert_eq!((pool.council_kind, pool.pending_council, pool.guardian), (COUNCIL_SQUADS, Pubkey::default(), Pubkey::default()));
    assert_eq!((pool.acceleration_notice_seconds, pool.cancel_grace_seconds), (ACCELERATION_NOTICE, CANCEL_GRACE));
    assert_eq!(k.pool, pool_pda(&creator.pubkey(), &k.asset_mint).0, "the creator seeds the pool");

    let ix = env.set_paused_ix(&k, &creator.pubkey(), true);
    env.expect_err(ix, &creator, &[], E_UNAUTHORIZED);
}

#[test]
fn init_pool_accepts_a_majority_of_voters() {
    accepted(&[PERM_ALL; 2], 2, None);
    accepted(&[PERM_ALL; 3], 2, None);
    accepted(&[PERM_ALL; 5], 3, Some(Pubkey::new_unique()));
    accepted(&[PERM_ALL; 10], 6, None);
    // Members who cannot vote do not count: two voters need two.
    accepted(&[PERM_ALL, PERM_ALL, PERM_INITIATE | PERM_EXECUTE], 2, None);
}

#[test]
fn init_pool_refuses_a_threshold_below_a_majority() {
    refused(&[PERM_ALL; 3], 1, Pubkey::default(), E_COUNCIL_INVALID);
    refused(&[PERM_ALL; 4], 2, Pubkey::default(), E_COUNCIL_INVALID);
    refused(&[PERM_ALL; 10], 5, Pubkey::default(), E_COUNCIL_INVALID);
    refused(&[PERM_INITIATE; 3], 1, Pubkey::default(), E_COUNCIL_INVALID);
}

#[test]
fn init_pool_refuses_a_multisig_with_a_config_authority() {
    refused(&[PERM_ALL; 3], 2, Pubkey::new_unique(), E_COUNCIL_INVALID);
}

#[test]
fn init_pool_refuses_more_than_ten_members() {
    refused(&[PERM_ALL; 11], 11, Pubkey::default(), E_COUNCIL_TOO_LARGE);
}

#[test]
fn init_pool_refuses_an_account_that_is_not_a_squads_multisig() {
    let mut env = Env::new();
    let multisig = env.multisig;
    let creator = env.creator.insecure_clone();

    let mut account = env.account(&multisig);
    account.owner = Pubkey::new_unique();
    env.svm.set_account(multisig, account.clone()).unwrap();
    let (_, ix) = init_ix(&mut env, &multisig);
    env.expect_err(ix, &creator, &[], E_COUNCIL_INVALID);

    account.owner = SQUADS_ID;
    account.data[0] ^= 1;
    env.svm.set_account(multisig, account).unwrap();
    let (_, ix) = init_ix(&mut env, &multisig);
    env.expect_err(ix, &creator, &[], E_COUNCIL_INVALID);
}

#[test]
fn init_pool_refuses_params_below_their_floors() {
    let mut env = Env::new();
    let multisig = env.multisig;
    let creator = env.creator.insecure_clone();
    type Edit = fn(&mut elote_pool::instructions::pool::InitPoolParams);
    let cases: [(Edit, u32); 3] = [
        (|p| p.acceleration_notice_seconds = ACCELERATION_NOTICE - 1, ERR_BASE + PoolError::AccelerationNoticeTooShort as u32),
        (|p| p.cancel_grace_seconds = CANCEL_GRACE - 1, ERR_BASE + PoolError::CancelGraceTooShort as u32),
        (|p| p.max_outstanding_principal = 0, E_WRONG_AMOUNT),
    ];
    for (edit, code) in cases {
        let council = env.council.pubkey();
        let asset = env.create_mint(&council, 6);
        let k = env.pool_keys_by(&creator.pubkey(), &asset);
        let mut p = env.init_params(BPS, COLLECTION_PERIOD);
        edit(&mut p);
        let ix = env.init_pool_ix_params(&k, &multisig, p);
        env.expect_err(ix, &creator, &[], code);
    }
}

#[test]
fn a_council_instruction_needs_the_vaults_signature() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let mut ix = env.set_paused_ix(&k, &env.council.pubkey(), true);
    for m in ix.accounts.iter_mut().filter(|m| m.pubkey == env.council.pubkey()) {
        m.is_signer = false;
    }
    let creator = env.creator.insecure_clone();
    env.expect_err(ix, &creator, &[], E_ACCOUNT_NOT_SIGNER);
}

/// A second 2-of-3 multisig and its vault, the council a rotation moves to.
fn second_council(env: &mut Env) -> (Pubkey, Keypair) {
    let (multisig, _) = multisig_with(env, 2, &[PERM_ALL; 3], Pubkey::default(), None);
    let vault = squads_vault_pda(&multisig).0;
    env.svm.airdrop(&vault, LAMPORTS_PER_SOL).unwrap();
    (multisig, Keypair::pda(vault))
}

#[test]
fn the_council_rotates_in_two_steps_to_a_verified_multisig() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let old = env.council.insecure_clone();
    let (multisig, new) = second_council(&mut env);

    let ix = env.accept_council_ix(&k, &new.pubkey(), &multisig);
    env.expect_err(ix, &new, &[], ERR_BASE + PoolError::NoPendingCouncil as u32);

    let ix = env.propose_council_ix(&k, &new.pubkey(), &new.pubkey(), COUNCIL_SQUADS);
    env.expect_err(ix, &new, &[], E_UNAUTHORIZED);
    let ix = env.propose_council_ix(&k, &old.pubkey(), &new.pubkey(), COUNCIL_SQUADS);
    env.send(ix, &old, &[]);
    let pool = env.pool(&k);
    assert_eq!((pool.pending_council, pool.pending_council_kind, pool.council), (new.pubkey(), COUNCIL_SQUADS, old.pubkey()));

    // Only the proposed key accepts, with its own multisig.
    let stranger = Keypair::new();
    let ix = env.accept_council_ix(&k, &stranger.pubkey(), &multisig);
    let payer = env.creator.insecure_clone();
    env.expect_err(ix, &payer, &[&stranger], ERR_BASE + PoolError::NoPendingCouncil as u32);
    let ix = env.accept_council_ix(&k, &new.pubkey(), &env.multisig.clone());
    env.expect_err(ix, &new, &[], E_COUNCIL_INVALID);

    let ix = env.accept_council_ix(&k, &new.pubkey(), &multisig);
    env.send(ix, &new, &[]);
    let pool = env.pool(&k);
    assert_eq!((pool.council, pool.multisig, pool.council_kind), (new.pubkey(), multisig, COUNCIL_SQUADS));
    assert_eq!(pool.council_vault_bump, squads_vault_pda(&multisig).1);
    assert_eq!(pool.pending_council, Pubkey::default());

    let ix = env.set_paused_ix(&k, &old.pubkey(), true);
    env.expect_err(ix, &old, &[], E_UNAUTHORIZED);
    let ix = env.set_paused_ix(&k, &new.pubkey(), true);
    env.send(ix, &new, &[]);
}

#[test]
fn accept_council_refuses_a_weak_multisig() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let old = env.council.insecure_clone();

    let (weak, _) = multisig_with(&mut env, 1, &[PERM_ALL; 3], Pubkey::default(), None);
    let weak_vault = Keypair::pda(squads_vault_pda(&weak).0);
    env.svm.airdrop(&weak_vault.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let ix = env.propose_council_ix(&k, &old.pubkey(), &weak_vault.pubkey(), COUNCIL_SQUADS);
    env.send(ix, &old, &[]);
    let ix = env.accept_council_ix(&k, &weak_vault.pubkey(), &weak);
    env.expect_err(ix, &weak_vault, &[], E_COUNCIL_INVALID);
}

#[test]
fn propose_council_takes_only_the_squads_kind() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let old = env.council.insecure_clone();
    let generic = env.funded(1);
    for kind in [COUNCIL_GENERIC, 2] {
        let ix = env.propose_council_ix(&k, &old.pubkey(), &generic.pubkey(), kind);
        env.expect_err(ix, &old, &[], ERR_BASE + PoolError::CouncilKindInvalid as u32);
    }
    // The zero key withdraws under the Squads kind only, too.
    let ix = env.propose_council_ix(&k, &old.pubkey(), &Pubkey::default(), COUNCIL_GENERIC);
    env.expect_err(ix, &old, &[], ERR_BASE + PoolError::CouncilKindInvalid as u32);
    let pool = env.pool(&k);
    assert_eq!((pool.pending_council, pool.pending_council_kind), (Pubkey::default(), 0));
}

#[test]
fn propose_council_runs_the_multisig_check() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let old = env.council.insecure_clone();
    let (_, new) = second_council(&mut env);
    // The council's own multisig at 1 of 3 voters, below a majority.
    let members: Vec<(Pubkey, u8)> = env.curators.iter().map(|c| (c.pubkey(), PERM_ALL)).collect();
    let multisig = env.multisig;
    put_squads_account(&mut env.svm, &multisig, multisig_data(&Pubkey::new_unique(), &Pubkey::default(), 1, None, &members));
    let ix = env.propose_council_ix(&k, &old.pubkey(), &new.pubkey(), COUNCIL_SQUADS);
    env.expect_err(ix, &old, &[], E_COUNCIL_INVALID);
    let ix = env.propose_council_ix(&k, &old.pubkey(), &Pubkey::default(), COUNCIL_SQUADS);
    env.expect_err(ix, &old, &[], E_COUNCIL_INVALID);
    assert_eq!(env.pool(&k).pending_council, Pubkey::default());
}

#[test]
fn proposing_the_zero_key_withdraws_a_rotation() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let old = env.council.insecure_clone();
    let (multisig, new) = second_council(&mut env);
    let ix = env.propose_council_ix(&k, &old.pubkey(), &new.pubkey(), COUNCIL_SQUADS);
    env.send(ix, &old, &[]);
    let ix = env.propose_council_ix(&k, &old.pubkey(), &Pubkey::default(), COUNCIL_SQUADS);
    env.send(ix, &old, &[]);
    assert_eq!(env.pool(&k).pending_council, Pubkey::default());
    let ix = env.accept_council_ix(&k, &new.pubkey(), &multisig);
    env.expect_err(ix, &new, &[], ERR_BASE + PoolError::NoPendingCouncil as u32);
}

#[test]
fn init_pool_refuses_the_share_mint_as_the_asset() {
    let mut env = Env::new();
    let council = env.council.pubkey();
    let creator = env.creator.insecure_clone();
    // A share mint that passes every share check, passed as the asset too. The pool PDA depends on
    // the asset, so the mint is created first and its authority handed to that pool afterwards.
    let council_kp = env.council.insecure_clone();
    let mint = env.create_share_mint(&council, 6, None, false);
    let pool = pool_pda(&creator.pubkey(), &mint).0;
    env.set_mint_authority(&mint, spl_token_2022::instruction::AuthorityType::MintTokens, &council_kp, Some(&pool));
    let share_mint = mint;
    let mut k = PoolKeys::new(&creator.pubkey(), &share_mint, &share_mint, TOKEN_2022_PROGRAM_ID, TOKEN_2022_PROGRAM_ID);
    k.collection = env.create_collection(&k.pool);
    let p = env.init_params(BPS, COLLECTION_PERIOD);
    let ix = env.init_pool_ix_params(&k, &env.multisig.clone(), p);
    env.expect_err(ix, &creator, &[], ERR_BASE + PoolError::AssetIsShareMint as u32);
}

/// A member pauses alone only through the Squads multisig the pool names: the same bytes under
/// another owner are not a multisig, so the member list is not read (`CouncilInvalid`). The
/// guardian and the council pause without reading it.
#[test]
fn a_member_pauses_only_through_a_squads_owned_multisig() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let curator = env.curators[0].insecure_clone();
    env.svm.airdrop(&curator.pubkey(), 1_000_000_000).unwrap();
    let multisig = env.multisig;
    let mut account = env.svm.get_account(&multisig).expect("the multisig");
    account.owner = Pubkey::new_unique();
    env.svm.set_account(multisig, account).unwrap();
    let ix = env.pause_ix(&k, &curator.pubkey());
    env.expect_err(ix, &curator, &[], E_COUNCIL_INVALID);
    assert!(!env.pool(&k).paused);
    let council = env.council.insecure_clone();
    let ix = env.pause_ix(&k, &council.pubkey());
    env.send(ix, &council, &[]);
    assert!(env.pool(&k).paused);
}
