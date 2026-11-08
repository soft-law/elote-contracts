//! The pagaré after its issuance (`draw`, in `lines.rs`): cancellation, endorsement in
//! procuración and its revocation, court annotations and receipt annotations, all council
//! instructions except the procurador's and the borrower's cancellations. Each appends an entry to the pagaré record's chain; the record is never closed.

use anchor_lang::prelude::*;

use crate::council;
use crate::errors::PoolError;
use crate::events::PagareCancelled;
use crate::pool_signer;
use crate::registry;
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, KIND_ANOTACION_JUDICIAL, KIND_CONSTANCIA_JUDICIAL, KIND_PROCURACION,
    KIND_RECIBO, KIND_REVOCACION, KIND_TERMINACION, PAGARE_FLAG_BURN_DELEGATE,
    PAGARE_FLAG_PROCURACION, PAGARE_OUTSTANDING, PAGARE_PAID, PAGARE_PAID_PENDING_CANCEL,
};
use crate::events::{PagareEntry, PagareReceiptAnnotated};

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct CancelPagare<'info> {
    /// The council (the owner while the asset is in the pool), the procurador of record with a
    /// procuración in force, or the line's borrower once the cancel grace has run; checked in the
    /// handler (`NotHolder`, `ProcuracionState`, `CancelGraceNotElapsed`).
    pub holder: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    /// Receives the asset's rent from the Core program. Any signer.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    /// The record of pagaré `draw_index` of the line; written, never closed.
    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Box<Account<'info, Pagare>>,

    /// The Core asset the record names; burnt by the Core program.
    /// CHECK: must be `pagare.asset`; the Core program checks the rest.
    #[account(mut, address = pagare.asset @ PoolError::WrongAccount)]
    pub asset: UncheckedAccount<'info>,

    /// The series' collection the asset belongs to.
    /// CHECK: must be `pool.collection`; the Core program checks the rest.
    #[account(mut, address = pool.collection @ PoolError::WrongAccount)]
    pub collection: UncheckedAccount<'info>,

    /// The burn delegate's authority, `["procurador", pagare, holder]`: required on the
    /// procurador's branch, where the program signs for it; left out on the owner's.
    /// CHECK: derived and compared in the handler (`WrongAccount`).
    pub procurador_authority: Option<UncheckedAccount<'info>>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Extinguishes a paid pagaré. Three signers may: the council, as the owner (with the multisig
/// check); the procurador of record alone (`NotHolder`, `ProcuracionState`), who also passes
/// `procurador_authority` (`WrongAccount`); or the line's borrower alone once
/// `now > paid_at + Pagare.cancel_grace_seconds` (`CancelGraceNotElapsed`), the grace the repayment
/// fixed on the pagaré, the debtor's exit that does not depend on the council. Requires the PAGADO mark a repayment wrote, `PAGARE_PAID_PENDING_CANCEL`,
/// whatever the line's status (`PagareAlreadyPaid`, `PagareNotCovered`), and a non-zero
/// `receipt_hash` (`DocumentHashRequired`). Writes a `Terminacion` entry, `status = PAGARE_PAID`
/// and `cancel_receipt_hash` (`paid_at` and `receipt_hash` stay), clears any procuración, then
/// thaws and burns the asset; the pool PDA burns as its owner except on the procurador's branch,
/// also under a procuración. Never paused: it moves no funds.
pub fn cancel_pagare(ctx: Context<CancelPagare>, draw_index: u16, receipt_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let pagare = &ctx.accounts.pagare;
    let signer = ctx.accounts.holder.key();
    let pagare_key = pagare.key();
    let now = Clock::get()?.unix_timestamp;
    let by_council = signer == pool.council;
    let by_borrower = !by_council && signer == line.borrower;
    let by_procurador = !by_council && !by_borrower;
    if by_council {
        council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    } else if by_procurador {
        require!(signer == pagare.procurador, PoolError::NotHolder);
        require!(pagare.flags & PAGARE_FLAG_PROCURACION != 0, PoolError::ProcuracionState);
    }
    require!(pagare.status != PAGARE_PAID, PoolError::PagareAlreadyPaid);
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    require!(pagare.status == PAGARE_PAID_PENDING_CANCEL, PoolError::PagareNotCovered);
    if by_borrower {
        let opens = pagare.paid_at.checked_add(i64::from(pagare.cancel_grace_seconds)).ok_or(PoolError::Overflow)?;
        require!(now > opens, PoolError::CancelGraceNotElapsed);
    }
    // Derived and compared before any write.
    let authority_bump = if by_procurador {
        let (pda, bump) = Pubkey::find_program_address(&[seeds::PROCURADOR, pagare_key.as_ref(), signer.as_ref()], ctx.program_id);
        let given = ctx.accounts.procurador_authority.as_ref().ok_or(PoolError::WrongAccount)?;
        require_keys_eq!(given.key(), pda, PoolError::WrongAccount);
        Some(bump)
    } else {
        None
    };

    let pool_key = pool.key();
    let from = if by_council { pool_key } else { signer };
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_TERMINACION, &from, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    pagare.status = PAGARE_PAID;
    pagare.cancel_receipt_hash = receipt_hash;
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
        by_borrower,
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
        None => burn.authority(Some(&pool_info)).invoke_signed(pool_signer!(pool))?,
        // The program signs for the procurador's address, which holds the burn delegate.
        Some(bump) => {
            let authority = ctx.accounts.procurador_authority.as_ref().ok_or(PoolError::WrongAccount)?.to_account_info();
            burn.authority(Some(&authority)).invoke_signed(&[&[seeds::PROCURADOR, pagare_key.as_ref(), signer.as_ref(), &[bump]]])?
        }
    };
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct EndorsePagare<'info> {
    /// The pagaré's holder: the council while the asset is in the pool.
    #[account(constraint = holder.key() == pool.council @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    /// Pays the rent the Core plugin may need. Any signer.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Box<Account<'info, Pagare>>,

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

