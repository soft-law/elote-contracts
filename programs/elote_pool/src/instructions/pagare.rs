//! The pagaré after its issuance (issuance is `draw`, in `lines.rs`). Extinction is an act of the
//! holder against a receipt (C22; LGTOC 17, 129): the holder and the attestor sign, the sidecar
//! becomes the lasting PAGADO mark, and the asset is thawed and burnt last.

use anchor_lang::prelude::*;

use crate::errors::PoolError;
use crate::events::{CancellationRequested, PagareCancelled};
use crate::pool_signer;
use crate::registry;
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, KIND_ANOTACION_JUDICIAL, KIND_CONSTANCIA_JUDICIAL, KIND_PROCURACION,
    KIND_RECIBO, KIND_REVOCACION, KIND_SOLICITUD_CANCELACION, KIND_TERMINACION, PAGARE_FLAG_BURN_DELEGATE,
    PAGARE_FLAG_PROCURACION, PAGARE_OUTSTANDING, PAGARE_PAID, PAGARE_PAID_PENDING_CANCEL,
};
use crate::events::{PagareEntry, PagareReceiptAnnotated};

// -----------------------------------------------------------------------------------------------
// cancel_pagare
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct CancelPagare<'info> {
    /// Who extinguishes the title: the holder (the admin while the asset is in the pool) or the
    /// procurador of record (`Pagare::procurador`, with a procuración in force); checked in the
    /// handler (`NotHolder`).
    pub holder: Signer<'info>,

    /// Attests the receipt behind `receipt_hash` and that the asset's log was archived. Optional
    /// for the backstop: left out, the holder cancels alone only after a recorded request's
    /// deadline, with the requested receipt (`AttestorRequired` otherwise).
    pub attestor: Option<Signer<'info>>,

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

    /// The program address the burn delegate was approved to, `["procurador", pagare, holder]`:
    /// required on the procurador's branch, where the program signs for it; left out on the
    /// owner's.
    /// CHECK: derived and compared in the handler (`WrongAccount`).
    pub procurador_authority: Option<UncheckedAccount<'info>>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Two branches by the signer: the owner (the admin while the asset is in the pool) or the
