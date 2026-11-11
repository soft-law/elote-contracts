//! Decision 56, the cotejo keys: while the pool names any, a draw is co-signed by one of them, and
//! the pagaré records which; only the council sets them. Decision 57, the sealed copy's hash: stored
//! as given, never rewritten. The rule is checked against `tests/reference` (`cotejo::co_signer`).

mod common;
mod reference;

use anchor_lang::InstructionData;
use common::*;
use elote_pool::events::{CotejoKeysSet, PagareIssued};
use elote_pool::instruction as ix;
use elote_pool::state::KIND_ANOTACION_JUDICIAL;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

const ZERO: Pubkey = Pubkey::new_from_array([0; 32]);

/// A draw's optional `cotejo` account: its key and whether it signs.
type Cotejo = Option<(Pubkey, bool)>;

struct Setup {
    env: Env,
    k: PoolKeys,
    borrower: Keypair,
    line_id: u64,
}

/// A funded pool and an approved line of 8 units, 6 installments.
fn setup() -> Setup {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 40 * UNIT);
    env.deposit(&k, &investor, 40 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(8 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    Setup { env, k, borrower, line_id }
}

/// A draw of one unit with `cotejo` in the optional slot, signing or not.
fn draw_with(s: &Setup, cotejo: Option<(Pubkey, bool)>) -> solana_instruction::Instruction {
    let ix = s.env.draw_ix_amount(&s.k, s.line_id, &s.borrower.pubkey(), UNIT);
    with_cotejo(ix, cotejo)
}

/// What a refused draw must leave alone.
fn snapshot(s: &Setup) -> Vec<Option<solana_account::Account>> {
    let line = line_pda(&s.k.pool, s.line_id).0;
    let draws = s.env.line(&s.k, s.line_id).draws;
    [s.k.pool, line, s.k.vault, s.k.asset_ata(&s.borrower.pubkey()), pagare_pda(&line, draws).0, pagare_asset_pda(&line, draws).0, s.k.collection]
        .iter()
        .map(|a| s.env.svm.get_account(a))
        .collect()
}

fn keys_bytes(keys: &[Pubkey; 3]) -> [[u8; 32]; 3] {
    [keys[0].to_bytes(), keys[1].to_bytes(), keys[2].to_bytes()]
}

#[test]
fn without_cotejo_keys_a_draw_ignores_the_cotejo_account() {
    let mut s = setup();
    assert_eq!(s.env.pool(&s.k).cotejo_keys, [ZERO; 3]);
    let stranger = s.env.funded(1);
    let b = s.borrower.insecure_clone();
    let cases: [(Cotejo, &[&Keypair]); 3] =
        [(None, &[]), (Some((Pubkey::new_unique(), false)), &[]), (Some((stranger.pubkey(), true)), &[&stranger])];
    for (i, (cotejo, extra)) in cases.into_iter().enumerate() {
        let ix = draw_with(&s, cotejo);
        let meta = s.env.send(ix, &b, extra);
        let issued: PagareIssued = find_event(&meta).expect("PagareIssued");
        let record = s.env.pagare(&s.k, s.line_id, i as u16);
        assert_eq!((record.cotejo, issued.cotejo), (ZERO, ZERO), "no key required, none recorded (case {i})");
        let keys = keys_bytes(&s.env.pool(&s.k).cotejo_keys);
        assert_eq!(reference::cotejo::co_signer(&keys, cotejo.map(|(k, sig)| (k.to_bytes(), sig))), Ok([0; 32]));
    }
}

#[test]
fn with_cotejo_keys_a_draw_without_a_valid_cotejo_is_refused_and_writes_nothing() {
    let mut s = setup();
    let (a, b) = (Keypair::new(), Keypair::new());
    let keys = [a.pubkey(), ZERO, b.pubkey()];
    s.env.set_cotejo_keys(&s.k, keys);
    assert_eq!(s.env.pool(&s.k).cotejo_keys, keys);
    let stranger = Keypair::new();
    let borrower = s.borrower.insecure_clone();
    let cases: Vec<(&str, Cotejo, Vec<&Keypair>)> = vec![
        ("no cotejo account", None, vec![]),
        ("a key of the set that does not sign", Some((a.pubkey(), false)), vec![]),
        ("a signer outside the set", Some((stranger.pubkey(), true)), vec![&stranger]),
        ("the borrower as its own cotejo", Some((borrower.pubkey(), true)), vec![]),
        ("the pool", Some((s.k.pool, false)), vec![]),
    ];
    for (what, cotejo, extra) in cases {
        assert_eq!(
            reference::cotejo::co_signer(&keys_bytes(&keys), cotejo.map(|(k, sig)| (k.to_bytes(), sig))),
            Err(()),
            "the reference refuses {what}"
        );
        let before = snapshot(&s);
        let ix = draw_with(&s, cotejo);
        s.env.expect_err(ix, &borrower, &extra, E_COTEJO_REQUIRED);
        assert_eq!(snapshot(&s), before, "{what}: nothing written");
    }
    // A key of the set, signing, draws.
    let ix = draw_with(&s, Some((b.pubkey(), true)));
    s.env.send(ix, &borrower, &[&b]);
    assert_eq!(s.env.pagare(&s.k, s.line_id, 0).cotejo, b.pubkey());
}

#[test]
fn each_of_the_three_keys_co_signs_and_is_recorded() {
    let mut s = setup();
    let cotejos = [Keypair::new(), Keypair::new(), Keypair::new()];
    let keys = [cotejos[0].pubkey(), cotejos[1].pubkey(), cotejos[2].pubkey()];
    let ev: CotejoKeysSet = find_event(&s.env.set_cotejo_keys(&s.k, keys)).expect("CotejoKeysSet");
    assert_eq!((ev.pool, ev.keys), (s.k.pool, keys));
    let borrower = s.borrower.insecure_clone();
    for (i, c) in cotejos.iter().enumerate() {
        let ix = draw_with(&s, Some((c.pubkey(), true)));
        assert!(!ix.accounts[1].is_writable, "the cotejo account is read only");
        s.env.svm.airdrop(&c.pubkey(), LAMPORTS_PER_SOL).unwrap();
        let payer_before = s.env.account(&c.pubkey()).lamports;
        let meta = s.env.send(ix, &borrower, &[c]);
        let issued: PagareIssued = find_event(&meta).expect("PagareIssued");
        let record = s.env.pagare(&s.k, s.line_id, i as u16);
        assert_eq!((record.cotejo, issued.cotejo), (c.pubkey(), c.pubkey()));
        assert_eq!(
            reference::cotejo::co_signer(&keys_bytes(&keys), Some((c.pubkey().to_bytes(), true))),
            Ok(c.pubkey().to_bytes())
        );
        assert_eq!(s.env.account(&c.pubkey()).lamports, payer_before, "the cotejo pays nothing");
    }
}

/// The zero key marks an empty slot. Even a transaction that makes the zero key "sign" (the harness
/// runs without signature checks) never satisfies the rule, whatever the empty slots.
#[test]
fn the_zero_key_never_satisfies_the_rule() {
    let mut s = setup();
    let a = Keypair::new();
    s.env.set_cotejo_keys(&s.k, [ZERO, a.pubkey(), ZERO]);
    let zero = Keypair::pda(ZERO);
    let borrower = s.borrower.insecure_clone();
    let before = snapshot(&s);
    let ix = draw_with(&s, Some((ZERO, true)));
    s.env.expect_err(ix, &borrower, &[&zero], E_COTEJO_REQUIRED);
    assert_eq!(snapshot(&s), before);
    assert_eq!(reference::cotejo::co_signer(&[[0; 32], a.pubkey().to_bytes(), [0; 32]], Some(([0; 32], true))), Err(()));
}

#[test]
fn rotation_refuses_the_old_key_and_takes_the_new_one() {
    let mut s = setup();
    let (old, new) = (Keypair::new(), Keypair::new());
    s.env.set_cotejo_keys(&s.k, [old.pubkey(), ZERO, ZERO]);
    let borrower = s.borrower.insecure_clone();
    let ix = draw_with(&s, Some((old.pubkey(), true)));
    s.env.send(ix, &borrower, &[&old]);
    s.env.set_cotejo_keys(&s.k, [ZERO, ZERO, new.pubkey()]);
    let ix = draw_with(&s, Some((old.pubkey(), true)));
    s.env.expect_err(ix, &borrower, &[&old], E_COTEJO_REQUIRED);
    let ix = draw_with(&s, Some((new.pubkey(), true)));
    s.env.send(ix, &borrower, &[&new]);
    assert_eq!((s.env.pagare(&s.k, s.line_id, 0).cotejo, s.env.pagare(&s.k, s.line_id, 1).cotejo), (old.pubkey(), new.pubkey()));
    // All zero turns the requirement off.
    s.env.set_cotejo_keys(&s.k, [ZERO; 3]);
    let ix = draw_with(&s, None);
    s.env.send(ix, &borrower, &[]);
    assert_eq!(s.env.pagare(&s.k, s.line_id, 2).cotejo, ZERO);
}

#[test]
fn only_the_council_sets_cotejo_keys() {
    let mut s = setup();
    let a = Keypair::new();
    s.env.set_cotejo_keys(&s.k, [a.pubkey(), ZERO, ZERO]);
    let creator = s.env.creator.insecure_clone();
    let borrower = s.borrower.insecure_clone();
    let keys = [Pubkey::new_unique(), ZERO, ZERO];
    s.env.svm.airdrop(&a.pubkey(), LAMPORTS_PER_SOL).unwrap();
    for signer in [&borrower, &creator, &a] {
        let ix = s.env.set_cotejo_keys_ix(&s.k, &signer.pubkey(), keys);
        s.env.expect_err(ix, signer, &[], E_UNAUTHORIZED);
    }
    // A curator alone is not the council either.
    let curator = s.env.curators[0].insecure_clone();
    s.env.svm.airdrop(&curator.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let ix = s.env.set_cotejo_keys_ix(&s.k, &curator.pubkey(), keys);
    s.env.expect_err(ix, &curator, &[], E_UNAUTHORIZED);
    assert_eq!(s.env.pool(&s.k).cotejo_keys, [a.pubkey(), ZERO, ZERO]);
}

#[test]
fn repeated_or_forbidden_keys_are_refused() {
    let mut s = setup();
    let a = Pubkey::new_unique();
    let council = s.env.council.pubkey();
    let pool = s.k.pool;
    let cases = [[a, a, ZERO], [a, ZERO, a], [pool, ZERO, ZERO], [ZERO, council, ZERO], [ZERO, ZERO, PROGRAM_ID], [a, a, a]];
    for keys in cases {
        assert!(!reference::cotejo::keys_valid(&keys_bytes(&keys), pool.to_bytes(), council.to_bytes(), PROGRAM_ID.to_bytes()));
        let c = s.env.council.insecure_clone();
        let ix = s.env.set_cotejo_keys_ix(&s.k, &council, keys);
        s.env.expect_err(ix, &c, &[], E_COTEJO_KEY_INVALID);
    }
    assert_eq!(s.env.pool(&s.k).cotejo_keys, [ZERO; 3]);
    // Empty slots may repeat; all zero is allowed.
    for keys in [[a, ZERO, ZERO], [ZERO; 3]] {
        assert!(reference::cotejo::keys_valid(&keys_bytes(&keys), pool.to_bytes(), council.to_bytes(), PROGRAM_ID.to_bytes()));
        s.env.set_cotejo_keys(&s.k, keys);
        assert_eq!(s.env.pool(&s.k).cotejo_keys, keys);
    }
}

#[test]
fn the_council_sets_cotejo_keys_while_paused() {
    let mut s = setup();
    s.env.set_paused(&s.k, true);
    let a = Keypair::new();
    s.env.set_cotejo_keys(&s.k, [a.pubkey(), ZERO, ZERO]);
    assert_eq!(s.env.pool(&s.k).cotejo_keys, [a.pubkey(), ZERO, ZERO]);
    s.env.set_paused(&s.k, false);
    let borrower = s.borrower.insecure_clone();
    let ix = draw_with(&s, Some((a.pubkey(), true)));
    s.env.send(ix, &borrower, &[&a]);
}

/// Decision 57: the sealed copy's hash is stored and emitted as given, zero allowed, and is not
/// folded into the pagaré hash or the issuance entry.
#[test]
fn the_sealed_hash_is_stored_as_given() {
    let mut s = setup();
    let borrower = s.borrower.insecure_clone();
    for (i, sealed) in [[0x5e; 32], [0; 32], core::array::from_fn(|j| j as u8)].into_iter().enumerate() {
        s.env.sealed_hash = sealed;
        let ix = s.env.draw_ix_amount(&s.k, s.line_id, &borrower.pubkey(), UNIT);
        let meta = s.env.send(ix, &borrower, &[]);
        let issued: PagareIssued = find_event(&meta).expect("PagareIssued");
        let record = s.env.pagare(&s.k, s.line_id, i as u16);
        assert_eq!((record.sealed_hash, issued.sealed_hash), (sealed, sealed));
        let issuance = reference::pagare::issuance_hash(&record.pagare_hash, &record.signed_pagare_hash);
        let entry = reference::pagare::entry_hash(&record.asset.to_bytes(), 0, &s.k.pool.to_bytes(), &s.k.pool.to_bytes(), &issuance, record.issued_at, 1);
        assert_eq!(record.log_root, reference::pagare::genesis_root(&entry), "the issuance entry does not bind the sealed hash");
        let derived = reference::pagare::pagare_hash(&s.env.line(&s.k, s.line_id).pagare_template_hash, &s.env.line(&s.k, s.line_id).terms_hash, &line_pda(&s.k.pool, s.line_id).0.to_bytes(), i as u16, UNIT, record.issued_at);
        assert_eq!(record.pagare_hash, derived);
    }
}

/// The bytes no instruction may change once a record exists: everything but `status ..=
/// last_kind`, the payment state and the registry (`record::offset`). They include `cotejo` and
/// `sealed_hash`.
fn frozen(data: &[u8]) -> Vec<u8> {
    use elote_pool::record::offset::{MUTABLE_END, STATUS};
    [&data[..STATUS], &data[MUTABLE_END..]].concat()
}

struct Watch {
    keys: Vec<u8>,
    records: Vec<(Pubkey, Vec<u8>)>,
}

impl Watch {
    fn pool_keys(env: &Env, k: &PoolKeys) -> Vec<u8> {
        let at = reference::layout::POOL_COTEJO_KEYS;
        env.account(&k.pool).data[at..at + 96].to_vec()
    }

    fn new(env: &Env, k: &PoolKeys) -> Self {
        Watch { keys: Self::pool_keys(env, k), records: Vec::new() }
    }

    /// Adds the records of `line_id` drawn since.
    fn track(&mut self, env: &Env, k: &PoolKeys, line_id: u64) {
        let line = line_pda(&k.pool, line_id).0;
        for i in 0..env.line(k, line_id).draws {
            let key = pagare_pda(&line, i).0;
            if !self.records.iter().any(|(r, _)| *r == key) {
                self.records.push((key, frozen(&env.account(&key).data)));
            }
        }
    }

    fn check(&self, env: &Env, k: &PoolKeys, what: &str) {
        assert_eq!(Self::pool_keys(env, k), self.keys, "{what} changed the cotejo keys");
        for (key, bytes) in &self.records {
            assert_eq!(&frozen(&env.account(key).data), bytes, "{what} changed a pagaré's issued fields ({key})");
        }
    }
}

/// Every instruction but `set_cotejo_keys` leaves `pool.cotejo_keys` alone, and none rewrites a
/// pagaré's `cotejo`, `sealed_hash` or any other issued field: a byte-level check after each step
/// of a run through the instruction set (all but `init_pool`, which creates the keys zero, and
/// `accept_council`, which writes only the council fields and is not driven here).
#[test]
fn only_set_cotejo_keys_moves_the_keys_and_no_instruction_rewrites_an_issued_pagare() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let council = env.council.insecure_clone();
    let cp = council.pubkey();
    let cotejo = Keypair::new();
    env.set_cotejo_keys(&k, [cotejo.pubkey(), ZERO, ZERO]);
    env.cotejo = Some(cotejo.insecure_clone());
    env.sealed_hash = [0x57; 32];
    let mut w = Watch::new(&env, &k);
    macro_rules! step {
        ($what:expr, $e:expr) => {{
            let r = $e;
            w.check(&env, &k, $what);
            r
        }};
    }
    let investor = step!("deposit", {
        let i = env.investor(&k, 60 * UNIT);
        env.deposit(&k, &i, 60 * UNIT);
        i
    });
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(10 * UNIT, 1_200, 6, 5_000);
    let (id, _) = step!("request_line", env.request_line_with(&k, &borrower, &p, None));
    step!("approve_line", env.approve_line(&k, id, &borrower.pubkey(), &p));
    for _ in 0..3 {
        step!("draw", env.draw_amount(&k, id, &borrower, 3 * UNIT));
    }
    w.track(&env, &k, id);
    let procurador = Pubkey::new_unique();
    step!("endorse_pagare", env.procuracion(&k, id, 0, &procurador, ENDOSO_HASH));
    step!("revoke_procuracion", {
        let ix = env.revoke_procuracion_ix(&k, id, 0, &cp, [0x3e; 32]);
        env.send(ix, &council, &[])
    });
    step!("endorse_pagare again", env.procuracion(&k, id, 0, &Pubkey::new_unique(), ENDOSO_HASH));
    step!("court_annotation", env.court_annotation(&k, id, 1, KIND_ANOTACION_JUDICIAL, [0xc0; 32]));
    let c = env.council.insecure_clone();
    let first = 3 * UNIT + env.interest_unpaid(&k, id);
    env.mint_to(&k.asset_mint, &c, &borrower.pubkey(), 20 * UNIT);
    step!("repay", env.repay(&k, id, &borrower, first));
    let entries = env.pagare(&k, id, 0).entry_count;
    step!("annotate_payment", {
        let ix = env.annotate_payment_ix(&k, id, 0, &cp, entries - 1, [0xaa; 32]);
        env.send(ix, &council, &[])
    });
    step!("condone", env.condone(&k, id, 0, UNIT, [0xd5; 32]));
    for (what, data) in [
        ("suspend_draws", ix::SuspendDraws { valid_until: env.valid_until() }.data()),
        ("resume_draws", ix::ResumeDraws { valid_until: env.valid_until() }.data()),
        ("propose_acceleration", ix::ProposeAcceleration { valid_until: env.valid_until() }.data()),
        ("withdraw_acceleration", ix::WithdrawAcceleration { valid_until: env.valid_until() }.data()),
        ("resume_draws", ix::ResumeDraws { valid_until: env.valid_until() }.data()),
    ] {
        step!(what, env.council_line(&k, id, data));
    }
    let vu = env.valid_until();
    for (what, data) in [
        ("set_max_line_bps", ix::SetMaxLineBps { max_line_bps: 6_000, valid_until: vu }.data()),
        ("set_provision_bps", ix::SetProvisionBps { provision_bps: 9_000, valid_until: vu }.data()),
        ("set_outstanding_cap", ix::SetOutstandingCap { max_outstanding_principal: 900 * UNIT, valid_until: vu }.data()),
        ("set_fix_bounds", ix::SetFixBounds { min_fix_rate: 0, max_fix_rate: 0, valid_until: vu }.data()),
        ("set_acceleration_notice", ix::SetAccelerationNotice { acceleration_notice_seconds: ACCELERATION_NOTICE, valid_until: vu }.data()),
        ("set_collection_period", ix::SetCollectionPeriod { collection_period_seconds: COLLECTION_PERIOD, valid_until: vu }.data()),
        ("pause", ix::Pause { valid_until: vu }.data()),
        ("resume", ix::Resume { valid_until: vu }.data()),
    ] {
        let ix = if what == "set_collection_period" { env.set_collection_period_ix(&k, &cp, COLLECTION_PERIOD) } else { env.council_pool_ix(&k, &cp, data) };
        step!(what, env.send(ix, &council, &[]));
    }
    step!("set_fx_feeder", {
        let ix = env.set_fx_feeder_ix(&k, &cp, &Pubkey::new_unique());
        env.send(ix, &council, &[])
    });
    step!("post_fix", {
        let today = env.today();
        let ix = env.post_fix_ix(&k, &cp, PARITY_FIX + 1, today);
        env.send(ix, &council, &[])
    });
    step!("propose_council", {
        let next = squads_vault_pda(&multisig_pda(&Pubkey::new_unique()).0).0;
        let ix = env.propose_council_ix(&k, &cp, &next, elote_pool::state::COUNCIL_SQUADS);
        env.send(ix, &council, &[])
    });
    step!("renounce_availability", {
        let ix = env.renounce_availability_ix(&k, id, &borrower.pubkey());
        env.send(ix, &borrower, &[])
    });
    // The rest of the line paid: the títulos are delivered, one is burnt, and one of another line
    // is annulled.
    let payoff = env.payoff(&k, id);
    step!("repay to the end", env.repay(&k, id, &borrower, payoff));
    step!("deliver_pagare", env.deliver_pagare(&k, id, 0));
    step!("deliver_pagare", env.deliver_pagare(&k, id, 1));
    step!("burn_pagare", {
        let ix = env.burn_pagare_ix(&k, id, 0, &borrower.pubkey(), [0xbb; 32]);
        env.send(ix, &borrower, &[])
    });
    let other = env.borrower(&k);
    let q = env.scheduled_params(6 * UNIT, 1_200, 6, 5_000);
    let (id2, _) = env.open_line_with(&k, &other, &q, None);
    step!("draw on a second line", env.draw_amount(&k, id2, &other, 2 * UNIT));
    step!("draw on a second line", env.draw_amount(&k, id2, &other, 2 * UNIT));
    w.track(&env, &k, id2);
    step!("annul_pagare", {
        let ix = env.annul_pagare_ix(&k, id2, 1, [0x3c; 32]);
        env.send(ix, &council, &[])
    });
    // Collection, acceleration and write-off.
    let line2 = env.line(&k, id2);
    env.warp(line2.first_due_at + 4 * line2.interval_seconds + 1);
    step!("mark_in_collection", {
        let ix = env.mark_in_collection_ix(&k, id2);
        env.send(ix, &investor, &[])
    });
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    step!("write_off", {
        let ix = env.write_off_ix(&k, id2, &cp);
        env.send(ix, &council, &[])
    });
    let recovery = UNIT;
    env.mint_to(&k.asset_mint, &c, &other.pubkey(), 10 * UNIT);
    step!("repay on a written-off line", env.repay(&k, id2, &other, recovery));
    let third = env.borrower(&k);
    let (id3, _) = env.open_line_with(&k, &third, &env.scheduled_params(6 * UNIT, 1_200, 6, 5_000), None);
    step!("draw on a third line", env.draw_amount(&k, id3, &third, 2 * UNIT));
    w.track(&env, &k, id3);
    step!("propose_acceleration", env.council_line(&k, id3, ix::ProposeAcceleration { valid_until: env.valid_until() }.data()));
    env.warp(env.now() + i64::from(ACCELERATION_NOTICE) + 1);
    step!("accelerate_line", env.council_line(&k, id3, ix::AccelerateLine { valid_until: env.valid_until() }.data()));
    // A rejected, a withdrawn and a cancelled line.
    let (r, _) = step!("request_line", env.request_line_with(&k, &third, &env.scheduled_params(6 * UNIT, 1_200, 6, 5_000), None));
    step!("reject_line", {
        let ix = env.reject_line_ix(&k, &cp, r, &cp);
        env.send(ix, &council, &[])
    });
    let (wd, _) = env.request_line_with(&k, &third, &env.scheduled_params(6 * UNIT, 1_200, 6, 5_000), None);
    step!("withdraw_request", {
        let ix = env.withdraw_request_ix(&k, &third.pubkey(), wd, &cp);
        env.send(ix, &third, &[])
    });
    let (cl, _) = env.open_line_with(&k, &third, &env.scheduled_params(6 * UNIT, 1_200, 6, 5_000), None);
    step!("cancel_line", {
        let ix = env.cancel_line_ix(&k, cl, &cp);
        env.send(ix, &council, &[])
    });
    step!("redeem", {
        let shares = env.token_balance(&k.share_ata(&investor.pubkey())) / 10;
        env.redeem(&k, &investor, shares)
    });
    // The one instruction that moves the keys.
    env.set_cotejo_keys(&k, [ZERO, Pubkey::new_unique(), ZERO]);
    assert_ne!(Watch::pool_keys(&env, &k), w.keys);
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        /// Random key sets (empty slots anywhere) and a random cotejo account: the program draws
        /// exactly when the reference says so, and records the key it names.
        #[test]
        fn the_program_follows_the_reference_rule(slots in proptest::collection::vec(any::<bool>(), 3), pick in 0usize..6, signs in any::<bool>()) {
            let mut s = setup();
            let pool_keys: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
            let keys: [Pubkey; 3] = core::array::from_fn(|i| if slots[i] { pool_keys[i].pubkey() } else { ZERO });
            s.env.set_cotejo_keys(&s.k, keys);
            let stranger = Keypair::new();
            let zero = Keypair::pda(ZERO);
            // 0..3: a pool key (set or empty slot), 3: a stranger, 4: the zero key, 5: none.
            let chosen: Option<&Keypair> = match pick { 0..=2 => Some(&pool_keys[pick]), 3 => Some(&stranger), 4 => Some(&zero), _ => None };
            let cotejo = chosen.map(|c| (c.pubkey(), signs));
            let expected = reference::cotejo::co_signer(&keys_bytes(&keys), cotejo.map(|(k, sig)| (k.to_bytes(), sig)));
            let borrower = s.borrower.insecure_clone();
            let ix = draw_with(&s, cotejo);
            let extra: Vec<&Keypair> = chosen.filter(|_| signs).into_iter().collect();
            let before = snapshot(&s);
            match (s.env.try_send(ix, &borrower, &extra), expected) {
                (Ok(_), Ok(key)) => prop_assert_eq!(s.env.pagare(&s.k, s.line_id, 0).cotejo.to_bytes(), key),
                (Err(e), Err(())) => {
                    prop_assert_eq!(custom_error_code(&e.err), Some(E_COTEJO_REQUIRED));
                    prop_assert_eq!(snapshot(&s), before);
                }
                (Ok(_), Err(())) => prop_assert!(false, "drew where the reference refuses"),
                (Err(e), Ok(_)) => prop_assert!(false, "refused where the reference draws: {:?}", e.err),
            }
        }
    }
}
