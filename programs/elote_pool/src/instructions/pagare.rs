//! The pagaré after its issuance (`draw`, in `lines.rs`) and its payment (`repay`): the delivery
//! of the paid título to the suscriptor or to the aval who settled it, the owner's burn, the
//! council's annulment of a título issued in error, the endoso en procuración and its revocation,
//! court annotations and receipt annotations. Each appends an entry to the pagaré record's chain;
//! the record is never closed.

use anchor_lang::prelude::*;

use crate::council;
use crate::errors::PoolError;
use crate::events::{PagareAnnulled, PagareBurned, PagareDelivered};
use crate::pool_signer;
use crate::registry;
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, KIND_ANOTACION_JUDICIAL, KIND_CONSTANCIA_JUDICIAL, KIND_ENTREGA,
    KIND_PROCURACION, KIND_RECIBO, KIND_REVOCACION, KIND_TERMINACION, PAGARE_FLAG_DELIVERED, PAGARE_FLAG_JUDICIAL_HOLD,
    PAGARE_FLAG_PAID_BY_AVAL, PAGARE_FLAG_PROCURACION, PAGARE_OUTSTANDING, PAGARE_PAID,
};
use crate::events::{PagareEntry, PagareReceiptAnnotated};

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct DeliverPagare<'info> {
    /// Anyone: the delivery is the debtor's right, not a decision. Pays what the CPIs may need.
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

    /// The título's new owner: the line's borrower, or its sponsor when the aval made the
    /// settling payment. Does not sign.
    /// CHECK: compared against the line in the handler (`WrongAccount`).
    pub recipient: UncheckedAccount<'info>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Delivers a paid título (decision 38, LGTOC 129-130): the pool thaws the asset, renounces the
/// `PermanentFreezeDelegate` (to `None`, terminal — nobody can ever freeze the título again) and
/// transfers it to the line's borrower, or to the aval when the aval's payment covered it
/// (`PAGARE_FLAG_PAID_BY_AVAL`, LGTOC 115) — never to whoever pays or sends this. Anyone may call
/// it, in the repayment's own transaction or later: the council is not in the path. Refused before
/// the payment (`PagareNotCovered`), twice (`PagareAlreadyDelivered`), and while a court act holds
/// the título (`DeliveryHeld`). The transfer itself returns every owner-managed plugin to the new
/// owner, so no revocation is needed; after the CPIs, an `Entrega` entry over the payment's
/// receipt hash and the `DELIVERED` flag record it.
pub fn deliver_pagare(ctx: Context<DeliverPagare>, draw_index: u16) -> Result<()> {
    use mpl_core::instructions::{ApprovePluginAuthorityV1CpiBuilder, TransferV1CpiBuilder, UpdatePluginV1CpiBuilder};
    use mpl_core::types::{PermanentFreezeDelegate, Plugin, PluginAuthority, PluginType};
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status == PAGARE_PAID, PoolError::PagareNotCovered);
    require!(pagare.flags & PAGARE_FLAG_DELIVERED == 0, PoolError::PagareAlreadyDelivered);
    require!(pagare.flags & PAGARE_FLAG_JUDICIAL_HOLD == 0, PoolError::DeliveryHeld);
    let line = &ctx.accounts.line;
    let to = if pagare.flags & PAGARE_FLAG_PAID_BY_AVAL != 0 {
        line.sponsor.ok_or(PoolError::WrongAccount)?
    } else {
        line.borrower
    };
    require_keys_eq!(ctx.accounts.recipient.key(), to, PoolError::WrongAccount);

    // The CPIs, in this order and no other: thaw, renounce, transfer. The renunciation can never
    // run on a frozen asset (frozen + `None` is stuck forever, tests/core_capabilities.rs Q2),
    // and once the owner changes the pool has no freeze to renounce.
    let pool = &ctx.accounts.pool;
    let core = ctx.accounts.mpl_core_program.to_account_info();
    let asset = ctx.accounts.asset.to_account_info();
    let collection = ctx.accounts.collection.to_account_info();
    let payer = ctx.accounts.payer.to_account_info();
    let authority = ctx.accounts.pool.to_account_info();
    let system = ctx.accounts.system_program.to_account_info();
    UpdatePluginV1CpiBuilder::new(&core)
        .asset(&asset)
        .collection(Some(&collection))
        .payer(&payer)
        .authority(Some(&authority))
        .system_program(&system)
        .plugin(Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: false }))
        .invoke_signed(pool_signer!(pool))?;
    ApprovePluginAuthorityV1CpiBuilder::new(&core)
        .asset(&asset)
        .collection(Some(&collection))
        .payer(&payer)
        .authority(Some(&authority))
        .system_program(&system)
        .plugin_type(PluginType::PermanentFreezeDelegate)
        .new_authority(PluginAuthority::None)
        .invoke_signed(pool_signer!(pool))?;
    let recipient_info = ctx.accounts.recipient.to_account_info();
    TransferV1CpiBuilder::new(&core)
        .asset(&asset)
        .collection(Some(&collection))
        .payer(&payer)
        .authority(Some(&authority))
        .new_owner(&recipient_info)
        .system_program(Some(&system))
        .invoke_signed(pool_signer!(pool))?;

    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let receipt_hash = ctx.accounts.pagare.receipt_hash;
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_ENTREGA, &pool_key, &to, &receipt_hash, now).ok_or(PoolError::Overflow)?;
    pagare.flags |= PAGARE_FLAG_DELIVERED;
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;
    let line_id = ctx.accounts.line.line_id;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id,
        draw_index,
        asset: asset_key,
        kind: KIND_ENTREGA,
        from: pool_key,
        to,
        entry_hash: hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(PagareDelivered {
        pool: pool_key,
        line_id,
        draw_index,
        asset: asset_key,
        recipient: to,
        payer: ctx.accounts.payer.key(),
        receipt_hash,
        entry_count,
        delivered_at: now,
    });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct BurnPagare<'info> {
    /// The delivered título's owner; the Core program refuses anyone else. Receives the asset's
    /// rent.
    #[account(mut)]
    pub owner: Signer<'info>,

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

    /// CHECK: must be `pagare.asset`; the Core program checks the rest, the ownership included.
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