/// procurador of record (`Pagare::procurador` with the procuración in force, else `NotHolder`;
/// on that branch `procurador_authority` must be the program address the delegate was approved
/// to, `WrongAccount`). The attestor co-signs or, on the owner's branch alone, a recorded
/// request's deadline has passed and `receipt_hash` is the requested one (the backstop on the
/// attestor, LGTOC 17 and 129; `AttestorRequired` otherwise). Checks, all before any write: the
/// pagaré is outstanding (`PagareAlreadyPaid`); `receipt_hash` is not zero
/// (`DocumentHashRequired`); the title carries the PAGADO mark a repayment wrote,
/// `PAGARE_PAID_PENDING_CANCEL`, whatever the line's status
/// (`PagareNotCovered` while outstanding: `repay` marks the pagarés oldest first as its principal
/// reaches each `drawn_through`, interest covering nothing by itself). Writes the `Terminacion`
/// entry (from the signer's role, to the pool), `status = PAGARE_PAID`, `cancel_receipt_hash`
/// (the payment's `paid_at` and `receipt_hash` stay), and ends any procuración in force with the
/// title; then thaws and burns, as the owner or through the procurador's program address. Receipt
/// uniqueness is the engine's and the attestor's, not checked here. Never paused: it records a
/// payment and moves no funds.
pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let pagare = &ctx.accounts.pagare;
    let signer = ctx.accounts.holder.key();
    let pagare_key = pagare.key();
    let by_procurador = signer != pool.admin;
    if by_procurador {
        require!(signer == pagare.procurador, PoolError::NotHolder);
        require!(pagare.flags & PAGARE_FLAG_PROCURACION != 0, PoolError::ProcuracionState);
    }
    require!(pagare.status != PAGARE_PAID, PoolError::PagareAlreadyPaid);
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    require!(pagare.status == PAGARE_PAID_PENDING_CANCEL, PoolError::PagareNotCovered);
    let now = Clock::get()?.unix_timestamp;
    // The attestor, or the backstop: the holder alone, once a recorded request's deadline has
    // passed, with the requested receipt.
    match ctx.accounts.attestor.as_ref() {
        Some(attestor) => {
            require_keys_eq!(attestor.key(), pool.attestor, PoolError::AttestorRequired);
        }
        None => {
            require!(
                !by_procurador && pagare.cancel_after != 0 && now > pagare.cancel_after && receipt_hash == pagare.requested_receipt_hash,
                PoolError::AttestorRequired
            );
        }
    }
    // The procurador's program address, derived and compared before any write.
    let authority_bump = if by_procurador {
        let (pda, bump) = Pubkey::find_program_address(&[seeds::PROCURADOR, pagare_key.as_ref(), signer.as_ref()], ctx.program_id);
        let given = ctx.accounts.procurador_authority.as_ref().ok_or(PoolError::WrongAccount)?;
        require_keys_eq!(given.key(), pda, PoolError::WrongAccount);
        Some(bump)
    } else {
        None
    };

    let pool_key = pool.key();
    let from = if by_procurador { signer } else { pool_key };
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_TERMINACION, &from, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    pagare.status = PAGARE_PAID;
    pagare.cancel_receipt_hash = receipt_hash;
    // Any cancellation clears a pending request.
    pagare.cancel_after = 0;
    pagare.requested_receipt_hash = [0; 32];
    // The title's extinction ends a procuración in force; the plugin flag stays as history.
    pagare.flags &= !PAGARE_FLAG_PROCURACION;
    pagare.procurador = Pubkey::default();
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;

    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id: line.line_id,
        draw_index,
        asset: asset_key,
        kind: KIND_TERMINACION,
        from,
        to: pool_key,
        entry_hash: hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(PagareCancelled {
        pool: pool_key,
        line_id: line.line_id,
        draw_index,
        asset: asset_key,
        receipt_hash,
        holder: signer,
        backstop: ctx.accounts.attestor.is_none(),
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
    let core = ctx.accounts.mpl_core_program.to_account_info();
    let asset_info = ctx.accounts.asset.to_account_info();
    let collection_info = ctx.accounts.collection.to_account_info();
    let payer_info = ctx.accounts.payer.to_account_info();
    let system_info = ctx.accounts.system_program.to_account_info();
    let pool_info = ctx.accounts.pool.to_account_info();
    let mut burn = mpl_core::instructions::BurnV1CpiBuilder::new(&core);
    burn.asset(&asset_info).collection(Some(&collection_info)).payer(&payer_info).system_program(Some(&system_info));
    match authority_bump {
        // The owner burns.
        None => burn.authority(Some(&pool_info)).invoke_signed(pool_signer!(pool))?,
        // The program signs for the procurador's address, which holds the burn delegate.
        Some(bump) => {
            let authority = ctx.accounts.procurador_authority.as_ref().ok_or(PoolError::WrongAccount)?.to_account_info();
            burn.authority(Some(&authority)).invoke_signed(&[&[seeds::PROCURADOR, pagare_key.as_ref(), signer.as_ref(), &[bump]]])?
        }
    };
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// endorse_pagare
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct EndorsePagare<'info> {
    /// The pagaré's holder: the admin while the asset is in the pool.
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// Attests the signed endoso behind `endoso_hash` and the endorsee's verification (v1 gate).
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    /// Pays the rent the Core plugin may need. Any signer.
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

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,

    /// CHECK: must be `pagare.asset`; the Core program checks the rest.
    #[account(mut, address = pagare.asset @ PoolError::WrongAccount)]
    pub asset: UncheckedAccount<'info>,

    /// CHECK: must be `pool.collection`; the Core program checks the rest.
    #[account(mut, address = pool.collection @ PoolError::WrongAccount)]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Endoso en procuración (LGTOC 35), the only kind the pool makes in this version: the pool stays
/// the owner; the entry goes into the registry; the power to extinguish the pagaré is lent through
/// an owner-managed `BurnDelegate` whose authority is the program PDA `["procurador", pagare, to]`,
/// which nobody holds a key for, so the burn can only happen inside `cancel_pagare` with the
/// receipt and the PAGADO mark. Checks, before any write: `kind == KIND_PROCURACION`
/// (`EndosoKindInvalid`); `to` is not the zero key, the pool, the attestor or the admin (`EndorseeInvalid`);
/// `endoso_hash != 0` (`DocumentHashRequired`); the line is not `WrittenOff` (`LineStatus`); the
/// pagaré is outstanding (`PagareAlreadyPaid`); no procuración is in force (`ProcuracionState`).
/// The plugin is added the first time and its authority approved afterwards (Core refuses a second
/// `AddPluginV1`), which the sidecar's flag records.
pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32]) -> Result<()> {
    use mpl_core::instructions::{AddPluginV1CpiBuilder, ApprovePluginAuthorityV1CpiBuilder};
    use mpl_core::types::{BurnDelegate, Plugin, PluginAuthority, PluginType};
    let pool = &ctx.accounts.pool;
    let pool_key = pool.key();
    require!(kind == KIND_PROCURACION, PoolError::EndosoKindInvalid);
    require!(
        to != Pubkey::default() && to != pool_key && to != pool.attestor && to != pool.admin,
        PoolError::EndorseeInvalid
    );
    require!(endoso_hash != [0u8; 32], PoolError::DocumentHashRequired);
    require!(ctx.accounts.line.status != LineStatus::WrittenOff, PoolError::LineStatus);
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status == PAGARE_OUTSTANDING, PoolError::PagareAlreadyPaid);
    require!(pagare.flags & PAGARE_FLAG_PROCURACION == 0, PoolError::ProcuracionState);

    let now = Clock::get()?.unix_timestamp;
    let pagare_key = pagare.key();
    let (delegate, _) = Pubkey::find_program_address(&[seeds::PROCURADOR, pagare_key.as_ref(), to.as_ref()], &crate::ID);
    let plugin_exists = pagare.flags & PAGARE_FLAG_BURN_DELEGATE != 0;
    let pagare = &mut ctx.accounts.pagare;
    let entry_hash = registry::append(pagare, kind, &pool_key, &to, &endoso_hash, now).ok_or(PoolError::Overflow)?;
    pagare.flags |= PAGARE_FLAG_BURN_DELEGATE | PAGARE_FLAG_PROCURACION;
    pagare.procurador = to;
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;

    let pool = &ctx.accounts.pool;
    let core = ctx.accounts.mpl_core_program.to_account_info();
    let asset = ctx.accounts.asset.to_account_info();
    let collection = ctx.accounts.collection.to_account_info();
    let payer = ctx.accounts.payer.to_account_info();
    let authority = ctx.accounts.pool.to_account_info();
    let system = ctx.accounts.system_program.to_account_info();
    if plugin_exists {
        ApprovePluginAuthorityV1CpiBuilder::new(&core)
            .asset(&asset)
            .collection(Some(&collection))
            .payer(&payer)
            .authority(Some(&authority))
            .system_program(&system)
            .plugin_type(PluginType::BurnDelegate)
            .new_authority(PluginAuthority::Address { address: delegate })
            .invoke_signed(pool_signer!(pool))?;
    } else {
        AddPluginV1CpiBuilder::new(&core)
            .asset(&asset)
            .collection(Some(&collection))
            .payer(&payer)
            .authority(Some(&authority))
            .system_program(&system)
            .plugin(Plugin::BurnDelegate(BurnDelegate {}))
            .init_authority(PluginAuthority::Address { address: delegate })
            .invoke_signed(pool_signer!(pool))?;
    }
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id: ctx.accounts.line.line_id,
        draw_index,
        asset: asset_key,
        kind,
        from: pool_key,
        to,
        entry_hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// revoke_procuracion
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct RevokeProcuracion<'info> {
    /// The pagaré's holder: the admin while the asset is in the pool. The owner alone revokes.
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

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

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,

    /// CHECK: must be `pagare.asset`; the Core program checks the rest.
    #[account(mut, address = pagare.asset @ PoolError::WrongAccount)]
    pub asset: UncheckedAccount<'info>,

    /// CHECK: must be `pool.collection`; the Core program checks the rest.
    #[account(mut, address = pool.collection @ PoolError::WrongAccount)]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// The revocation of the procuración in force (LGTOC 41): the owner alone, over the signed
