//! Credit lines approved from the pool: request and approval with an optional
//! sponsor, draws in tranches, one repayment order, write-off on what was drawn.

mod common;

use common::*;
use elote_pool::events::{LineDrawn, LineRequested, LineWrittenOff};
use elote_pool::state::{LineStatus, SPONSOR_GUARANTOR, SPONSOR_PROMOTER};
use solana_signer::Signer;

/// A pool with one investor who deposited `assets`.
fn pool_with(env: &mut Env, assets: u64) -> (PoolKeys, Keypair) {
    pool_capped(env, assets, HARNESS_MAX_LINE_BPS)
}

/// As `pool_with`, with the line cap `max_line_bps`.
fn pool_capped(env: &mut Env, assets: u64, max_line_bps: u16) -> (PoolKeys, Keypair) {
    let (k, _) = env.setup_pool_capped(max_line_bps);
    let a = env.investor(&k, assets);
    env.deposit(&k, &a, assets);
    (k, a)
}

#[test]
fn open_draw_repay_full_cycle_raises_share_value() {
    let mut env = Env::new();
    let (k, a) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);

    // Open 4.0 at 10% a year for a year. Nothing is owed until a tranche is drawn.
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, None);
    assert_eq!(line_id, 0);
    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    assert_eq!(ev.pool, k.pool);
    assert_eq!(ev.line_id, 0);
    assert_eq!(ev.borrower, borrower.pubkey());
    assert_eq!(ev.sponsor, None);
    assert_eq!(ev.agreement_hash, AGREEMENT_HASH);
    assert_eq!(ev.principal, 4 * UNIT);
    assert_eq!(ev.rate_bps, 1_000);
    assert_eq!(ev.term_end, p.term_end);
    assert_eq!((ev.first_due_at, ev.expediente_root, ev.pagare_template_hash), (0, EXPEDIENTE_ROOT, PAGARE_TEMPLATE_HASH));
    assert_eq!((ev.currency, ev.payer), (elote_pool::state::CURRENCY_MXN, env.council.pubkey()));
    assert_eq!(ev.terms_hash, env.terms_hash(&k, 0, &borrower.pubkey(), &p));
    assert_eq!((ev.installments, ev.interval_seconds, ev.late_bps, ev.max_missed), (0, 0, 0, 0), "a bullet line");

    let line = env.line(&k, 0);
    assert_eq!(line.pool, k.pool);
    assert_eq!(line.borrower, borrower.pubkey());
    assert_eq!(line.status, LineStatus::Approved);
    assert_eq!((line.principal_seconds, line.drawn, line.draws), (0, 0, 0));
    assert_eq!(line.agreement_hash, AGREEMENT_HASH);
    assert_eq!(line.expediente_root, EXPEDIENTE_ROOT);
    assert_eq!((line.requested_at, line.approved_at, line.availability_end), (env.now(), env.now(), p.term_end));
    assert_eq!((line.currency, line.terms_hash), (elote_pool::state::CURRENCY_MXN, env.terms_hash(&k, 0, &borrower.pubkey(), &p)));
    assert_eq!(line.bump, line_pda(&k.pool, 0).1);
    assert_eq!(env.pool(&k).line_count, 1);
    assert_eq!(env.nav(&k), 10 * UNIT, "approval moves no money");

    // Drawn in full at once: 0.4 of interest over the year.
    let ev = env.draw(&k, 0, &borrower);
    assert_eq!((ev.pool, ev.line_id, ev.amount_mxn, ev.outstanding_after), (k.pool, 0, 4 * UNIT, 4 * UNIT));
    assert_eq!(env.token_balance(&k.vault), 6 * UNIT);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
    assert_eq!(env.nav(&k), 10 * UNIT);
    assert_eq!(env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), 4 * UNIT);
    let line = env.line(&k, 0);
    assert_eq!(line.status, LineStatus::Active);
    assert_eq!(line.drawn_at, env.now());
    assert_eq!((line.drawn, line.draws), (4 * UNIT, 1));
    assert_eq!(line.interest_accrued(p.term_end), Some(400_000), "a year of 10% on 4.0 (decision 48)");

    // The borrower earns the interest elsewhere and pays at the term, the interest fully accrued.
    // Late interest, then the accrued interest, then principal.
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 400_000);
    env.warp(p.term_end);

    let ev = env.repay(&k, 0, &borrower, 2 * UNIT);
    assert_eq!(ev.payer, borrower.pubkey());
    assert_eq!((ev.interest_part, ev.principal_part), (400_000, 1_600_000));
    assert_eq!(ev.remaining, 2_400_000);
    assert_eq!(ev.nav_after, 10_400_000, "interest counts when collected");
    assert_eq!(env.line(&k, 0).status, LineStatus::Active);

    let ev = env.repay(&k, 0, &borrower, 2_400_000);
    assert_eq!((ev.interest_part, ev.principal_part), (0, 2_400_000));
    assert_eq!(ev.remaining, 0);
    assert_eq!(ev.nav_after, 10_400_000);
    let line = env.line(&k, 0);
    assert_eq!(line.status, LineStatus::Repaid);
    assert_eq!((line.principal_repaid, line.interest_repaid), (4 * UNIT, 400_000));
    assert_eq!(line.closed_at, env.now());
    let pool = env.pool(&k);
    assert_eq!(pool.outstanding_principal, 0);
    assert_eq!(pool.interest_collected, 400_000);
    assert_eq!(env.nav(&k), 10_400_000);

    // Every share is now worth 1.04.
    let ev = env.redeem(&k, &a, UNIT);
    assert_eq!(ev.assets, 1_040_000);
}

