//! Compute-unit regression tests. The only loop in the program is `math::arrears`, bounded by
//! `MAX_INSTALLMENTS` (120). These tests drive it at 18 installments (the pilot's longest term)
//! and at the cap, on the three instructions that run it, and pin a ceiling so a change that
//! makes the loop heavier fails here before it fails on chain.
//!
//! The ceiling is below the 200_000 CU a transaction gets per instruction without a compute
//! budget instruction, so clients never need to request more.
//!
//! The figures are printed by each test and listed in ARCHITECTURE.md §10.

mod common;

use common::*;
use solana_signer::Signer;

/// Every instruction must stay below this, with headroom for a future accrual of state reads.
const CEILING: u64 = 100_000;

/// Compute units of `repay`, `mark_in_collection` and `write_off` on a line with `installments`
/// installments, all of them missed, so the arrears loop runs its full length each time.
fn compute_units(installments: u16) -> [(&'static str, u64); 3] {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, installments, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);

    // Past the last due date: every installment is missed and late interest has run on each.
    env.warp(env.now() + i64::from(installments) * p.interval() + 1);
    let late = env.arrears(&k, line_id).late_due;
    assert!(late > 0);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    // The write-off waits for the collection period from the mark.
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let write_off = env.send(ix, &council, &[]).compute_units_consumed;
    [("repay", repay), ("mark_in_collection", mark), ("write_off", write_off)]
}

/// `init_pool` walks the extensions of both mints and validates the share mint; the heaviest
/// realistic shape is a Token-2022 asset with issuer controls and a permissioned share mint.
#[test]
fn init_pool_stays_under_the_ceiling_with_and_without_extensions() {
    let mut env = Env::new();
    let (_, meta) = env.setup_pool();
    let plain = meta.compute_units_consumed;
    let gate = env.funded(1);
    let (_, meta) = env.setup_permissioned_pool(&gate.pubkey());
    let permissioned = meta.compute_units_consumed;
    println!("init_pool, classic asset and plain shares: {plain} CU");
    println!("init_pool, classic asset and permissioned shares: {permissioned} CU");
    assert!(plain < CEILING && permissioned < CEILING, "init_pool used {plain} / {permissioned} CU, ceiling {CEILING}");
}

/// `draw` carries the Metaplex Core CPI (about 37 000 CU, fixed by the plugin set), so it has its
/// own ceiling, 10 000 CU below `CEILING`: a growth in `draw` fails here with that margin left.
const DRAW_CEILING: u64 = 90_000;

/// `draw` on a 120-installment line: the payability check and the tranche interest add a few
/// multiplications; the Metaplex Core CPI lands here too and so does the registry's issuance
/// entry. Deterministic since the client passes the PDA bumps. Measured on the first and second
/// tranches and on a tranche with a two-digit index (a longer name and attribute); about 82 000 CU
/// each. Trimming attributes is the lever if it grows.
#[test]
fn draw_stays_under_its_ceiling_at_120_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 40 * UNIT);
    let first = env.send(ix, &borrower, &[]).compute_units_consumed;
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 50 * UNIT);
    let second = env.send(ix, &borrower, &[]).compute_units_consumed;
    let mut tenth = 0;
    for _ in 2..=10 {
        let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
        tenth = env.send(ix, &borrower, &[]).compute_units_consumed;
    }
    assert_eq!(env.line(&k, line_id).draws, 11);
    println!("draw, first tranche at 120 installments: {first} CU");
    println!("draw, second tranche at 120 installments: {second} CU");
    println!("draw, tranche 10 at 120 installments: {tenth} CU");
    for (what, cu) in [("first", first), ("second", second), ("tranche 10", tenth)] {
        assert!(cu <= DRAW_CEILING, "draw ({what}) used {cu} CU, ceiling {DRAW_CEILING}");
    }
}

/// `post_fix` by the council (multisig check) and by the feeder (band and date checks, and the
/// market reference when it exists).
#[test]
fn post_fix_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let council = env.council.insecure_clone();
    let feeder = env.funded(1);
    let ix = env.set_fx_feeder_ix(&k, &council.pubkey(), &feeder.pubkey());
    env.send(ix, &council, &[]);
    let ix = env.post_fix_ix(&k, &council.pubkey(), 183_740, env.today());
    let by_council = env.send(ix, &council, &[]).compute_units_consumed;
    env.warp(env.now() + 86_400);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 184_000, env.today());
    let by_feeder = env.send(ix, &feeder, &[]).compute_units_consumed;
    // With the market reference present, as on mainnet.
    env.put_reference_rate(185_000, 3_600);
    env.warp(env.now() + 86_400);
    let ix = env.post_fix_ix(&k, &feeder.pubkey(), 185_100, env.today());
    let with_reference = env.send(ix, &feeder, &[]).compute_units_consumed;
    println!("post_fix, by the council: {by_council} CU");
    println!("post_fix, by the feeder: {by_feeder} CU");
    println!("post_fix, by the feeder against the reference: {with_reference} CU");
    assert!(by_council < CEILING && by_feeder < CEILING && with_reference < CEILING);
}