/// revocation's hash (`DocumentHashRequired`); refused when none is in force (`ProcuracionState`).
/// Appends the `Revocacion` entry (from the pool, to the recorded procurador), clears the flag and
/// the procurador, and `RevokePluginAuthorityV1` puts the `BurnDelegate` back to `Owner`; the
/// plugin stays, so the next procuración approves it.
pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32]) -> Result<()> {
    use mpl_core::instructions::RevokePluginAuthorityV1CpiBuilder;
    use mpl_core::types::PluginType;
    require!(entry_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &ctx.accounts.pagare;
    require!(pagare.flags & PAGARE_FLAG_PROCURACION != 0, PoolError::ProcuracionState);
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let to = pagare.procurador;
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_REVOCACION, &pool_key, &to, &entry_hash, now).ok_or(PoolError::Overflow)?;
    pagare.flags &= !PAGARE_FLAG_PROCURACION;
    pagare.procurador = Pubkey::default();
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;

    let pool = &ctx.accounts.pool;
    RevokePluginAuthorityV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.payer.to_account_info())
        .authority(Some(&ctx.accounts.pool.to_account_info()))
        .system_program(&ctx.accounts.system_program.to_account_info())
        .plugin_type(PluginType::BurnDelegate)
        .invoke_signed(pool_signer!(pool))?;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id: ctx.accounts.line.line_id,
        draw_index,
        asset: asset_key,
        kind: KIND_REVOCACION,
        from: pool_key,
        to,
        entry_hash: hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// court_annotation
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct CourtAnnotation<'info> {
    /// The pagaré's holder: the admin while the asset is in the pool.
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// Attests the court document behind `document_hash`.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

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

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,
}