/// The delivered título's owner — the suscriptor, or the aval who settled it — burns the asset,
/// optionally and only after the delivery (`PagareNotCovered` before it). A `Terminacion` entry
/// over `document_hash` (zero for none: the burn is the owner's unilateral act) precedes the
/// `BurnV1`, whose authority is the signer: the Core program itself refuses anyone but the
/// asset's owner, so the program checks nothing more. The record stays, as always.
pub fn burn_pagare(ctx: Context<BurnPagare>, draw_index: u16, document_hash: [u8; 32]) -> Result<()> {
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status == PAGARE_PAID && pagare.flags & PAGARE_FLAG_DELIVERED != 0, PoolError::PagareNotCovered);
    let now = Clock::get()?.unix_timestamp;
    let owner = ctx.accounts.owner.key();
    let pool_key = ctx.accounts.pool.key();
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_TERMINACION, &owner, &owner, &document_hash, now).ok_or(PoolError::Overflow)?;
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;
    let line_id = ctx.accounts.line.line_id;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id,
        draw_index,
        asset: asset_key,
        kind: KIND_TERMINACION,
        from: owner,
        to: owner,
        entry_hash: hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(PagareBurned { pool: pool_key, line_id, draw_index, asset: asset_key, owner, document_hash });

    // Last, so a Core refusal leaves nothing half done. The owner signed the transaction, so the
    // CPI carries the signature; the rent goes back to the owner.
    mpl_core::instructions::BurnV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.owner.to_account_info())
        .authority(Some(&ctx.accounts.owner.to_account_info()))
        .system_program(Some(&ctx.accounts.system_program.to_account_info()))
        .invoke()?;
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(draw_index: u16)]
pub struct AnnulPagare<'info> {
    /// The council, the holder.
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