/// `cancel_pagare` runs no arrears loop: one coverage comparison, the sidecar's mark and the Core
/// `BurnV1` CPI. Measured on a 120-installment line repaid in full, the shape of a real
/// cancellation.
#[test]
fn cancel_pagare_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let interest = env.line(&k, line_id).interest_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 100 * UNIT + interest);
    let ix = env.annotate_payment_ix(&k, line_id, 0, &council.pubkey(), 2, [0x4f; 32]);
    let annotate = env.send(ix, &council, &[]).compute_units_consumed;
    println!("annotate_payment at 120 installments: {annotate} CU");
    assert!(annotate < CEILING);
    let cancel = env.cancel_pagare(&k, line_id, 0).compute_units_consumed;
    println!("cancel_pagare at 120 installments: {cancel} CU");
    assert!(cancel < CEILING, "cancel_pagare used {cancel} CU, ceiling {CEILING}");
    // The procurador's branch, on a second line.
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(50 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let procurador = env.funded(1);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 50 * UNIT + interest);
    let ix = env.cancel_pagare_by_procurador_ix(&k, line_id, 0, &procurador.pubkey(), &council.pubkey(), RECEIPT_HASH);
    let by_procurador = env.send(ix, &council, &[&procurador]).compute_units_consumed;
    println!("cancel_pagare by the procurador at 120 installments: {by_procurador} CU");
    assert!(by_procurador < CEILING, "cancel by the procurador used {by_procurador} CU, ceiling {CEILING}");
    // The borrower's exit, on a third line under a procuración: alone, after the cancel grace.
    let borrower = env.borrower(&k);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH);
    let interest = env.line(&k, line_id).interest_due;
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), interest);
    env.repay(&k, line_id, &borrower, 50 * UNIT + interest);
    env.warp(env.now() + i64::from(CANCEL_GRACE) + 1);
    let ix = env.cancel_pagare_ix(&k, line_id, 0, &borrower.pubkey(), &borrower.pubkey(), RECEIPT_HASH);
    let by_borrower = env.send(ix, &borrower, &[]).compute_units_consumed;
    println!("cancel_pagare by the borrower at 120 installments: {by_borrower} CU");
    assert!(by_borrower < CEILING);
}

/// After the mark every remaining installment is due, so a repayment early in a 120-installment
/// schedule loops over all of them instead of the few that were missed; the figure is the
/// all-missed shape above, which the ceiling already covers.
#[test]
fn repay_after_the_mark_stays_under_the_ceiling_at_120_installments() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    env.warp(env.now() + 3 * p.interval() + 1);
    let ix = env.mark_in_collection_ix(&k, line_id);
    let mark = env.send(ix, &investor, &[]).compute_units_consumed;
    let late = env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), late + 1_000);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), late + 1_000);
    let repay = env.send(ix, &borrower, &[]).compute_units_consumed;
    env.warp(env.now() + i64::from(COLLECTION_PERIOD) + 1);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let write_off = env.send(ix, &council, &[]).compute_units_consumed;
    println!("mark_in_collection at 120 installments, 3 missed: {mark} CU");
    println!("repay after the mark at 120 installments: {repay} CU");
    println!("write_off after the mark at 120 installments: {write_off} CU");
    assert!(mark < CEILING && repay < CEILING && write_off < CEILING);
}

/// `endorse_pagare` en procuración: one registry entry and one Core CPI (the first time an
/// `AddPluginV1`, later an approve).
#[test]
fn procuracion_stays_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(100 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, line_id, &borrower);
    let procurador = env.funded(1);
    let cu = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH).compute_units_consumed;
    println!("endorse_pagare (procuracion, plugin added) at 120 installments: {cu} CU");
    assert!(cu < CEILING, "procuracion used {cu} CU, ceiling {CEILING}");
    let council = env.council.insecure_clone();
    let ix = env.revoke_procuracion_ix(&k, line_id, 0, &council.pubkey(), [0x7e; 32]);
    let revoke = env.send(ix, &council, &[]).compute_units_consumed;
    println!("revoke_procuracion at 120 installments: {revoke} CU");
    let again = env.procuracion(&k, line_id, 0, &procurador.pubkey(), ENDOSO_HASH).compute_units_consumed;
    println!("endorse_pagare (procuracion, plugin approved) at 120 installments: {again} CU");
    assert!(revoke < CEILING && again < CEILING);
    let court = env
        .court_annotation(&k, line_id, 0, elote_pool::state::KIND_ANOTACION_JUDICIAL, [0x6a; 32])
        .compute_units_consumed;
    println!("court_annotation at 120 installments: {court} CU");
    assert!(court < CEILING);
}

#[test]
fn arrears_instructions_stay_under_the_ceiling_at_18_and_120_installments() {
    for n in [18u16, 120] {
        for (name, cu) in compute_units(n) {
            println!("{name} at {n} installments: {cu} CU");
            assert!(cu < CEILING, "{name} at {n} installments used {cu} CU, ceiling {CEILING}");
        }
    }
}

