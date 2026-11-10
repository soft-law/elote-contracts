//! The account layouts: their final sizes, and the reserved tail every account is created with.

mod common;

use common::*;
use anchor_lang::Space;
use elote_pool::state::{CreditLine, Pagare, Pool};
use solana_signer::Signer;

/// The sizes are fixed for mainnet. Each account ends in reserved bytes for fields added later,
/// created zero, so zero there always means "no value".
#[test]
fn account_sizes_are_fixed_and_reserved_starts_zero() {
    assert_eq!(8 + Pool::INIT_SPACE, 495);
    assert_eq!(8 + CreditLine::INIT_SPACE, 528);
    assert_eq!(8 + Pagare::INIT_SPACE, 363);

    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let p = env.scheduled_params(4 * UNIT, 1_200, 6, 5_000);
    let (line_id, _) = env.open_line_with(&k, &borrower, &p, None);
    env.draw_amount(&k, line_id, &borrower, UNIT);

    let pool = env.pool(&k);
    let line = env.line(&k, line_id);
    let pagare = env.pagare(&k, line_id, 0);
    assert_eq!(pool.reserved, [0u8; 48]);
    assert_eq!((pool.min_fix_rate, pool.max_fix_rate), (0, 0), "no FIX bounds unless set");
    assert_eq!((pool.impaired_principal, pool.provision_bps), (0, 10_000), "nothing impaired yet; the provision starts at 100% (decision 40)");
    assert_eq!((line.principal_seconds, line.accrued_at), (0, env.now()), "the accrual starts at the first draw (decision 48)");
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (0, 0));
    assert_eq!(line.reserved, [0u8; 64]);
    assert_eq!(pagare.reserved, [0u8; 28]);
    for (key, len) in [
        (k.pool, 8 + Pool::INIT_SPACE),
        (line_pda(&k.pool, line_id).0, 8 + CreditLine::INIT_SPACE),
        (pagare_pda(&line_pda(&k.pool, line_id).0, 0).0, 8 + Pagare::INIT_SPACE),
    ] {
        assert_eq!(env.account(&key).data.len(), len);
    }
}

/// Every field of a `CreditLine` sits at one offset, sponsor or none: `sponsor` is a key, the zero
/// key meaning "none", as `procurador` and `fx_feeder` are. `status` is at byte 177 on both lines.
#[test]
fn a_line_has_one_layout_with_or_without_a_sponsor() {
    let mut env = Env::new();
    let (k, _) = env.setup_pool();
    let investor = env.investor(&k, 10 * UNIT);
    env.deposit(&k, &investor, 10 * UNIT);
    let borrower = env.borrower(&k);
    let sponsor = env.funded(1);
    let p = env.line_params(UNIT, 1_000);
    let (plain, _) = env.open_line_with(&k, &borrower, &p, None);
    let mut sp = p;
    sp.sponsor = Some(sponsor.pubkey());
    let (sponsored, _) = env.open_line_with(&k, &borrower, &sp, Some(&sponsor));
    for (line_id, key) in [(plain, solana_pubkey::Pubkey::default()), (sponsored, sponsor.pubkey())] {
        let data = env.account(&line_pda(&k.pool, line_id).0).data;
        assert_eq!(&data[80..112], key.as_ref(), "the sponsor, zero when none");
        assert_eq!(data[177], 1, "status Approved at byte 177");
        assert_eq!(&data[145..177], &EXPEDIENTE_ROOT, "expediente_root at byte 145");
    }
}