/// The borrower draws alone: no other key signs a draw, and nobody else can draw on the line.
#[test]
fn the_borrower_draws_alone() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let stranger = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);

    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    assert_eq!(ix.accounts.iter().filter(|m| m.is_signer).map(|m| m.pubkey).collect::<Vec<_>>(), vec![borrower.pubkey(), borrower.pubkey()], "the borrower, also as the rent payer");
    let ix = env.draw_ix_amount(&k, line_id, &stranger.pubkey(), UNIT);
    env.expect_err(ix, &stranger, &[], E_UNAUTHORIZED);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), UNIT);
    env.send(ix, &borrower, &[]);
    assert_eq!(env.line(&k, line_id).drawn, UNIT);
}

#[test]
fn sponsor_must_cosign_when_named_and_its_role_is_recorded() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let impostor = env.funded(1);
    let council = env.council.insecure_clone();
    let mut p = env.line_params(4 * UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());
    p.sponsor_role = SPONSOR_GUARANTOR;

    // Named but absent.
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &[&borrower], E_SPONSOR_REQUIRED);
    // Someone else signs in the sponsor's place.
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, Some(impostor.pubkey()));
    env.expect_err(ix, &council, &[&borrower, &impostor], E_SPONSOR_REQUIRED);
    // The sponsor's account is passed without its signature.
    let mut ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, Some(sponsor.pubkey()));
    let meta = ix.accounts.iter_mut().find(|m| m.pubkey == sponsor.pubkey()).unwrap();
    meta.is_signer = false;
    assert!(env.try_send(ix, &council, &[&borrower]).is_err());
    // A role byte above `SPONSOR_GUARANTOR` means nothing.
    let mut bad = p;
    bad.sponsor_role = 2;
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &bad, Some(sponsor.pubkey()));
    env.expect_err(ix, &council, &[&borrower, &sponsor], E_SPONSOR_ROLE_INVALID);
    assert_eq!(env.pool(&k).line_count, 0);

    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, Some(&sponsor));
    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    assert_eq!(ev.sponsor, Some(sponsor.pubkey()));
    assert_eq!((ev.pool, ev.line_id, ev.sponsor_role), (k.pool, line_id, SPONSOR_GUARANTOR));
    let line = env.line(&k, line_id);
    assert_eq!((line.sponsor, line.sponsor_role), (sponsor.pubkey(), SPONSOR_GUARANTOR));

    // A public promoter: the default role, no payment obligation recorded.
    let mut p = env.line_params(UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());
    let (line_id, meta) = env.open_line_with(&k, &borrower, &p, Some(&sponsor));
    let ev: LineRequested = find_event(&meta).expect("LineRequested");
    assert_eq!(ev.sponsor_role, SPONSOR_PROMOTER);
    assert_eq!(env.line(&k, line_id).sponsor_role, SPONSOR_PROMOTER);
}