/// Endoso en procuración, the only kind the pool makes: the pool stays the owner and lends the
/// power to extinguish the pagaré through an owner-managed `BurnDelegate` whose authority is the
/// program address `["procurador", pagare, to]`. Nobody holds its key, so the burn can only happen
/// inside `cancel_pagare`. Refused: `kind != KIND_PROCURACION` (`EndosoKindInvalid`); `to` the zero
/// key, the pool, the council, the pool's creator, the FIX feeder, the line's
/// borrower, rent payer or sponsor, or a member of the council's multisig (`EndorseeInvalid`); a
/// zero `endoso_hash`
/// (`DocumentHashRequired`); a `WrittenOff` line (`LineStatus`); a pagaré not outstanding
/// (`PagareAlreadyPaid`); a procuración in force (`ProcuracionState`). The plugin is added at the
/// first procuración and approved afterwards (Core refuses a second `AddPluginV1`); a flag
/// records it.
pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    use mpl_core::instructions::{AddPluginV1CpiBuilder, ApprovePluginAuthorityV1CpiBuilder};
    use mpl_core::types::{BurnDelegate, Plugin, PluginAuthority, PluginType};
    let pool = &ctx.accounts.pool;
    let pool_key = pool.key();
    require!(kind == KIND_PROCURACION, PoolError::EndosoKindInvalid);
    let multisig = ctx.accounts.multisig.to_account_info();
    council::check(&multisig, &pool.council, pool.council_vault_bump)?;
    let line = &ctx.accounts.line;
    require!(
        to != Pubkey::default()
            && to != pool_key
            && to != pool.council
            && to != pool.creator
            && to != pool.fx_feeder
            && to != line.borrower
            && to != line.payer
            && line.sponsor != Some(to)
            && !council::is_member(&multisig, &to)?,
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

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct RevokeProcuracion<'info> {
    /// The pagaré's holder: the council while the asset is in the pool. The owner alone revokes.
    #[account(constraint = holder.key() == pool.council @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Box<Account<'info, Pagare>>,

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

/// Revokes the procuración in force: the owner alone, over the signed revocation's hash
/// (`DocumentHashRequired`); refused when none is in force (`ProcuracionState`). Appends a
/// `Revocacion` entry (from the pool to the procurador) and returns the `BurnDelegate` authority
/// to the owner; the plugin stays, so the next procuración approves it.
pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    use mpl_core::instructions::RevokePluginAuthorityV1CpiBuilder;
    use mpl_core::types::PluginType;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
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

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct CourtAnnotation<'info> {
    /// The pagaré's holder: the council while the asset is in the pool.
    #[account(constraint = holder.key() == pool.council @ PoolError::NotHolder)]
    pub holder: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Box<Account<'info, Pagare>>,
}

/// A court's act on the title, `AnotacionJudicial` or `ConstanciaJudicial` (`AnnotationKindInvalid`),
/// over the court document's hash; the council signs. Appends the entry
/// (from and to the pool: there is no counterparty); nothing else moves. Allowed on any line
/// status and beside a procuración; refused only on a burnt title (`PAGARE_PAID`).
pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
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

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct AnnotatePayment<'info> {
    /// The holder: the council while the asset is in the pool.
    #[account(constraint = annotator.key() == pool.council @ PoolError::Unauthorized)]
    pub annotator: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &draw_index.to_le_bytes()],
        bump = pagare.bump,
        has_one = line @ PoolError::WrongAccount,
    )]
    pub pagare: Box<Account<'info, Pagare>>,
}

/// The council (the holder) attaches the real receipt to payment entry `entry_count` of
/// the pagaré's chain (`EntryNotFound` if absent) by appending a `Recibo` entry over `receipt_hash`
/// (`DocumentHashRequired`), from the annotator to the pool. The chain keeps no entry kinds, so the
/// reference is not checked to be a payment entry; the engine checks it. Allowed on any status, a
/// burnt title included: a receipt may arrive late. The annotation also replaces the payer's
/// provisional `Pagare.receipt_hash` while the status is `PAGARE_PAID_PENDING_CANCEL`.
pub fn annotate_payment(ctx: Context<AnnotatePayment>, draw_index: u16, entry_count: u32, receipt_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    require!(receipt_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &mut ctx.accounts.pagare;
    require!(entry_count >= 1 && entry_count <= pagare.entry_count, PoolError::EntryNotFound);
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let annotator = ctx.accounts.annotator.key();
    let hash = registry::append(pagare, KIND_RECIBO, &annotator, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    if pagare.status == PAGARE_PAID_PENDING_CANCEL {
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