/// A court's act on the title: `AnotacionJudicial` (embargo, suspension, cancellation) or
/// `ConstanciaJudicial` (LGTOC 28), over the court document's hash, attested by the attestor.
/// Appends the entry (from and to the pool: there is no counterparty) and emits it; nothing else
/// moves. Allowed on any line status and beside a procuración; refused only on a burnt title,
/// which the sidecar records as `PAGARE_PAID` (the burn happens only inside `cancel_pagare`).
pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32]) -> Result<()> {
    require!(kind == KIND_ANOTACION_JUDICIAL || kind == KIND_CONSTANCIA_JUDICIAL, PoolError::AnnotationKindInvalid);
    require!(document_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &mut ctx.accounts.pagare;
    require!(pagare.status != PAGARE_PAID, PoolError::PagareAlreadyPaid);
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let hash = registry::append(pagare, kind, &pool_key, &pool_key, &document_hash, now).ok_or(PoolError::Overflow)?;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id: ctx.accounts.line.line_id,
        draw_index,
        asset: pagare.asset,
        kind,
        from: pool_key,
        to: pool_key,
        entry_hash: hash,
        entry_count: pagare.entry_count,
        recorded_at: now,
        stated_at: now,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// request_cancellation
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct RequestCancellation<'info> {
    /// The pagaré's holder: the admin while the asset is in the pool.
    #[account(constraint = holder.key() == pool.admin @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

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

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,
}

/// The backstop's request (LGTOC 17, 129): the holder records, over the receipt's hash, that the
/// title is to be cancelled, under the same checks as the cancellation itself (the PAGADO mark a
/// repayment wrote), so no request precedes the payment. Appends the `SolicitudCancelacion` entry
/// and fixes the deadline, `cancel_after = now + pool.cancel_delay_seconds`, which a later change
/// of the delay never moves. A new request replaces a pending one (its receipt, a fresh wait) but
/// never brings the deadline earlier than the one it replaces. Any cancellation clears the
/// request.
pub fn request_cancellation(ctx: Context<RequestCancellation>, draw_index: u16, receipt_hash: [u8; 32]) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status != PAGARE_PAID, PoolError::PagareAlreadyPaid);
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    require!(pagare.status == PAGARE_PAID_PENDING_CANCEL, PoolError::PagareNotCovered);
    let now = Clock::get()?.unix_timestamp;
    let deadline = now
        .checked_add(i64::from(pool.cancel_delay_seconds))
        .ok_or(PoolError::Overflow)?
        .max(pagare.cancel_after);
    let pool_key = pool.key();
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_SOLICITUD_CANCELACION, &pool_key, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    pagare.cancel_after = deadline;
    pagare.requested_receipt_hash = receipt_hash;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id: line.line_id,
        draw_index,
        asset: pagare.asset,
        kind: KIND_SOLICITUD_CANCELACION,
        from: pool_key,
        to: pool_key,
        entry_hash: hash,
        entry_count: pagare.entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(CancellationRequested {
        pool: pool_key,
        line_id: line.line_id,
        draw_index,
        asset: pagare.asset,
        receipt_hash,
        cancel_after: deadline,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// annotate_payment
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct AnnotatePayment<'info> {
    /// The holder (the admin while the asset is in the pool) or the attestor.
    #[account(constraint = annotator.key() == pool.admin || annotator.key() == pool.attestor @ PoolError::Unauthorized)]
    pub annotator: Signer<'info>,

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

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Account<'info, Pagare>,
}

/// The repair path of the payment entries: the holder or the attestor attaches the real receipt
/// to the payment entry `entry_count` of the pagaré's chain (one that exists: `EntryNotFound`),
/// as a `Recibo` entry over `receipt_hash` (`DocumentHashRequired`), from the annotator to the
/// pool. The chain does not keep the kinds of past entries, so the reference is not checked to be
/// a payment entry beyond existing; the engine checks it. Allowed whatever the title's status, a
/// burnt one included: the sidecar persists and a receipt may arrive late. The payer's
/// `receipt_hash` on the PAGADO mark is provisional: the holder's annotation replaces it while the
/// burn is still owed (`PAGARE_PAID_PENDING_CANCEL`); after the burn it stays the receipt the
/// cancellation rested on, and the attestor's annotation never touches it.
pub fn annotate_payment(ctx: Context<AnnotatePayment>, draw_index: u16, entry_count: u32, receipt_hash: [u8; 32]) -> Result<()> {
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &mut ctx.accounts.pagare;
    require!(entry_count >= 1 && entry_count <= pagare.entry_count, PoolError::EntryNotFound);
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let annotator = ctx.accounts.annotator.key();
    let hash = registry::append(pagare, KIND_RECIBO, &annotator, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    if annotator == ctx.accounts.pool.admin && pagare.status == PAGARE_PAID_PENDING_CANCEL {
        pagare.receipt_hash = receipt_hash;
    }
    let line_id = ctx.accounts.line.line_id;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id,
        draw_index,
        asset: pagare.asset,
        kind: KIND_RECIBO,
        from: annotator,
        to: pool_key,
        entry_hash: hash,
        entry_count: pagare.entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(PagareReceiptAnnotated {
        pool: pool_key,
        line_id,
        draw_index,
        entry_count: pagare.entry_count,
        payment_entry_count: entry_count,
        receipt_hash,
    });
    Ok(())
}