#[test]
fn sponsor_pays_interest_on_behalf() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &sponsor.pubkey(), 400_000);
    let mut p = env.line_params(4 * UNIT, 1_000);
    p.sponsor = Some(sponsor.pubkey());
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, Some(&sponsor));
    env.draw(&k, line_id, &borrower);
    env.warp(p.term_end);

    // The accrued interest is paid first, by whoever pays: at the term the sponsor's 0.4 covers
    // it all.
    let ev = env.repay(&k, line_id, &sponsor, 400_000);
    assert_eq!(ev.payer, sponsor.pubkey());
    assert_eq!((ev.principal_part, ev.interest_part, ev.remaining), (0, 400_000, 4 * UNIT));
    assert_eq!(env.token_balance(&ata(&sponsor.pubkey(), &k.asset_mint)), 0);

    let ev = env.repay(&k, line_id, &borrower, 4 * UNIT);
    assert_eq!((ev.principal_part, ev.interest_part, ev.remaining), (4 * UNIT, 0, 0));
    assert_eq!(ev.nav_after, 10_400_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
}

/// Overpayment is clamped on every line: the payer is debited what is due and no more, and the
/// line closes.
#[test]
fn repay_clamps_overpayment_and_rejects_wrong_status() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &borrower.pubkey(), 2 * UNIT);
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);

    // Not drawn yet.
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);

    env.draw(&k, line_id, &borrower);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 0);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);

    // Paused: a borrower can still repay. At the term, the interest fully accrued.
    env.warp(env.line(&k, line_id).term_end);
    env.set_paused(&k, true);
    let ev = env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (400_000, 600_000, 3_400_000));
    env.set_paused(&k, false);

    // 6.0 offered against 3.4 due: 3.4 moves, the rest stays.
    let before = env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint));
    let ev = env.repay(&k, line_id, &borrower, 6 * UNIT);
    assert_eq!((ev.interest_part, ev.principal_part, ev.remaining), (0, 3_400_000, 0));
    assert_eq!(before - env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), 3_400_000);
    assert_eq!(env.line(&k, line_id).status, LineStatus::Repaid);
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
    assert_eq!(env.token_balance(&ata(&borrower.pubkey(), &k.asset_mint)), 1_600_000);
}

#[test]
fn draw_rejects_insufficient_liquidity_strangers_and_the_pause() {
    let mut env = Env::new();
    let (k, a) = pool_capped(&mut env, 4 * UNIT, BPS); // a line of the whole NAV
    let borrower = env.borrower(&k);
    // A line can never exceed the NAV (the cap is at most 100%), so the vault runs short only
    // when a redemption drains it after the approval.
    let line_id = env.open_line(&k, &borrower, 4 * UNIT, 1_000);
    env.redeem(&k, &a, UNIT);
    assert_eq!(env.token_balance(&k.vault), 3 * UNIT);

    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_INSUFFICIENT_LIQUIDITY);
    assert_eq!(env.token_balance(&k.vault), 3 * UNIT);

    // A smaller tranche fits; the rest waits for a deposit.
    env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_INSUFFICIENT_LIQUIDITY);
    let council = env.council.insecure_clone();
    env.mint_to(&k.asset_mint, &council, &a.pubkey(), UNIT);
    env.deposit(&k, &a, UNIT);

    // Only the borrower draws.
    let other = env.borrower(&k);
    let ix = env.draw_ix(&k, line_id, &other.pubkey());
    env.expect_err(ix, &other, &[], E_UNAUTHORIZED);

    // Paused: no draw.
    env.set_paused(&k, true);
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_PAUSED);
    env.set_paused(&k, false);

    env.draw(&k, line_id, &borrower);
    assert_eq!(env.token_balance(&k.vault), 0);
    // Nothing left to draw: a tranche of zero is refused, and so is one above what is available.
    let ix = env.draw_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    let ix = env.draw_ix_amount(&k, line_id, &borrower.pubkey(), 1);
    env.expect_err(ix, &borrower, &[], E_WRONG_AMOUNT);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT, "drawn in two tranches");
    assert_eq!(env.line(&k, line_id).draws, 2);
}