/// The ceiling case of the payment entries: one payment covering three pagarés (the most `repay`
/// takes, `state::MAX_SIDECARS_PER_REPAY`) on a 120-installment line with every installment missed
/// (late interest on all of them), with at least 5 000 CU to spare.
#[test]
fn a_payment_covering_three_sidecars_stays_under_the_ceiling() {
    use elote_pool::state::MAX_SIDECARS_PER_REPAY;
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 200 * UNIT);
    env.deposit(&k, &investor, 200 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(30 * UNIT, 1_200, 120, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    for _ in 0..MAX_SIDECARS_PER_REPAY {
        env.draw_amount(&k, line_id, &borrower, 10 * UNIT);
    }
    let t0 = env.line(&k, line_id).drawn_at;
    env.warp(t0 + 120 * p.interval() + 1);
    let line = env.line(&k, line_id);
    let owed = line.drawn + line.interest_due - line.principal_repaid - line.interest_repaid + env.arrears(&k, line_id).late_due;
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), owed);
    let sidecars = env.payment_sidecars(&k, line_id, owed);
    assert_eq!(sidecars.len(), MAX_SIDECARS_PER_REPAY);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), owed);
    let cu = env.send(ix, &borrower, &[]).compute_units_consumed;
    println!("repay covering three sidecars at 120 installments, all missed: {cu} CU");
    assert_eq!(usize::from(env.line(&k, line_id).pagares_paid), MAX_SIDECARS_PER_REPAY);
    assert!(cu + 5_000 <= CEILING, "the three-sidecar repay used {cu} CU, ceiling {CEILING} less a 5 000 CU margin");
}

#[test]
fn request_and_approve_stay_under_the_ceiling() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let mut p = env.scheduled_params(4 * UNIT, 1_200, 120, 5_000);
    p.sponsor = None;
    let (id, request) = env.request_line_with(&k, &borrower, &p, None);
    let approve = env.approve_line(&k, id, &borrower.pubkey(), &p);
    let (request, approve) = (request.compute_units_consumed, approve.compute_units_consumed);
    println!("request_line at 120 installments: {request} CU");
    println!("approve_line (with the multisig check): {approve} CU");
    assert!(request < CEILING && approve < CEILING);
}

/// What moves `init_pool`'s cost between pools: Anchor's `init` finds the canonical bump of the
/// pool, vault and dead-shares PDAs, and `init_pool` the Squads vault's, each try costing about
/// 1,500 CU; the canonical bump differs by address, so two pools with the same extensions can
/// differ by thousands of CU. Printed per pool with the tries each address took.
#[test]
fn init_pool_cost_follows_the_pda_bump_searches() {
    let mut figures = Vec::new();
    for permissioned in [false, true, false, true, false, true] {
        let mut env = Env::new();
        let (k, meta) = if permissioned {
            let gate = env.funded(1);
            env.setup_permissioned_pool(&gate.pubkey())
        } else {
            env.setup_pool()
        };
        let tries = |bump: u8| 256 - u32::from(bump);
        let searches = tries(pool_pda(&k.creator, &k.asset_mint).1)
            + tries(vault_pda(&k.pool).1)
            + tries(dead_shares_pda(&k.pool).1)
            + tries(squads_vault_pda(&env.multisig).1);
        let cu = meta.compute_units_consumed;
        println!("init_pool permissioned={permissioned}: {cu} CU, {searches} bump tries in all");
        figures.push((permissioned, cu, searches));
        assert!(cu < 70_000, "init_pool used {cu} CU");
    }
    // Same shape: the cost less 1,500 CU per try is one figure, so the tries explain all of it.
    for p in [false, true] {
        let base: Vec<u64> = figures.iter().filter(|f| f.0 == p).map(|f| f.1 - 1_500 * u64::from(f.2)).collect();
        assert!(base.windows(2).all(|w| w[0] == w[1]), "permissioned={p}: {base:?}");
    }
}

#[test]
fn the_council_line_instructions_and_the_pause_stay_under_the_ceiling() {
    use anchor_lang::InstructionData;
    use elote_pool::instruction as ix;
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 120, 5_000);
    let (id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw(&k, id, &borrower);
    let suspend = env.council_line(&k, id, ix::SuspendDraws { valid_until: env.valid_until() }.data()).compute_units_consumed;
    let resume = env.council_line(&k, id, ix::ResumeDraws { valid_until: env.valid_until() }.data()).compute_units_consumed;
    let propose = env.council_line(&k, id, ix::ProposeAcceleration { valid_until: env.valid_until() }.data()).compute_units_consumed;
    env.warp(p.first_due_at() + 119 * p.interval());
    let accelerate = env.council_line(&k, id, ix::AccelerateLine { valid_until: env.valid_until() }.data()).compute_units_consumed;
    let council = env.council.insecure_clone();
    let i = env.pause_ix(&k, &council.pubkey());
    let pause = env.send(i, &council, &[]).compute_units_consumed;
    println!("suspend_draws: {suspend} CU");
    println!("resume_draws: {resume} CU");
    println!("propose_acceleration: {propose} CU");
    println!("accelerate_line at 120 installments, all due: {accelerate} CU");
    println!("pause by the council: {pause} CU");
    for cu in [suspend, resume, propose, accelerate, pause] {
        assert!(cu < CEILING);
    }
}
