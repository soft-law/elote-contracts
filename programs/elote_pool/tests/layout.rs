//! The account layouts: their final sizes, and the reserved tail every account is created with.

mod common;
mod reference;

use common::*;
use anchor_lang::Space;
use elote_pool::state::{CreditLine, Pagare, Pool};
use solana_signer::Signer;

/// The sizes are fixed for mainnet. Each account ends in reserved bytes for fields added later,
/// created zero, so zero there always means "no value".
#[test]
fn account_sizes_are_fixed_and_reserved_starts_zero() {
    assert_eq!(8 + Pool::INIT_SPACE, 591, "495 before decision 56 added the three cotejo keys");
    assert_eq!(8 + CreditLine::INIT_SPACE, 528);
    assert_eq!(
        8 + Pagare::INIT_SPACE,
        527,
        "363 before decision 55 added the signed message's hash, signature and day; 463 before decisions 56 and 57 added the cotejo key and the sealed hash"
    );

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
    assert_eq!(pool.cotejo_keys, [solana_pubkey::Pubkey::default(); 3], "no cotejo keys until the council sets them");
    assert_eq!((pagare.cotejo, pagare.sealed_hash), (solana_pubkey::Pubkey::default(), [0u8; 32]));
    for (key, len) in [
        (k.pool, 8 + Pool::INIT_SPACE),
        (line_pda(&k.pool, line_id).0, 8 + CreditLine::INIT_SPACE),
        (pagare_pda(&line_pda(&k.pool, line_id).0, 0).0, 8 + Pagare::INIT_SPACE),
    ] {
        assert_eq!(env.account(&key).data.len(), len);
    }
}

/// A record with every byte distinct where it can be, for the offset pins below.
fn distinct_pagare() -> Pagare {
    let key = |b: u8| solana_pubkey::Pubkey::new_from_array(core::array::from_fn(|i| b.wrapping_add(i as u8)));
    Pagare {
        line: key(1),
        draw_index: 0x0203,
        asset: key(40),
        pagare_hash: [0x50; 32],
        amount_mxn: 0x1112_1314_1516_1718,
        issued_at: 0x2122_2324_2526_2728,
        due_at: 0x3132_3334_3536_3738,
        drawn_through: 0x4142_4344_4546_4748,
        cert_fingerprint: [0x60; 32],
        signature_method: 2,
        status: 0,
        paid_at: 0x5152_5354_5556_5758,
        receipt_hash: [0x70; 32],
        procurador: key(80),
        flags: 0x0a,
        log_root: [0x90; 32],
        entry_count: 0x6162_6364,
        last_kind: 5,
        bump: 254,
        fix_rate: 0x7172_7374_7576_7778,
        fix_date: 0x2026_1111,
        usdc_delivered: 0x0102_0304_0506_0708,
        delivery_hash: [0xa0; 32],
        signed_pagare_hash: [0xb0; 32],
        pagare_signature: [0xc0; 64],
        signed_on: 20_261_111,
        cotejo: key(200),
        sealed_hash: [0xd0; 32],
        reserved: [0; 28],
    }
}

/// `repay` and `condone` read a record's fields and write its payment state and registry in
/// place (`record::SidecarFields`): every offset reads what Borsh wrote, and a write changes
/// exactly those fields, every other byte untouched.
#[test]
fn the_in_place_record_view_matches_the_borsh_layout() {
    use anchor_lang::AccountSerialize;
    use elote_pool::record::SidecarFields;
    let p = distinct_pagare();
    let mut data = Vec::new();
    p.try_serialize(&mut data).unwrap();
    assert_eq!(data.len(), 8 + Pagare::INIT_SPACE);
    // Decisions 56 and 57 append after `signed_on`, at the reference's offsets.
    let (c, s) = (reference::layout::PAGARE_COTEJO, reference::layout::PAGARE_SEALED_HASH);
    assert_eq!(&data[c..c + 32], p.cotejo.as_ref());
    assert_eq!(&data[s..s + 32], &p.sealed_hash);
    assert_eq!(reference::layout::PAGARE_LEN, data.len());
    let f = SidecarFields::parse(&data).expect("a record");
    assert_eq!(
        f,
        SidecarFields {
            line: p.line,
            draw_index: p.draw_index,
            asset: p.asset,
            amount_mxn: p.amount_mxn,
            drawn_through: p.drawn_through,
            usdc_delivered: p.usdc_delivered,
            status: p.status,
            paid_at: p.paid_at,
            receipt_hash: p.receipt_hash,
            procurador: p.procurador,
            flags: p.flags,
            log_root: p.log_root,
            entry_count: p.entry_count,
            last_kind: p.last_kind,
        }
    );
    let mut g = f;
    g.status = 1;
    g.paid_at = -7;
    g.receipt_hash = [0xee; 32];
    g.procurador = solana_pubkey::Pubkey::default();
    g.flags = 0x21;
    g.log_root = [0xdd; 32];
    g.entry_count = 9;
    g.last_kind = 14;
    let mut written = data.clone();
    g.write_into(&mut written);
    let mut expected = p.clone();
    expected.status = 1;
    expected.paid_at = -7;
    expected.receipt_hash = [0xee; 32];
    expected.procurador = solana_pubkey::Pubkey::default();
    expected.flags = 0x21;
    expected.log_root = [0xdd; 32];
    expected.entry_count = 9;
    expected.last_kind = 14;
    let mut want = Vec::new();
    expected.try_serialize(&mut want).unwrap();
    assert_eq!(written, want, "the write lands on those fields and nothing else");
    // Wrong length or discriminator: not a record.
    assert!(SidecarFields::parse(&data[..data.len() - 1]).is_none());
    let mut other = data.clone();
    other[0] ^= 1;
    assert!(SidecarFields::parse(&other).is_none());
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
