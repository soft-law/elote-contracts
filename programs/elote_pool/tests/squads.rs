//! The council through the real Squads v4 program: a 2-of-3 multisig whose vault signs only inside
//! `vault_transaction_execute` (Squads' `invoke_signed`), never as a signer of the outer
//! transaction. The vault transaction and an approved proposal are written into the SVM; the
//! execution is the binary's (`tests/fixtures/squads_v4-mainnet.so`, the mainnet deployment).
//! CPI depth: Squads (1), elote_pool (2), Metaplex Core (3), system (4).

mod common;

use common::*;
use elote_pool::errors::PoolError;
use litesvm::types::TransactionMetadata;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;

pub const SQUADS_FIXTURE: &str = "programs/elote_pool/tests/fixtures/squads_v4-mainnet.so";
/// sha256 of the mainnet deployment of `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` (immutable,
/// last deployed at slot 302582236), dumped 2026-11-04.
pub const SQUADS_SHA256: &str = "dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c";
const CEILING: u64 = 100_000;
const MARGIN: u64 = 5_000;

/// An env whose council multisig is executed by the real Squads binary.
fn squads_env() -> Env {
    let mut env = Env::new();
    let so = std::fs::read(workspace_root().join(SQUADS_FIXTURE)).expect("read the Squads fixture");
    let digest = <sha2::Sha256 as sha2::Digest>::digest(&so);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, SQUADS_SHA256, "{SQUADS_FIXTURE} is not the mainnet Squads v4 binary");
    env.svm.add_program(SQUADS_ID, &so).expect("load squads");
    env
}

fn disc(ns: &str, name: &str) -> [u8; 8] {
    let d = <sha2::Sha256 as sha2::Digest>::digest(format!("{ns}:{name}").as_bytes());
    d[..8].try_into().unwrap()
}