/// The council annuls an outstanding título issued in error: a `Terminacion` entry over the
/// annulment document's own hash (`DocumentHashRequired`; never a receipt — nothing was paid).
/// Nothing is burnt, no state moves and the debt follows the line as always: a título issued in
/// error is annotated, never destroyed (decision 38). Refused once paid (`PagareAlreadyPaid`):
/// a paid título is the debtor's.
pub fn annul_pagare(ctx: Context<AnnulPagare>, draw_index: u16, document_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    require!(document_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &ctx.accounts.pagare;
    require!(pagare.status == PAGARE_OUTSTANDING, PoolError::PagareAlreadyPaid);
    let now = Clock::get()?.unix_timestamp;
    let pool_key = pool.key();
    let pagare = &mut ctx.accounts.pagare;
    let hash = registry::append(pagare, KIND_TERMINACION, &pool_key, &pool_key, &document_hash, now).ok_or(PoolError::Overflow)?;
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;
    let line_id = ctx.accounts.line.line_id;
    emit_cpi!(PagareEntry {
        pool: pool_key,
        line_id,
        draw_index,
        asset: asset_key,
        kind: KIND_TERMINACION,
        from: pool_key,
        to: pool_key,
        entry_hash: hash,
        entry_count,
        recorded_at: now,
        stated_at: now,
    });
    emit_cpi!(PagareAnnulled { pool: pool_key, line_id, draw_index, asset: asset_key, document_hash });
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

/// Endoso en procuración, the only kind the pool makes: the pool stays the owner, and the
/// procurador of record collects — through `repay`, like any payer — and litigates with the
/// registry's entry as the endorsement. It lends no power over the asset (decision 38: the
/// extinction route is the delivery to the debtor, so there is no burn to delegate). Refused:
/// `kind != KIND_PROCURACION` (`EndosoKindInvalid`); `to` the zero
/// key, the pool, the council, the pool's creator, the FIX feeder, the line's
/// borrower, rent payer or sponsor, or a member of the council's multisig (`EndorseeInvalid`); a
/// zero `endoso_hash`
/// (`DocumentHashRequired`); a `WrittenOff` line (`LineStatus`); a pagaré not outstanding
/// (`PagareAlreadyPaid`); a procuración in force (`ProcuracionState`).
pub fn endorse_pagare(ctx: Context<EndorsePagare>, draw_index: u16, kind: u8, to: Pubkey, endoso_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
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
    let pagare = &mut ctx.accounts.pagare;
    let entry_hash = registry::append(pagare, kind, &pool_key, &to, &endoso_hash, now).ok_or(PoolError::Overflow)?;
    pagare.flags |= PAGARE_FLAG_PROCURACION;
    pagare.procurador = to;
    let entry_count = pagare.entry_count;
    let asset_key = pagare.asset;

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

/// Revokes the procuración in force: the owner alone, over the signed revocation's hash
/// (`DocumentHashRequired`); refused when none is in force (`ProcuracionState`). Appends a
/// `Revocacion` entry (from the pool to the procurador); there is no power over the asset to take
/// back (decision 38).
pub fn revoke_procuracion(ctx: Context<RevokeProcuracion>, draw_index: u16, entry_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
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
/// (from and to the pool: there is no counterparty); an `AnotacionJudicial` also holds the título
/// (an embargo must comprise it, LGTOC 20: `deliver_pagare` refuses while the hold is set) and a
/// `ConstanciaJudicial` lifts the hold. Allowed on any line status, beside a procuración and on a
/// paid título whose delivery is pending; refused only once the título was delivered
/// (`PagareAlreadyPaid`): from then on it is the owner's.
pub fn court_annotation(ctx: Context<CourtAnnotation>, draw_index: u16, kind: u8, document_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    require!(kind == KIND_ANOTACION_JUDICIAL || kind == KIND_CONSTANCIA_JUDICIAL, PoolError::AnnotationKindInvalid);
    require!(document_hash != [0u8; 32], PoolError::DocumentHashRequired);
    let pagare = &mut ctx.accounts.pagare;
    require!(
        !(pagare.status == PAGARE_PAID && pagare.flags & PAGARE_FLAG_DELIVERED != 0),
        PoolError::PagareAlreadyPaid
    );
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let hash = registry::append(pagare, kind, &pool_key, &pool_key, &document_hash, now).ok_or(PoolError::Overflow)?;
    if kind == KIND_ANOTACION_JUDICIAL {
        pagare.flags |= PAGARE_FLAG_JUDICIAL_HOLD;
    } else {
        pagare.flags &= !PAGARE_FLAG_JUDICIAL_HOLD;
    }
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
/// reference is not checked to be a payment entry; the engine checks it. Allowed on any status,
/// a delivered or burnt título included: a receipt may arrive late, and the correction is the
/// entry — no field is rewritten (LGTOC 41).
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