#[test]
fn write_off_after_term_lowers_nav_once() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);

    // An approved line is not written off.
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);

    env.draw(&k, line_id, &borrower);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);
    env.warp(p.term_end);
    // 1.0 repaid at the term: the 0.4 of interest accrued first, then 0.6 of principal.
    env.repay(&k, line_id, &borrower, UNIT);
    assert_eq!(env.nav(&k), 10_400_000);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_TERM_NOT_ENDED);

    env.warp(p.term_end + 1);
    let ix = env.write_off_ix(&k, line_id, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_UNAUTHORIZED);

    // Pause does not block recognising a loss. The loss is the drawn principal still unpaid.
    env.set_paused(&k, true);
    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    let meta = env.send(ix, &council, &[]);
    let ev: LineWrittenOff = find_event(&meta).expect("LineWrittenOff");
    assert_eq!((ev.pool, ev.line_id, ev.loss, ev.nav_after), (k.pool, line_id, 3_400_000, 7 * UNIT));
    let pool = env.pool(&k);
    assert_eq!(pool.outstanding_principal, 0);
    assert_eq!(pool.written_off, 3_400_000);
    assert_eq!(env.nav(&k), 7 * UNIT);
    let line = env.line(&k, line_id);
    assert_eq!(line.status, LineStatus::WrittenOff);
    assert_eq!(line.closed_at, p.term_end + 1);
    env.set_paused(&k, false);

    let ix = env.write_off_ix(&k, line_id, &council.pubkey());
    env.expect_err(ix, &council, &[], E_LINE_STATUS);
    assert_eq!(env.nav(&k), 7 * UNIT, "the loss counts once");
    let ix = env.repay_ix(&k, line_id, &borrower.pubkey(), UNIT);
    env.expect_err(ix, &borrower, &[], E_LINE_STATUS);
}

#[test]
fn open_rejects_bad_params() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let council = env.council.insecure_clone();
    let good = env.line_params(4 * UNIT, 1_000);
    let signers: [&Keypair; 1] = [&borrower];

    let mut p = good;
    p.principal = 0;
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, E_WRONG_AMOUNT);

    let mut p = good;
    p.rate_bps = 10_001;
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, E_WRONG_AMOUNT);

    let mut p = good;
    p.term_end = env.now();
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, E_TERM_ENDED);

    let mut p = good;
    p.agreement_hash = [0; 32];
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, E_AGREEMENT_HASH_REQUIRED);

    // The expediente root is a commitment the line cannot do without.
    let mut p = good;
    p.expediente_root = [0; 32];
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, E_EXPEDIENTE_ROOT_REQUIRED);

    // Out of sequence: the next id is 0.
    let ix = env.request_line_ix(&k, &council.pubkey(), 1, &borrower.pubkey(), &good, None);
    env.expect_err(ix, &council, &signers, E_WRONG_ACCOUNT);

    // The pagaré template is part of what the council approves.
    let mut p = good;
    p.pagare_template_hash = [0; 32];
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &p, None);
    env.expect_err(ix, &council, &signers, ERR_BASE + elote_pool::errors::PoolError::PagareTemplateHashRequired as u32);

    // Paused: no new lines.
    env.set_paused(&k, true);
    let ix = env.request_line_ix(&k, &council.pubkey(), 0, &borrower.pubkey(), &good, None);
    env.expect_err(ix, &council, &signers, E_PAUSED);
    env.set_paused(&k, false);

    // The full rate is accepted and the next ids follow in sequence; at 100% a year a line drawn
    // in full for a year owes its principal again.
    let mut p = good;
    p.rate_bps = 10_000;
    let (id0, _) = env.open_line_with(&k, &borrower, &p, None);
    let (id1, _) = env.open_line_with(&k, &borrower, &good, None);
    assert_eq!((id0, id1), (0, 1));
    env.draw(&k, id0, &borrower);
    assert_eq!(env.line(&k, 0).interest_accrued(env.now() + YEAR), Some(4 * UNIT));
    assert_eq!(env.pool(&k).line_count, 2);
}

/// The window closes strictly, at `term_end` on a bullet line. An approved line nobody drew
/// stays `Approved` and lapses; `cancel_line` clears it.
#[test]
fn the_draw_window_of_a_bullet_line_closes_at_the_term_exactly() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (late, _) = env.open_line_with(&k, &borrower, &p, None);
    let (on_time, _) = env.open_line_with(&k, &borrower, &p, None);

    // One second before the term a draw is still allowed, and earns one second of interest.
    env.warp(p.term_end - 1);
    env.draw_amount(&k, on_time, &borrower, 4 * UNIT);
    assert_eq!((4_000_000u128 * 1_000 / (10_000 * YEAR as u128)) as u64, 0);
    assert_eq!(env.line(&k, on_time).interest_accrued(p.term_end + YEAR), Some(0), "4.0 at 10% for one second rounds to nothing");

    // At the term the window has closed, whether or not anything was drawn.
    env.warp(p.term_end);
    let ix = env.draw_ix(&k, late, &borrower.pubkey());
    env.expect_err(ix, &borrower, &[], E_AVAILABILITY_ENDED);
    assert_eq!(env.line(&k, late).status, LineStatus::Approved);
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
}

