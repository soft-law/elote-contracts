//! The pagaré after its issuance: `cancel_pagare` extinguishes a paid pagaré (step 6c); the endoso
//! (`endorse_pagare`, step 6d) will live here too. Issuance stays in `lines.rs`, inside `draw`.
//!
//! Per the legal brief of 2026-11-01 (C22; LGTOC 17 and 129; CCF 2088), extinction is an act of
//! the holder against a receipt, not a permissionless burn: the holder and the attestor sign, the
//! sidecar becomes the lasting "PAGADO" mark, and the asset is burnt last through the permanent
//! burn delegate the pool PDA holds.

use anchor_lang::prelude::*;

use crate::errors::PoolError;
use crate::events::PagareCancelled;
use crate::pool_signer;
use crate::state::{seeds, CreditLine, LineStatus, Pagare, Pool, PAGARE_OUTSTANDING, PAGARE_PAID};

// -----------------------------------------------------------------------------------------------
// cancel_pagare
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct CancelPagare<'info> {
    /// The pagaré's current holder. While the asset is in the pool that is the admin, the
    /// vehicle's signer (step 6d extends this to the holder of record after an endoso).
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// The pool's attestor: the agreements engine attests the receipt behind `receipt_hash` was
    /// signed and is conserved, and that the AppData log was archived before the burn.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    /// Receives the asset's rent from the Core program when it is burnt (the relayer that funded
    /// it at the draw, as a rule). Any signer.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Account<'info, CreditLine>,

    /// The sidecar of pagaré `draw_index` of the line; it is written, never closed.
    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,

    /// The Core asset the sidecar names; burnt by the Core program.
    /// CHECK: must be `pagare.asset`; the Core program checks the rest.
    #[account(mut, address = pagare.asset @ PoolError::WrongAccount)]
    pub asset: UncheckedAccount<'info>,

    /// The series' collection the asset belongs to.
    /// CHECK: must be `pool.collection`; the Core program checks the rest.
    #[account(mut, address = pool.collection @ PoolError::WrongAccount)]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Extinguishes pagaré `draw_index` of the line against the FEA-signed receipt whose hash is
/// `receipt_hash` (LGTOC 129). Checks, all before any write: the pagaré is outstanding
/// (`PagareAlreadyPaid`); `receipt_hash` is not zero (`DocumentHashRequired`); the line is
/// `Active` or `Repaid` (`LineStatus`: in collection or written off the títulos stay in force);
/// the coverage rule, `principal_repaid >= drawn_through` (`PagareNotCovered`): the principal of
/// pagarés `0..=k` is repaid, and since `repay` settles interest before principal, so is every unit
/// of interest fixed up to that payment. Repayments cover the pagarés oldest first, so a later one
/// cannot be cancelled while an earlier one is uncovered. Interest alone covers nothing: a título
/// is never extinguished while any of its principal is owed. The program does not check that the
/// receipt is distinct from other receipts; the engine issues one per pagaré and the attestor's
/// signature is the guard. Writes the "PAGADO" mark (`status`, `paid_at`, `receipt_hash`), emits
/// `PagareCancelled`, then burns the asset through the permanent burn delegate, last. Not blocked
/// by the pause: it records a payment and moves no funds.
///
/// The event carries no AppData length: reading it back would mean decoding the asset's external
/// adapter header on chain (§5 fixed-offset rule), and the engine archives the log (NOM-151)
/// before it asks for the burn.
pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status == PAGARE_OUTSTANDING, PoolError::PagareAlreadyPaid);
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    require!(matches!(line.status, LineStatus::Active | LineStatus::Repaid), PoolError::LineStatus);
    require!(line.principal_repaid >= pagare.drawn_through, PoolError::PagareNotCovered);

    let now = Clock::get()?.unix_timestamp;
    let pagare = &mut ctx.accounts.pagare;
    pagare.status = PAGARE_PAID;
    pagare.paid_at = now;
    pagare.receipt_hash = receipt_hash;

    emit_cpi!(PagareCancelled {
        pool: pool.key(),
        line_id: line.line_id,
        draw_index,
        asset: ctx.accounts.asset.key(),
        receipt_hash,
        holder: ctx.accounts.holder.key(),
    });

    // Last, so a Core refusal leaves nothing half done: the pool PDA burns as the permanent burn
    // delegate (it is also the owner while the asset is in the pool), the rent goes to `payer`.
    mpl_core::instructions::BurnV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.payer.to_account_info())
        .authority(Some(&ctx.accounts.pool.to_account_info()))
        .system_program(Some(&ctx.accounts.system_program.to_account_info()))
        .invoke_signed(pool_signer!(pool))?;
    Ok(())
}
