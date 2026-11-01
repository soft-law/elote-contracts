//! The pagaré after its issuance (issuance is `draw`, in `lines.rs`). Extinction is an act of the
//! holder against a receipt (C22; LGTOC 17, 129): the holder and the attestor sign, the sidecar
//! becomes the lasting PAGADO mark, and the asset is thawed and burnt last.

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
    /// The pagaré's holder: the admin while the asset is in the pool.
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// Attests the receipt behind `receipt_hash` and that the AppData log was archived.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    /// Receives the asset's rent from the Core program. Any signer.
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

/// Checks, all before any write: the pagaré is outstanding (`PagareAlreadyPaid`); `receipt_hash`
/// is not zero (`DocumentHashRequired`); the line is `Active` or `Repaid` (`LineStatus`: in
/// collection or written off the títulos stay in force); `principal_repaid >= drawn_through`
/// (`PagareNotCovered`). Coverage counts principal only, oldest pagaré first: `repay` settles
/// interest before principal, so when the principal of pagarés `0..=k` is repaid every unit of
/// interest fixed up to that payment is too, and a título never goes while its principal is owed.
/// Receipt uniqueness is the engine's and the attestor's, not checked here. Never paused: it
/// records a payment and moves no funds.
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

    // Last, so a Core refusal leaves nothing half done: the pool PDA thaws the asset as the
    // collection's authority (a frozen asset cannot be burnt), then burns it as its owner; the
    // rent goes to `payer`.
    mpl_core::instructions::UpdatePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.payer.to_account_info())
        .authority(Some(&ctx.accounts.pool.to_account_info()))
        .system_program(&ctx.accounts.system_program.to_account_info())
        .plugin(mpl_core::types::Plugin::PermanentFreezeDelegate(mpl_core::types::PermanentFreezeDelegate { frozen: false }))
        .invoke_signed(pool_signer!(pool))?;
    mpl_core::instructions::BurnV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.payer.to_account_info())
        .authority(Some(&ctx.accounts.pool.to_account_info()))
        .system_program(Some(&ctx.accounts.system_program.to_account_info()))
        .invoke_signed(pool_signer!(pool))?;
    Ok(())
}