/// `approve_line` refuses a principal above `max_line_bps` of the NAV at the approval
/// (`LineTooLarge`). The NAV counts principal lent out, so drawing one line does not shrink the
/// room for the next; a pool with nothing deposited approves no line at all.
#[test]
fn approve_line_refuses_a_principal_above_max_line_bps_of_the_nav() {
    let mut env = Env::new();
    let council = env.council.insecure_clone();
    let mint = env.create_mint(&council.pubkey(), 6);
    let (k, ix) = env.init_pool_full(&mint, 1_000); // 10% of the NAV per line
    env.send(ix, &council, &[]);
    let borrower = env.borrower(&k);
    let refused = |env: &mut Env, p: &LineParams| {
        let (id, _) = env.request_line_with(&k, &borrower, p, None);
        let hash = env.terms_hash(&k, id, &borrower.pubkey(), p);
        let ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
        env.expect_err(ix, &council, &[], E_LINE_TOO_LARGE);
        assert_eq!(env.line(&k, id).status, LineStatus::Requested);
    };

    // NAV 0: nothing can be approved, however small.
    let p = env.line_params(1, 0);
    refused(&mut env, &p);

    // NAV 10.0: the cap is exactly 1.0.
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let p = env.line_params(UNIT + 1, 0);
    refused(&mut env, &p);
    let first = env.open_line(&k, &borrower, UNIT, 0);
    env.draw(&k, first, &borrower);

    // Drawn principal is still in the NAV (9.0 in the vault + 1.0 outstanding), so a second
    // line of 1.0 is approved; 1.0 + 1 is not.
    assert_eq!(env.nav(&k), 10 * UNIT);
    let p = env.line_params(UNIT + 1, 0);
    refused(&mut env, &p);
    env.open_line(&k, &borrower, UNIT, 0);

    // A wrong vault account is refused before the cap is measured.
    let p = env.line_params(UNIT, 0);
    let (id, _) = env.request_line_with(&k, &borrower, &p, None);
    let hash = env.terms_hash(&k, id, &borrower.pubkey(), &p);
    let mut ix = env.approve_line_ix(&k, &council.pubkey(), id, p.expediente_root, hash);
    let vault_index = ix.accounts.iter().position(|m| m.pubkey == k.vault).expect("vault meta");
    ix.accounts[vault_index].pubkey = ata(&investor.pubkey(), &k.asset_mint);
    env.expect_err(ix, &council, &[], E_WRONG_ACCOUNT);
}

/// The first tranche is the only one to flip `Approved -> Active`; later tranches keep
/// `drawn_at` (informational) and the status.
#[test]
fn draw_state_follows_the_tranches() {
    let mut env = Env::new();
    let (k, _) = pool_with(&mut env, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.line_params(4 * UNIT, 1_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);

    let ev = env.draw_amount(&k, line_id, &borrower, UNIT);
    assert_eq!((ev.amount_mxn, ev.drawn, ev.available), (UNIT, UNIT, 3 * UNIT));
    assert_eq!((ev.availability_end, ev.draw_index), (p.term_end, 0));
    let t_first = env.now();
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.drawn_at, line.draws), (LineStatus::Active, t_first, 1));

    env.warp(t_first + 1_000);
    let ev: LineDrawn = env.draw_amount(&k, line_id, &borrower, 3 * UNIT);
    assert_eq!((ev.drawn, ev.available), (4 * UNIT, 0));
    let line = env.line(&k, line_id);
    assert_eq!((line.status, line.drawn_at, line.draws), (LineStatus::Active, t_first, 2));
    // To the term: 1.0 for a year and 3.0 for a year less 1,000 seconds at 10%, rounded once.
    let expected = (1_000_000u128 * YEAR as u128 + 3_000_000 * (YEAR as u128 - 1_000)) * 1_000 / (10_000 * YEAR as u128);
    assert_eq!(expected, 399_990);
    assert_eq!(line.interest_accrued(p.term_end), Some(399_990));
    assert_eq!(env.pool(&k).outstanding_principal, 4 * UNIT);
}