/// Runs `ix` as approved proposal `index` of the env's multisig, executed by curator 0, who pays.
/// The vault's signature comes from Squads; it is not among the outer transaction's signers.
#[allow(clippy::result_large_err)] // litesvm's error carries the whole transaction metadata
fn via_squads(env: &mut Env, index: u64, ix: Instruction) -> Result<TransactionMetadata, litesvm::types::FailedTransactionMetadata> {
    let multisig = env.multisig;
    let (vault, vault_bump) = squads_vault_pda(&multisig);
    let mut metas: Vec<(Pubkey, bool, bool)> = Vec::new();
    for m in ix.accounts.iter().map(|m| (m.pubkey, m.is_signer, m.is_writable)).chain([(ix.program_id, false, false)]) {
        match metas.iter_mut().find(|x| x.0 == m.0) {
            Some(x) => {
                x.1 |= m.1;
                x.2 |= m.2;
            }
            None => metas.push(m),
        }
    }
    metas.sort_by_key(|m| match (m.1, m.2) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    });
    let index_of = |k: &Pubkey| metas.iter().position(|m| m.0 == *k).unwrap() as u8;
    let count = |f: fn(&(Pubkey, bool, bool)) -> bool| metas.iter().filter(|m| f(m)).count() as u8;
    let mut msg = vec![count(|m| m.1), count(|m| m.1 && m.2), count(|m| !m.1 && m.2)];
    msg.extend_from_slice(&(metas.len() as u32).to_le_bytes());
    for m in &metas {
        msg.extend_from_slice(m.0.as_ref());
    }
    msg.extend_from_slice(&1u32.to_le_bytes());
    msg.push(index_of(&ix.program_id));
    msg.extend_from_slice(&(ix.accounts.len() as u32).to_le_bytes());
    for a in &ix.accounts {
        msg.push(index_of(&a.pubkey));
    }
    msg.extend_from_slice(&(ix.data.len() as u32).to_le_bytes());
    msg.extend_from_slice(&ix.data);
    msg.extend_from_slice(&0u32.to_le_bytes()); // no address lookup tables

    let member = env.curators[0].insecure_clone();
    let index_bytes = index.to_le_bytes();
    let (tx, tx_bump) = Pubkey::find_program_address(&[b"multisig", multisig.as_ref(), b"transaction", &index_bytes], &SQUADS_ID);
    let mut t = disc("account", "VaultTransaction").to_vec();
    t.extend_from_slice(multisig.as_ref());
    t.extend_from_slice(member.pubkey().as_ref()); // creator
    t.extend_from_slice(&index_bytes);
    t.extend_from_slice(&[tx_bump, 0, vault_bump]); // bump, vault_index, vault_bump
    t.extend_from_slice(&0u32.to_le_bytes()); // ephemeral signer bumps
    t.extend_from_slice(&msg);
    put_squads_account(&mut env.svm, &tx, t);
    let (proposal, p_bump) =
        Pubkey::find_program_address(&[b"multisig", multisig.as_ref(), b"transaction", &index_bytes, b"proposal"], &SQUADS_ID);
    let mut p = disc("account", "Proposal").to_vec();
    p.extend_from_slice(multisig.as_ref());
    p.extend_from_slice(&index_bytes);
    p.push(3); // ProposalStatus::Approved { timestamp }
    p.extend_from_slice(&(env.now() - 100).to_le_bytes());
    p.push(p_bump);
    p.extend_from_slice(&2u32.to_le_bytes()); // approved by curators 0 and 1
    p.extend_from_slice(env.curators[0].pubkey().as_ref());
    p.extend_from_slice(env.curators[1].pubkey().as_ref());
    p.extend_from_slice(&0u32.to_le_bytes()); // rejected
    p.extend_from_slice(&0u32.to_le_bytes()); // cancelled
    put_squads_account(&mut env.svm, &proposal, p);

    let mut accounts = vec![
        AccountMeta::new_readonly(multisig, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(tx, false),
        AccountMeta::new_readonly(member.pubkey(), true),
    ];
    for m in &metas {
        let signer = m.1 && m.0 != vault;
        accounts.push(if m.2 { AccountMeta::new(m.0, signer) } else { AccountMeta::new_readonly(m.0, signer) });
    }
    assert!(!accounts.iter().any(|a| a.pubkey == vault && a.is_signer), "the vault never signs the outer transaction");
    let exec = Instruction { program_id: SQUADS_ID, accounts, data: disc("global", "vault_transaction_execute").to_vec() };
    env.svm.airdrop(&member.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    env.try_send(exec, &member, &[])
}

fn ok(env: &mut Env, index: u64, ix: Instruction, what: &str) -> u64 {
    match via_squads(env, index, ix) {
        Ok(meta) => {
            let cu = meta.compute_units_consumed;
            println!("{what} through Squads: {cu} CU");
            assert!(cu + MARGIN <= CEILING, "{what} used {cu} CU, ceiling {CEILING} less {MARGIN}");
            cu
        }
        Err(e) => panic!("{what} failed: {:?}\n{}", e.err, e.meta.pretty_logs()),
    }
}

/// Squads and elote_pool number their errors from 6000 alike, so the refusal must be elote_pool's,
/// by name, surfacing through Squads.
fn refused(env: &mut Env, index: u64, ix: Instruction, e: PoolError) {
    let code = ERR_BASE + e as u32;
    let name = format!("{e:?}");
    match via_squads(env, index, ix) {
        Ok(meta) => panic!("expected {name}\n{}", meta.pretty_logs()),
        Err(f) => {
            let logs = f.meta.pretty_logs();
            assert_eq!(custom_error_code(&f.err), Some(code), "{logs}");
            assert!(logs.contains("programs/elote_pool/src") && logs.contains(&format!("Error Code: {name}.")), "{logs}");
        }
    }
}

fn pool_with_line(env: &mut Env) -> (PoolKeys, Keypair, u64, LineParams) {
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 120, 5_000);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    (k, borrower, id, p)
}

#[test]
fn the_council_approves_a_line_through_squads() {
    let mut env = squads_env();
    let (k, borrower, id, p) = pool_with_line(&mut env);
    let council = env.council.pubkey();
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council, id, p.expediente_root, hash);
    ok(&mut env, 1, ix, "approve_line");
    assert_eq!(env.line(&k, id).status, elote_pool::state::LineStatus::Approved);
}

