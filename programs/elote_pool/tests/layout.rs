//! The account layouts: their final sizes, and the reserved tail every account is created with.

mod common;

use common::*;
use anchor_lang::Space;
use elote_pool::state::{CreditLine, Pagare, Pool};

/// The sizes are fixed for mainnet. Each account ends in reserved bytes for fields added later,
/// created zero, so zero there always means "no value".
#[test]
fn account_sizes_are_fixed_and_reserved_starts_zero() {
    assert_eq!(8 + Pool::INIT_SPACE, 527);
    assert_eq!(8 + CreditLine::INIT_SPACE, 529);
    assert_eq!(8 + Pagare::INIT_SPACE, 399);

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
    assert_eq!(pool.reserved, [0u8; 64]);
    assert_eq!((pool.impaired_principal, pool.provision_bps), (0, 0), "room for the provision at the mark, unused");
    assert_eq!(line.draw_time_weight, u128::from(UNIT) * env.now() as u128, "amount times draw time");
    assert_eq!((line.acceleration_due_at, line.availability_end_saved), (0, 0));
    assert_eq!(line.reserved, [0u8; 64]);
    assert_eq!(pagare.reserved, [0u8; 28]);
    assert_eq!(pagare.cancel_grace_seconds, 0, "fixed at payment, zero while outstanding");
    for (key, len) in [
        (k.pool, 8 + Pool::INIT_SPACE),
        (line_pda(&k.pool, line_id).0, 8 + CreditLine::INIT_SPACE),
        (pagare_pda(&line_pda(&k.pool, line_id).0, 0).0, 8 + Pagare::INIT_SPACE),
    ] {
        assert_eq!(env.account(&key).data.len(), len);
    }
}