#[test]
fn the_council_endorses_and_cancels_pagares_through_squads() {
    let mut env = squads_env();
    let (k, borrower, id, p) = pool_with_line(&mut env);
    env.approve_line(&k, id, &borrower.pubkey(), &p);
    env.draw_amount(&k, id, &borrower, UNIT);
    env.draw_amount(&k, id, &borrower, UNIT);
    let council = env.council.pubkey();

    // Endorsement en procuración of pagaré 1: Squads, elote_pool, Core and the system program.
    let procurador = Pubkey::new_unique();
    let ix = env.endorse_pagare_ix(&k, id, 1, &council, elote_pool::state::KIND_PROCURACION, procurador, ENDOSO_HASH);
    ok(&mut env, 1, ix, "endorse_pagare");
    assert_eq!(env.pagare(&k, id, 1).procurador, procurador);

    // Pagaré 0 paid by the borrower, then cancelled by the council.
    let line = env.line(&k, id);
    let first = env.pagare(&k, id, 0).amount_mxn + line.interest_due;
    let c = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &c, &borrower.pubkey(), first);
    env.repay(&k, id, &borrower, first);
    assert_eq!(env.pagare(&k, id, 0).status, elote_pool::state::PAGARE_PAID_PENDING_CANCEL);
    let ix = env.cancel_pagare_ix(&k, id, 0, &council, &council, RECEIPT_HASH);
    ok(&mut env, 2, ix, "cancel_pagare (council)");
    assert_eq!(env.pagare(&k, id, 0).status, elote_pool::state::PAGARE_PAID);
}

#[test]
fn a_stale_proposal_fails_through_squads() {
    let mut env = squads_env();
    let (k, borrower, id, p) = pool_with_line(&mut env);
    let council = env.council.pubkey();
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    env.valid_until = env.now() + 86_400;
    let ix = env.approve_line_ix(&k, &council, id, p.expediente_root, hash);
    env.valid_until = i64::MAX;
    env.warp(env.now() + 2 * 86_400);
    refused(&mut env, 1, ix, PoolError::ProposalExpired);
}

#[test]
fn a_multisig_lowered_below_a_majority_is_refused_on_the_next_council_instruction() {
    let mut env = squads_env();
    let (k, borrower, id, p) = pool_with_line(&mut env);
    let council = env.council.pubkey();
    // The council lowers its own threshold to 1 of 3 (as a Squads config transaction would).
    let mut account = env.account(&env.multisig.clone());
    account.data[72..74].copy_from_slice(&1u16.to_le_bytes());
    let multisig = env.multisig;
    env.svm.set_account(multisig, account).unwrap();
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    let ix = env.approve_line_ix(&k, &council, id, p.expediente_root, hash);
    refused(&mut env, 1, ix, PoolError::CouncilInvalid);
}

#[test]
fn the_council_proposes_its_successor_through_squads() {
    let mut env = squads_env();
    let (k, _) = env.setup_pool();
    let council = env.council.pubkey();
    let next = squads_vault_pda(&multisig_pda(&Pubkey::new_unique()).0).0;
    let ix = env.propose_council_ix(&k, &council, &next, elote_pool::state::COUNCIL_SQUADS);
    ok(&mut env, 1, ix, "propose_council");
    let pool = env.pool(&k);
    assert_eq!((pool.pending_council, pool.pending_council_kind), (next, elote_pool::state::COUNCIL_SQUADS));
}

#[test]
fn a_multisig_lowered_below_a_majority_cannot_propose_a_successor_through_squads() {
    let mut env = squads_env();
    let (k, _) = env.setup_pool();
    let council = env.council.pubkey();
    let mut account = env.account(&env.multisig.clone());
    account.data[72..74].copy_from_slice(&1u16.to_le_bytes());
    let multisig = env.multisig;
    env.svm.set_account(multisig, account).unwrap();
    let next = squads_vault_pda(&multisig_pda(&Pubkey::new_unique()).0).0;
    let ix = env.propose_council_ix(&k, &council, &next, elote_pool::state::COUNCIL_SQUADS);
    refused(&mut env, 1, ix, PoolError::CouncilInvalid);
    assert_eq!(env.pool(&k).pending_council, Pubkey::default());
}
