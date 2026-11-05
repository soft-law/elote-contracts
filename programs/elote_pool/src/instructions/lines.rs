//! Credit lines approved from the pool: request and approval, draw, repay, collection and write off.
//!
//! A line moves `Requested -> Approved -> Active -> Repaid | WrittenOff` (a rejected or withdrawn
//! request closes), or `Approved -> Cancelled` if nothing
//! was drawn; a scheduled line can also move `Active -> InCollection -> Repaid | WrittenOff`. The
//! pause blocks requests, approvals and draws. Repayments go to late interest, then interest,
//! then principal. Collected interest raises the NAV; a written-off principal lowers it once.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self as token, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{
    AccelerationProposed, AccelerationWithdrawn, LateInterestCollected, LineAccelerated, LineApproved, LineDrawsResumed, LineCancelled, LineDrawn, LineDrawsSuspended, LineInCollection,
    FxRealized, LineRejected, LineRepaid, LineRequested, LineWrittenOff, RequestWithdrawn, NavUpdated, PagareEntry, PagareIssued,
    PagarePayment, PaymentEntry,
};
use crate::math::{self, Arrears, ScheduleTerms};
use crate::council;
use crate::fx;
use crate::pool_signer;
use crate::registry;
use crate::terms::{self, LineTerms};
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, ScheduleParams, BPS, KIND_ABONO, KIND_EMISION, KIND_PAGADO,
    CURRENCY_MXN, MAX_DRAWS, MAX_INSTALLMENTS, MAX_SIDECARS_PER_REPAY, MIN_MAX_MISSED, REQUEST_TTL_SECONDS, PAGARE_OUTSTANDING, PAGARE_PAID_PENDING_CANCEL,
    SIGNATURE_EFIRMA_WALLET, SIGNATURE_WALLET, SPONSOR_GUARANTOR,
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(line_id: u64)]
pub struct RequestLine<'info> {
    pub borrower: Signer<'info>,

    /// Funds the request and receives the rent back on withdraw or reject; never the borrower's
    /// concern.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The sponsor named in the terms; required (and signing) when one is named.
    pub sponsor: Option<Signer<'info>>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = payer,
        space = 8 + CreditLine::INIT_SPACE,
        seeds = [seeds::LINE, pool.key().as_ref(), &line_id.to_le_bytes()],
        bump,
    )]
    pub line: Account<'info, CreditLine>,

    pub system_program: Program<'info, System>,
}

/// Stores the terms of line `line_id` (the pool's next id, never reused) as `Requested`, with
/// their `terms_hash`. The borrower, the payer and the named sponsor sign; refused while paused or
/// with the council as borrower. Moves no money.
pub fn request_line(ctx: Context<RequestLine>, line_id: u64, t: LineTerms) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &mut ctx.accounts.pool;
    let borrower = ctx.accounts.borrower.key();
    require!(!pool.paused, PoolError::Paused);
    require!(line_id == pool.line_count, PoolError::WrongAccount);
    require!(borrower != pool.council, PoolError::BorrowerIsCouncil);
    require!(t.principal > 0, PoolError::WrongAmount);
    require!(u64::from(t.rate_bps) <= BPS && u64::from(t.late_bps) <= BPS, PoolError::WrongAmount);
    require!(t.term_end > now, PoolError::TermEnded);
    require!(t.agreement_hash != [0u8; 32], PoolError::AgreementHashRequired);
    require!(t.expediente_root != [0u8; 32], PoolError::ExpedienteRootRequired);
    require!(t.pagare_template_hash != [0u8; 32], PoolError::PagareTemplateHashRequired);
    require!(t.sponsor_role <= SPONSOR_GUARANTOR, PoolError::SponsorRoleInvalid);
    if let Some(named) = t.sponsor {
        let signed = ctx.accounts.sponsor.as_ref().is_some_and(|s| s.key() == named);
        require!(signed, PoolError::SponsorRequired);
    }
    if let Some(s) = t.schedule {
        validate_schedule(&s, t.principal, t.rate_bps, now, t.term_end)?;
    }
    let terms_hash = terms::terms_hash(&pool.key(), line_id, &borrower, CURRENCY_MXN, &t);
    let s = t.schedule.unwrap_or(ScheduleParams { first_due_at: 0, interval_seconds: 0, installments: 0, max_missed: 0 });

    let line = &mut ctx.accounts.line;
    line.set_inner(CreditLine {
        pool: pool.key(),
        line_id,
        borrower,
        sponsor: t.sponsor,
        sponsor_role: t.sponsor_role,
        agreement_hash: t.agreement_hash,
        expediente_root: t.expediente_root,
        status: LineStatus::Requested,
        principal: t.principal,
        rate_bps: t.rate_bps,
        term_end: t.term_end,
        availability_end: 0,
        drawn: 0,
        draws: 0,
        interest_due: 0,
        principal_repaid: 0,
        interest_repaid: 0,
        pagares_paid: 0,
        first_due_at: s.first_due_at,
        interval_seconds: s.interval_seconds,
        installments: s.installments,
        late_bps: t.late_bps,
        max_missed: s.max_missed,
        late_collected: 0,
        late_paid_until: 0,
        drawn_at: 0,
        closed_at: 0,
        accelerated_at: 0,
        bump: ctx.bumps.line,
        currency: CURRENCY_MXN,
        requested_at: now,
        approved_at: 0,
        terms_hash,
        pagare_template_hash: t.pagare_template_hash,
        payer: ctx.accounts.payer.key(),
        draw_time_weight: 0,
        availability_end_saved: 0,
        acceleration_due_at: 0,
        usdc_drawn: 0,
        usdc_cost_repaid: 0,
        reserved: [0; 64],
    });
    pool.line_count = pool.line_count.checked_add(1).ok_or(PoolError::Overflow)?;

    emit_cpi!(LineRequested {
        pool: pool.key(),
        line_id,
        borrower,
        payer: line.payer,
        sponsor: t.sponsor,
        sponsor_role: t.sponsor_role,
        agreement_hash: t.agreement_hash,
        expediente_root: t.expediente_root,
        pagare_template_hash: t.pagare_template_hash,
        terms_hash,
        currency: CURRENCY_MXN,
        principal: t.principal,
        rate_bps: t.rate_bps,
        term_end: t.term_end,
        first_due_at: s.first_due_at,
        interval_seconds: s.interval_seconds,
        installments: s.installments,
        late_bps: t.late_bps,
        max_missed: s.max_missed,
    });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct ApproveLine<'info> {
    pub council: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = council @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,

    #[account(mut, has_one = pool @ PoolError::WrongAccount)]
    pub line: Account<'info, CreditLine>,

    /// Read for the NAV the concentration cap is measured against.
    #[account(address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
}

/// The council approves exactly the requested expediente and terms (`ExpedienteMismatch`), within
/// `REQUEST_TTL_SECONDS` of the request (`RequestExpired`), with the borrower not a member of the
/// council's multisig (`BorrowerIsCurator`) and the principal at most `max_line_bps` of the NAV now
/// (`LineTooLarge`), converted to MXN at the FIX in force; refused while paused. Opens the draw window: to the first due date, or to the term on a bullet line.
pub fn approve_line(ctx: Context<ApproveLine>, expediente_root: [u8; 32], terms_hash: [u8; 32], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    let multisig = ctx.accounts.multisig.to_account_info();
    council::check(&multisig, &pool.council, pool.council_vault_bump)?;
    let line = &mut ctx.accounts.line;
    require!(line.status == LineStatus::Requested, PoolError::LineStatus);
    let expires = line.requested_at.checked_add(REQUEST_TTL_SECONDS).ok_or(PoolError::Overflow)?;
    require!(now <= expires, PoolError::RequestExpired);
    require!(expediente_root == line.expediente_root && terms_hash == line.terms_hash, PoolError::ExpedienteMismatch);
    require!(!council::is_member(&multisig, &line.borrower)?, PoolError::BorrowerIsCurator);
    let nav = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    // The NAV is in USDC and the principal in MXN: the cap is converted at the FIX in force.
    require!(pool.fix_rate != 0, PoolError::FixStale);
    let cap = pool.max_line_principal(nav).ok_or(PoolError::Overflow)?;
    let cap_mxn = fx::mxn_for_usdc(cap, pool.fix_rate).ok_or(PoolError::Overflow)?;
    require!(line.principal <= cap_mxn, PoolError::LineTooLarge);
    let availability_end = if line.installments > 0 { line.first_due_at } else { line.term_end };
    require!(availability_end > now, PoolError::TermEnded);
    line.status = LineStatus::Approved;
    line.approved_at = now;
    line.availability_end = availability_end;
    emit_cpi!(LineApproved { pool: pool.key(), line_id: line.line_id, expediente_root, terms_hash, availability_end });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct RejectLine<'info> {
    pub council: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = council @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        has_one = pool @ PoolError::WrongAccount,
        has_one = payer @ PoolError::WrongAccount,
        constraint = line.status == LineStatus::Requested @ PoolError::LineStatus,
        close = payer,
    )]
    pub line: Account<'info, CreditLine>,

    /// CHECK: receives the rent; must be the line's `payer`.
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
}

/// The council rejects a `Requested` line; the account closes and its rent returns to the payer.
/// The id is not reused.
pub fn reject_line(ctx: Context<RejectLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    emit_cpi!(LineRejected { pool: pool.key(), line_id: ctx.accounts.line.line_id });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct WithdrawRequest<'info> {
    pub borrower: Signer<'info>,

    #[account(
        mut,
        has_one = borrower @ PoolError::Unauthorized,
        has_one = payer @ PoolError::WrongAccount,
        constraint = line.status == LineStatus::Requested @ PoolError::LineStatus,
        close = payer,
    )]
    pub line: Account<'info, CreditLine>,

    /// CHECK: receives the rent; must be the line's `payer`.
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
}

/// The borrower withdraws a `Requested` line, expired or not; the account closes and its rent
/// returns to the payer. The id is not reused.
pub fn withdraw_request(ctx: Context<WithdrawRequest>) -> Result<()> {
    let line = &ctx.accounts.line;
    emit_cpi!(RequestWithdrawn { pool: line.pool, line_id: line.line_id });
    Ok(())
}

/// Checks the schedule against the line's terms: at least one and at most `MAX_INSTALLMENTS`
/// installments, a positive interval, `MIN_MAX_MISSED <= max_missed <=
/// installments`, a first due date after the request, the last due date
/// (`first_due_at + (installments - 1) * interval`) no later than `term_end`, and at least one
/// unit per installment if the whole commitment were drawn now (`draw` re-checks on what is
/// actually drawn). The dates are fixed by the contract and not re-checked at the draw.
fn validate_schedule(s: &ScheduleParams, principal: u64, rate_bps: u16, now: i64, term_end: i64) -> Result<()> {
    require!(s.installments >= 1 && s.installments <= MAX_INSTALLMENTS, PoolError::InvalidSchedule);
    require!(s.interval_seconds > 0, PoolError::InvalidSchedule);
    require!(s.max_missed >= MIN_MAX_MISSED, PoolError::MaxMissedTooLow);
    require!(u16::from(s.max_missed) <= s.installments, PoolError::InvalidSchedule);
    require!(s.first_due_at > now, PoolError::InvalidSchedule);
    let last_due = last_due_of(s)?;
    require!(last_due <= term_end, PoolError::ScheduleEndsAfterTerm);
    let seconds = u64::try_from(last_due.checked_sub(now).ok_or(PoolError::Overflow)?).map_err(|_| PoolError::Overflow)?;
    let interest_if_drawn_now = math::tranche_interest(principal, rate_bps, seconds).ok_or(PoolError::Overflow)?;
    let total = principal.checked_add(interest_if_drawn_now).ok_or(PoolError::Overflow)?;
    require!(total >= u64::from(s.installments), PoolError::InvalidSchedule);
    Ok(())
}

/// The last due date of a schedule: `first_due_at + (installments - 1) * interval`.
fn last_due_of(s: &ScheduleParams) -> Result<i64> {
    let span = s
        .interval_seconds
        .checked_mul(i64::from(s.installments).checked_sub(1).ok_or(PoolError::Overflow)?)
        .ok_or(PoolError::Overflow)?;
    Ok(s.first_due_at.checked_add(span).ok_or(PoolError::Overflow)?)
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(amount_mxn: u64, fix_rate: u64, fix_date: u32, asset_bump: u8, pagare_bump: u8)]
pub struct Draw<'info> {
    /// Signs alone: the signature covers the pagaré hash the program derives and the FIX.
    pub borrower: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        mut,
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
        has_one = borrower @ PoolError::Unauthorized,
    )]
    pub line: Box<Account<'info, CreditLine>>,

    /// Read for its decimals (`transfer_checked`).
    #[account(address = pool.asset_mint @ PoolError::WrongAccount)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = borrower_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = borrower_asset_account.owner == borrower.key() @ PoolError::WrongAccount,
    )]
    pub borrower_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,


    /// Pays the rent of the pagaré asset and its sidecar. Any signer.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The pagaré's Core asset, created here by the Core program. The client passes the bump so the
    /// constraint does not search for it.
    /// CHECK: the seeds; the Core program initialises it.
    #[account(
        mut,
        seeds = [seeds::PAGARE_ASSET, line.key().as_ref(), &line.draws.to_le_bytes()],
        bump = asset_bump,
    )]
    pub asset: UncheckedAccount<'info>,

    /// The pagaré's record, created by the handler (Anchor's `init` would search for the bump).
    /// CHECK: the seeds; the handler initialises it.
    #[account(
        mut,
        seeds = [seeds::PAGARE, line.key().as_ref(), &line.draws.to_le_bytes()],
        bump = pagare_bump,
    )]
    pub pagare: UncheckedAccount<'info>,

    /// The series' collection the pool controls (validated at `init_pool`).
    /// CHECK: must be `pool.collection`; the Core program checks the rest.
    #[account(mut, address = pool.collection @ PoolError::WrongAccount)]
    pub collection: UncheckedAccount<'info>,

    /// CHECK: the Metaplex Core program, by address.
    #[account(address = mpl_core::ID @ PoolError::WrongAccount)]
    pub mpl_core_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Lower-case hex of `bytes`, byte by byte, without the formatting machinery.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)]);
        s.push(DIGITS[usize::from(b & 0x0f)]);
    }
    // Every byte pushed is an ASCII digit.
    String::from_utf8(s).unwrap_or_default()
}

/// Decimal text of `n` without the formatting machinery, which is costly on chain.
fn dec(n: i64) -> String {
    let mut s = dec_u(n.unsigned_abs());
    if n < 0 {
        s.insert(0, '-');
    }
    s
}

/// `dec` for an unsigned value (64-bit arithmetic only: 128-bit division is a software routine).
fn dec_u(mut m: u64) -> String {
    if m == 0 {
        return "0".into();
    }
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    while m > 0 {
        i -= 1;
        digits[i] = b'0' + (m % 10) as u8;
        m /= 10;
    }
    // Every byte written is an ASCII digit.
    String::from_utf8(digits[i..].to_vec()).unwrap_or_default()
}

/// Mints the pagaré: a Core asset in the pool's collection, owned by the pool PDA, named
/// `PAGARE <line_id>-<draw_index>`, with an empty URI and nothing personal. Plugins at creation:
/// - `PermanentFreezeDelegate`, frozen, authority `UpdateAuthority` (the collection's, the pool
///   PDA): the asset cannot move, and only the program can thaw it, which `cancel_pagare` does
///   right before the burn.
/// - `ImmutableMetadata` and `Attributes` (the eight facts, 32-byte values as hex; the certificate commitment stays in the record only), authority
///   `None`: name, URI and facts never change.
/// - `AddBlocker`, authority `None`: no authority-managed plugin can be added later.
///
/// No transfer or burn delegate: the pool owns the asset and only the program signs for it. The
/// only plugin added later is the owner-managed `BurnDelegate` of a procuración (`endorse_pagare`).
/// `init_pool` admits only a bare collection, so no collection plugin reaches the asset. No AppData:
/// the registry lives in the `Pagare` record.
#[allow(clippy::too_many_arguments)]
fn issue_pagare<'info>(accounts: &Draw<'info>, asset_bump: u8, record: &Pagare) -> Result<()> {
    let (draw_index, amount, document_hash, issued_at, due_at) =
        (record.draw_index, record.amount_mxn, record.pagare_hash, record.issued_at, record.due_at);
    use mpl_core::types::{
        AddBlocker, Attribute, Attributes, ImmutableMetadata, PermanentFreezeDelegate, Plugin, PluginAuthority,
        PluginAuthorityPair,
    };
    let pool = &accounts.pool;
    let line = &accounts.line;
    // Hex, not base58: base58 of a 32-byte key is too costly on chain.
    let attributes = vec![
        Attribute { key: "pagare_hash".into(), value: hex(&document_hash) },
        Attribute { key: "amount_mxn".into(), value: dec_u(amount) },
        Attribute { key: "currency".into(), value: "MXN".into() },
        Attribute { key: "issued_at".into(), value: dec(issued_at) },
        Attribute { key: "due_at".into(), value: dec(due_at) },
        Attribute { key: "line".into(), value: dec_u(line.line_id) },
        Attribute { key: "draw_index".into(), value: dec_u(draw_index.into()) },
        Attribute { key: "signature_method".into(), value: dec_u(record.signature_method.into()) },
    ];
    let plugins = vec![
        PluginAuthorityPair {
            plugin: Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true }),
            authority: Some(PluginAuthority::UpdateAuthority),
        },
        PluginAuthorityPair { plugin: Plugin::ImmutableMetadata(ImmutableMetadata {}), authority: Some(PluginAuthority::None) },
        PluginAuthorityPair {
            plugin: Plugin::Attributes(Attributes { attribute_list: attributes }),
            authority: Some(PluginAuthority::None),
        },
        PluginAuthorityPair { plugin: Plugin::AddBlocker(AddBlocker {}), authority: Some(PluginAuthority::None) },
    ];

    let index_bytes = draw_index.to_le_bytes();
    let line_key = line.key();
    let pool_seeds = pool_signer!(pool);
    let asset_seeds: &[&[u8]] = &[seeds::PAGARE_ASSET, line_key.as_ref(), &index_bytes, &[asset_bump]];
    sweep_to_payer(accounts, &accounts.asset.to_account_info(), asset_seeds)?;
    mpl_core::instructions::CreateV2CpiBuilder::new(&accounts.mpl_core_program.to_account_info())
        .asset(&accounts.asset.to_account_info())
        .collection(Some(&accounts.collection.to_account_info()))
        .authority(Some(&accounts.pool.to_account_info()))
        .payer(&accounts.payer.to_account_info())
        .owner(Some(&accounts.pool.to_account_info()))
        .system_program(&accounts.system_program.to_account_info())
        // No series label and no offering vocabulary.
        .name(["PAGARE ", &dec_u(line.line_id), "-", &dec_u(draw_index.into())].concat())
        .uri(String::new())
        .plugins(plugins)
        .invoke_signed(&[pool_seeds[0], asset_seeds])?;
    Ok(())
}

/// The pagaré addresses are predictable once the line is open, and both `create_account`s (the
/// Core program's for the asset, ours for the sidecar) refuse a funded address: left alone, a
/// stranger could block every draw of a line for the price of rent. Both addresses are this
/// program's PDAs, so whatever was sent there is moved to the payer first, signed by the PDA.
fn sweep_to_payer<'info>(accounts: &Draw<'info>, account: &AccountInfo<'info>, seeds: &[&[u8]]) -> Result<()> {
    let stray = account.lamports();
    if stray > 0 {
        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(
                accounts.system_program.key(),
                anchor_lang::system_program::Transfer { from: account.clone(), to: accounts.payer.to_account_info() },
                &[seeds],
            ),
            stray,
        )?;
    }
    Ok(())
}

/// Creates the sidecar by hand (the seeds constraint verified the address with the client's bump)
/// and writes `record` into it.
fn create_sidecar<'info>(accounts: &Draw<'info>, pagare_bump: u8, record: &Pagare) -> Result<()> {
    use anchor_lang::system_program::{create_account, CreateAccount};
    let space = 8 + Pagare::INIT_SPACE;
    let line_key = accounts.line.key();
    let index_bytes = record.draw_index.to_le_bytes();
    let seeds: &[&[u8]] = &[seeds::PAGARE, line_key.as_ref(), &index_bytes, &[pagare_bump]];
    let pagare = accounts.pagare.to_account_info();
    sweep_to_payer(accounts, &pagare, seeds)?;
    create_account(
        CpiContext::new_with_signer(
            accounts.system_program.key(),
            CreateAccount { from: accounts.payer.to_account_info(), to: pagare.clone() },
            &[seeds],
        ),
        Rent::get()?.minimum_balance(space),
        space as u64,
        &crate::ID,
    )?;
    let mut data = pagare.try_borrow_mut_data()?;
    record.try_serialize(&mut &mut data[..])?;
    Ok(())
}

/// `sha256(pagare_template_hash || terms_hash || line || draw_index u16 || amount_mxn u64 ||
/// drawn_at i64)`, little endian: the pagaré the borrower signs, derived from the approved text and
/// terms, promising `amount_mxn` centavos of MXN.
pub fn pagare_hash(line: &CreditLine, line_key: &Pubkey, draw_index: u16, amount_mxn: u64, drawn_at: i64) -> [u8; 32] {
    solana_sha256_hasher::hashv(&[
        &line.pagare_template_hash,
        &line.terms_hash,
        line_key.as_ref(),
        &draw_index.to_le_bytes(),
        &amount_mxn.to_le_bytes(),
        &drawn_at.to_le_bytes(),
    ])
    .to_bytes()
}

/// `sha256(pagare_hash || fix_rate u64 || fix_date u32 || usdc_delivered u64)`: the delivery the
/// borrower instructed, in USDC at the FIX.
pub fn delivery_hash(pagare_hash: &[u8; 32], fix_rate: u64, fix_date: u32, usdc_delivered: u64) -> [u8; 32] {
    solana_sha256_hasher::hashv(&[pagare_hash, &fix_rate.to_le_bytes(), &fix_date.to_le_bytes(), &usdc_delivered.to_le_bytes()]).to_bytes()
}

/// One tranche of `amount_mxn` centavos, signed by the borrower alone, delivered as USDC at the FIX
/// in force, which the borrower signs as `fix_rate` and `fix_date` (`FixMismatch` if it changed,
/// `FixStale` if older than `fx::MAX_FIX_AGE_DAYS`). Every check runs before anything is written:
/// not paused; `Approved` or `Active`; `0 < amount_mxn <= principal - drawn`; `now <
/// availability_end`, strictly; on a scheduled line every installment keeps at least one centavo;
/// fewer than `MAX_DRAWS` tranches; at least one USDC base unit delivered; the vault has it; the
/// pool's outstanding cost stays within `max_outstanding_principal`; a valid signature method. The
/// program derives the pagaré hash and the delivery hash; the tranche's interest, at the annual rate
/// from now to the last due date, is fixed here.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: Context<Draw>,
    amount_mxn: u64,
    fix_rate: u64,
    fix_date: u32,
    asset_bump: u8,
    pagare_bump: u8,
    cert_fingerprint: [u8; 32],
    signature_method: u8,
) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    require!(!pool.paused, PoolError::Paused);
    require!(matches!(line.status, LineStatus::Approved | LineStatus::Active), PoolError::LineStatus);
    let available = line.available().ok_or(PoolError::Overflow)?;
    require!(amount_mxn > 0 && amount_mxn <= available, PoolError::WrongAmount);
    let now = Clock::get()?.unix_timestamp;
    require!(now < line.availability_end, PoolError::AvailabilityEnded);
    require!(pool.fix_rate != 0 && fix_rate == pool.fix_rate && fix_date == pool.fix_date, PoolError::FixMismatch);
    let age = fx::mexico_city_day(now).checked_sub(fx::fix_day(pool.fix_date)?).ok_or(PoolError::Overflow)?;
    require!(age <= fx::MAX_FIX_AGE_DAYS, PoolError::FixStale);
    let last_due = line.last_due().ok_or(PoolError::Overflow)?;
    let seconds = u64::try_from(last_due.checked_sub(now).ok_or(PoolError::Overflow)?).map_err(|_| PoolError::Overflow)?;
    let interest = math::tranche_interest(amount_mxn, line.rate_bps, seconds).ok_or(PoolError::Overflow)?;
    let drawn_after = line.drawn.checked_add(amount_mxn).ok_or(PoolError::Overflow)?;
    let interest_after = line.interest_due.checked_add(interest).ok_or(PoolError::Overflow)?;
    if let Some(s) = line.schedule() {
        let total_after = drawn_after.checked_add(interest_after).ok_or(PoolError::Overflow)?;
        require!(total_after >= u64::from(s.installments), PoolError::InvalidSchedule);
    }
    require!(line.draws < MAX_DRAWS, PoolError::TooManyDraws);
    let usdc = fx::usdc_for_mxn(amount_mxn, fix_rate).ok_or(PoolError::Overflow)?;
    require!(usdc > 0, PoolError::WrongAmount);
    require!(usdc <= ctx.accounts.vault.amount, PoolError::InsufficientLiquidity);
    let outstanding_after = pool.outstanding_principal.checked_add(usdc).ok_or(PoolError::Overflow)?;
    require!(outstanding_after <= pool.max_outstanding_principal, PoolError::OutstandingCapExceeded);
    // The certificate fingerprint may be zero only for a wallet-only signature.
    require!(
        signature_method <= SIGNATURE_EFIRMA_WALLET && (signature_method == SIGNATURE_WALLET || cert_fingerprint != [0u8; 32]),
        PoolError::SignatureMethodInvalid
    );
    let draw_index = line.draws;
    let document_hash = pagare_hash(line, &line.key(), draw_index, amount_mxn, now);
    let delivery = delivery_hash(&document_hash, fix_rate, fix_date, usdc);

    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.asset_mint.to_account_info(),
                to: ctx.accounts.borrower_asset_account.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            pool_signer!(pool),
        ),
        usdc,
        ctx.accounts.asset_mint.decimals,
    )?;

    let record = Pagare {
        line: ctx.accounts.line.key(),
        draw_index,
        asset: ctx.accounts.asset.key(),
        pagare_hash: document_hash,
        amount_mxn,
        issued_at: now,
        due_at: last_due,
        drawn_through: drawn_after,
        cert_fingerprint,
        signature_method,
        status: PAGARE_OUTSTANDING,
        paid_at: 0,
        receipt_hash: [0; 32],
        cancel_receipt_hash: [0; 32],
        procurador: Pubkey::default(),
        flags: 0,
        log_root: [0; 32],
        entry_count: 0,
        last_kind: 0,
        bump: pagare_bump,
        fix_rate,
        fix_date,
        usdc_delivered: usdc,
        delivery_hash: delivery,
        cancel_grace_seconds: 0,
        reserved: [0; 28],
    };
    // The registry opens with the issuance entry, from and to the pool, over the pagaré's hash.
    let mut record = record;
    let pool_key = ctx.accounts.pool.key();
    let entry_hash =
        registry::append(&mut record, KIND_EMISION, &pool_key, &pool_key, &document_hash, now).ok_or(PoolError::Overflow)?;
    issue_pagare(ctx.accounts, asset_bump, &record)?;
    create_sidecar(ctx.accounts, pagare_bump, &record)?;

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = outstanding_after;
    let line = &mut ctx.accounts.line;
    line.drawn = drawn_after;
    line.usdc_drawn = line.usdc_drawn.checked_add(usdc).ok_or(PoolError::Overflow)?;
    let weight = u128::from(amount_mxn).checked_mul(u128::try_from(now).map_err(|_| PoolError::Overflow)?).ok_or(PoolError::Overflow)?;
    line.draw_time_weight = line.draw_time_weight.checked_add(weight).ok_or(PoolError::Overflow)?;
    line.draws = line.draws.checked_add(1).ok_or(PoolError::Overflow)?;
    line.interest_due = interest_after;
    if line.status == LineStatus::Approved {
        line.status = LineStatus::Active;
        line.drawn_at = now;
    }

    ctx.accounts.vault.reload()?;
    ctx.accounts.pool.check(ctx.accounts.vault.amount, None)?;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    emit_cpi!(LineDrawn {
        pool: pool.key(),
        line_id: line.line_id,
        draw_index,
        amount_mxn,
        fix_rate,
        fix_date,
        usdc_delivered: usdc,
        drawn: line.drawn,
        available: line.available().ok_or(PoolError::Overflow)?,
        interest_due: line.interest_due,
        availability_end: line.availability_end,
        outstanding_after: pool.outstanding_principal,
    });
    emit_cpi!(PagareIssued {
        pool: pool.key(),
        line_id: line.line_id,
        draw_index,
        asset: ctx.accounts.asset.key(),
        pagare_hash: document_hash,
        amount_mxn,
        delivery_hash: delivery,
        due_at: last_due,
        holder: pool.key(),
    });
    emit_cpi!(PagareEntry {
        pool: pool.key(),
        line_id: line.line_id,
        draw_index,
        asset: ctx.accounts.asset.key(),
        kind: KIND_EMISION,
        from: pool.key(),
        to: pool.key(),
        entry_hash,
        entry_count: record.entry_count,
        recorded_at: now,
        stated_at: now,
    });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct Repay<'info> {
    /// Anyone may repay: the borrower, a sponsor paying the interest, or a third party.
    pub payer: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Account<'info, CreditLine>,

    /// Read for its decimals (`transfer_checked`).
    #[account(address = pool.asset_mint @ PoolError::WrongAccount)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Read for the share supply (`NavUpdated` and the invariant check).
    #[account(address = pool.share_mint @ PoolError::WrongAccount)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = payer_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = payer_asset_account.owner == payer.key() @ PoolError::WrongAccount,
    )]
    pub payer_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

/// Pays an `Active` or `InCollection` line. Anyone may pay, and the pause never blocks it. `amount`
/// must cover the late interest due (`LateInterestUnpaid`; on a bullet line, from `term_end` or
/// from its acceleration); the payment
/// settles late interest, then interest, then principal, and debits the payer only what is due.
/// What is owed is what was drawn plus the interest fixed at each draw.
///
/// The line becomes `Repaid` only when nothing is due and nothing can be drawn (window closed,
/// fully drawn, or in collection). While it can draw, clearing the debt leaves it `Active` and a
/// payment on a zero debt is refused (`WrongAmount`); otherwise a payment on a zero debt closes
/// the line and moves nothing.
///
/// Remaining accounts: exactly the pagaré records `[pagares_paid ..= last_touched]`, oldest first,
/// where `last_touched` is the first one this payment leaves partly unpaid, or the last draw; none
/// once the cursor has passed the last draw (`PagareAccountsMismatch` otherwise), and never more
/// than `MAX_SIDECARS_PER_REPAY` (`TooManySidecars`): a larger payment is split. A covered pagaré
/// gets a `Pagado` entry, status `PAGARE_PAID_PENDING_CANCEL`, `paid_at`, `receipt_hash` and the
/// pool's `cancel_grace_seconds`, fixed for the borrower's exit (only the burn waits), and the cursor `pagares_paid` moves past it. The uncovered one gets an `Abono`
/// entry only if the payment applied principal to it. `receipt_hash` is the payer's, unverified and
/// provisional until the holder annotates the real one (`annotate_payment`); zero means "receipt to
/// follow".
///
/// The credit is `fx::mxn_for_usdc(max_usdc, fix)`; the USDC taken pays exactly the MXN applied,
/// rounded up. The pool's outstanding falls by the cost of the principal repaid at each pagaré's
/// own FIX; the difference with the USDC that paid it (the principal's centavos at today's FIX,
/// rounded up; the rounding of the rest stays with the charges) is `FxRealized`, emitted whenever
/// principal is applied or the difference is not zero.
pub fn repay<'info>(ctx: Context<'info, Repay<'info>>, max_usdc: u64, fix_rate: u64, fix_date: u32, receipt_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    require!(matches!(line.status, LineStatus::Active | LineStatus::InCollection), PoolError::LineStatus);
    // The FIX in force, whatever its age: a payment is never refused for an old FIX.
    require!(pool.fix_rate != 0 && fix_rate == pool.fix_rate && fix_date == pool.fix_date, PoolError::FixMismatch);
    let amount = fx::mxn_for_usdc(max_usdc, fix_rate).ok_or(PoolError::Overflow)?;

    let principal_left = line.drawn.checked_sub(line.principal_repaid).ok_or(PoolError::Overflow)?;
    let interest_left = line.interest_due.checked_sub(line.interest_repaid).ok_or(PoolError::Overflow)?;
    let remaining = principal_left.checked_add(interest_left).ok_or(PoolError::Overflow)?;
    require!(amount > 0, PoolError::WrongAmount);

    let late_part = line_arrears(line, now)?.late_due;
    require!(amount >= late_part, PoolError::LateInterestUnpaid);
    // While the line can still draw, a zero debt is nothing to pay, and a payment that clears the
    // debt does not close the line; otherwise a third party could pay one unit and
    // forfeit the borrower's commitment.
    let can_draw = line.status == LineStatus::Active
        && now < line.availability_end
        && line.available().ok_or(PoolError::Overflow)? > 0;
    require!(remaining > 0 || late_part > 0 || !can_draw, PoolError::WrongAmount);
    let rest = amount.checked_sub(late_part).ok_or(PoolError::Overflow)?.min(remaining);
    let interest_part = rest.min(interest_left);
    let principal_part = rest.checked_sub(interest_part).ok_or(PoolError::Overflow)?;
    let transfer = late_part
        .checked_add(interest_part)
        .and_then(|v| v.checked_add(principal_part))
        .ok_or(PoolError::Overflow)?;
    let principal_before = line.principal_repaid;
    let principal_after = principal_before.checked_add(principal_part).ok_or(PoolError::Overflow)?;
    // The USDC taken, rounded up, and the part of it that paid principal: what the principal's
    // centavos cost at the FIX, rounded up. The rest, at least the charges rounded down, is the
    // charges', so their rounding never reaches `fx_realized` and a payment without principal
    // realizes nothing.
    let usdc_in = fx::usdc_ceil_for_mxn(transfer, fix_rate).ok_or(PoolError::Overflow)?;
    let usdc_principal = fx::usdc_ceil_for_mxn(principal_part, fix_rate).ok_or(PoolError::Overflow)?;

    // The records, exactly `[pagares_paid ..= last_touched]`, are verified before any write.
    let line_key = line.key();
    let cursor = line.pagares_paid;
    let draws = line.draws;
    let sidecars = ctx.remaining_accounts;
    require!(sidecars.len() <= MAX_SIDECARS_PER_REPAY, PoolError::TooManySidecars);
    if cursor >= draws {
        require!(sidecars.is_empty(), PoolError::PagareAccountsMismatch);
    } else {
        require!(!sidecars.is_empty(), PoolError::PagareAccountsMismatch);
    }
    let mut touched: Vec<Account<'info, Pagare>> = Vec::with_capacity(sidecars.len());
    for (i, info) in sidecars.iter().enumerate() {
        let idx = u16::try_from(i).ok().and_then(|i| cursor.checked_add(i)).ok_or(PoolError::PagareAccountsMismatch)?;
        require!(idx < draws, PoolError::PagareAccountsMismatch);
        // Owned by the program with the `Pagare` discriminator (`try_from`), naming this line and
        // this index: only `draw` writes such an account, at `["pagare", line, index]`, so these
        // three facts identify the record without recomputing its address.
        let sidecar = Account::<Pagare>::try_from(info).map_err(|_| PoolError::PagareAccountsMismatch)?;
        require!(
            sidecar.line == line_key && sidecar.draw_index == idx && sidecar.status == PAGARE_OUTSTANDING,
            PoolError::PagareAccountsMismatch
        );
        let covered = sidecar.drawn_through <= principal_after;
        let last = i + 1 == sidecars.len();
        // An uncovered pagaré ends the set; a covered one that ends it must be the last draw.
        require!(if covered { !last || idx.saturating_add(1) == draws } else { last }, PoolError::PagareAccountsMismatch);
        touched.push(sidecar);
    }
    // The cost of the principal repaid, pagaré by pagaré at the FIX each was drawn at.
    let mut cost = 0u64;
    for sidecar in &touched {
        let start = sidecar.drawn_through.checked_sub(sidecar.amount_mxn).ok_or(PoolError::Overflow)?;
        let before = principal_before.saturating_sub(start);
        let after = principal_after.saturating_sub(start);
        let c = fx::cost_of(sidecar.usdc_delivered, sidecar.amount_mxn, after)
            .zip(fx::cost_of(sidecar.usdc_delivered, sidecar.amount_mxn, before))
            .and_then(|(a, b)| a.checked_sub(b))
            .ok_or(PoolError::Overflow)?;
        cost = cost.checked_add(c).ok_or(PoolError::Overflow)?;
    }
    let fx_delta = i64::try_from(usdc_principal).ok().zip(i64::try_from(cost).ok()).map(|(r, c)| r - c).ok_or(PoolError::Overflow)?;

    if usdc_in > 0 {
        token::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.payer_asset_account.to_account_info(),
                    mint: ctx.accounts.asset_mint.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            usdc_in,
            ctx.accounts.asset_mint.decimals,
        )?;
    }

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_sub(cost).ok_or(PoolError::Overflow)?;
    let usdc_interest = fx::usdc_for_mxn(interest_part, fix_rate).ok_or(PoolError::Overflow)?;
    pool.interest_collected = pool.interest_collected.checked_add(usdc_interest).ok_or(PoolError::Overflow)?;
    pool.fx_realized = pool.fx_realized.checked_add(fx_delta).ok_or(PoolError::Overflow)?;

    let line = &mut ctx.accounts.line;
    line.principal_repaid = line.principal_repaid.checked_add(principal_part).ok_or(PoolError::Overflow)?;
    line.interest_repaid = line.interest_repaid.checked_add(interest_part).ok_or(PoolError::Overflow)?;
    line.usdc_cost_repaid = line.usdc_cost_repaid.checked_add(cost).ok_or(PoolError::Overflow)?;
    // The watermark moves only when something was collected: an accrual that rounds down to zero
    // keeps running instead of being erased by every payment.
    if late_part > 0 {
        line.late_collected = line.late_collected.checked_add(late_part).ok_or(PoolError::Overflow)?;
        line.late_paid_until = now;
    }
    let remaining_after = remaining
        .checked_sub(interest_part)
        .and_then(|v| v.checked_sub(principal_part))
        .ok_or(PoolError::Overflow)?;
    if remaining_after == 0 && !can_draw {
        line.status = LineStatus::Repaid;
        line.closed_at = now;
    }

    // The payment entries, oldest first; the cursor moves past every pagaré marked paid.
    let cancel_grace = pool.cancel_grace_seconds;
    let payer_key = ctx.accounts.payer.key();
    let pool_key = pool.key();
    let line_id = line.line_id;
    let mut entries: Vec<PaymentEntry> = Vec::with_capacity(touched.len());
    let mut floor = principal_before;
    for sidecar in touched.iter_mut() {
        let covered = sidecar.drawn_through <= principal_after;
        let kind = if covered { KIND_PAGADO } else { KIND_ABONO };
        let ceiling = sidecar.drawn_through.min(principal_after);
        let principal_applied = ceiling.saturating_sub(floor);
        floor = floor.max(ceiling);
        // No entry without principal: otherwise anyone could grow the chain one unit at a time.
        if principal_applied == 0 {
            continue;
        }
        let hash = registry::append(sidecar, kind, &payer_key, &pool_key, &receipt_hash, now).ok_or(PoolError::Overflow)?;
        if covered {
            sidecar.status = PAGARE_PAID_PENDING_CANCEL;
            sidecar.paid_at = now;
            sidecar.receipt_hash = receipt_hash;
            sidecar.cancel_grace_seconds = cancel_grace;
            line.pagares_paid = sidecar.draw_index.checked_add(1).ok_or(PoolError::Overflow)?;
        }
        emit_cpi!(PagareEntry {
            pool: pool_key,
            line_id,
            draw_index: sidecar.draw_index,
            asset: sidecar.asset,
            kind,
            from: payer_key,
            to: pool_key,
            entry_hash: hash,
            entry_count: sidecar.entry_count,
            recorded_at: now,
            stated_at: now,
        });
        entries.push(PaymentEntry { draw_index: sidecar.draw_index, kind, entry_count: sidecar.entry_count, principal_applied });
        sidecar.exit(ctx.program_id)?;
    }
    if !touched.is_empty() {
        emit_cpi!(PagarePayment { pool: pool_key, line_id, payer: payer_key, receipt_hash, late_part, interest_part, principal_part, entries });
    }

    ctx.accounts.vault.reload()?;
    let share_supply = ctx.accounts.share_mint.supply;
    let nav_after = pool.check(ctx.accounts.vault.amount, Some(share_supply))?;
    if late_part > 0 {
        emit_cpi!(LateInterestCollected { pool: pool.key(), line_id: line.line_id, amount: late_part });
    }
    if principal_part > 0 || fx_delta != 0 {
        emit_cpi!(FxRealized {
            pool: pool.key(),
            line_id: line.line_id,
            receipt_hash,
            cost,
            received: usdc_principal,
            delta: fx_delta,
            nav_after,
        });
    }
    emit_cpi!(LineRepaid {
        pool: pool.key(),
        line_id: line.line_id,
        payer: ctx.accounts.payer.key(),
        receipt_hash,
        usdc_received: usdc_in,
        fix_rate,
        principal_part,
        interest_part,
        remaining: remaining_after,
        nav_after,
    });
    emit_cpi!(NavUpdated { pool: pool.key(), total_assets: nav_after, share_supply });
    Ok(())
}

/// The arrears of a line at `now`: the installments repay `drawn + interest_due` on the
/// contract's calendar, and late interest accrues on the principal share, `drawn`, of what is
/// overdue, from the first second past each due date (there is no grace period).
fn line_arrears(line: &CreditLine, now: i64) -> Result<Arrears> {
    // A bullet line is one installment due at the term: late interest runs on its unpaid
    // principal from `term_end`, or from the mark if it was accelerated before, at the same rate
    // formula as a schedule.
    let (anchor, interval, installments) = match line.schedule() {
        Some(s) => (line.anchor().ok_or(PoolError::Overflow)?, s.interval_seconds, s.installments),
        None => (line.term_end.checked_sub(1).ok_or(PoolError::Overflow)?, 1, 1),
    };
    let terms = ScheduleTerms {
        total: line.drawn.checked_add(line.interest_due).ok_or(PoolError::Overflow)?,
        anchor,
        interval,
        installments,
        late_bps: line.late_bps,
        grace: 0,
        late_paid_until: line.late_paid_until,
        principal: line.drawn,
        late_collected: line.late_collected,
        accelerated_at: line.accelerated_at,
    };
    let paid = line.principal_repaid.checked_add(line.interest_repaid).ok_or(PoolError::Overflow)?;
    Ok(math::arrears(&terms, paid, now).ok_or(PoolError::Overflow)?)
}

#[event_cpi]
#[derive(Accounts)]
pub struct MarkInCollection<'info> {
    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Account<'info, CreditLine>,
}

/// Anyone may move an active scheduled line to `InCollection` once `max_missed` installments are
/// missed, past the due date of the last of them: with `covered` installments paid,
/// `now > anchor + (covered + max_missed) * interval`, where the anchor is one interval before
/// `first_due_at`. The mark accelerates the balance (vencimiento anticipado):
/// `accelerated_at = now`, from which every installment is due and late interest runs on all
/// remaining principal, and the ordinary interest of the installments not yet due is forgiven
/// (`interest_due` shrinks to the share of the installments due, never below what was repaid; no
/// NAV effect, since interest counts when collected). A council proposal still in its notice is
/// superseded (`acceleration_due_at` back to zero, `AccelerationWithdrawn`). It records facts and
/// moves no funds, so it is not blocked by the pause.
pub fn mark_in_collection(ctx: Context<MarkInCollection>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let s = line.schedule().ok_or(PoolError::NotScheduled)?;
    require!(line.status == LineStatus::Active, PoolError::LineStatus);
    let a = line_arrears(line, now)?;
    let k = a.covered.checked_add(u16::from(s.max_missed)).ok_or(PoolError::Overflow)?;
    require!(k <= s.installments, PoolError::NotEnoughMissed);
    let anchor = line.anchor().ok_or(PoolError::Overflow)?;
    let last_missed_due = s.interval_seconds.checked_mul(i64::from(k)).and_then(|span| anchor.checked_add(span)).ok_or(PoolError::Overflow)?;
    require!(now > last_missed_due, PoolError::NotEnoughMissed);

    // The line keeps the ordinary interest of the installments due at the mark, at least the ones
    // that put it in collection.
    let (kept, interest_forgiven, remaining) = accelerated_balance(line, a.due.max(k), now)?;
    let pool_key = pool.key();
    let line = &mut ctx.accounts.line;
    line.status = LineStatus::InCollection;
    line.accelerated_at = now;
    line.interest_due = kept;
    // The objective route supersedes a council proposal still in its notice; draws stay suspended.
    let superseded = line.acceleration_due_at != 0;
    line.acceleration_due_at = 0;
    emit_cpi!(LineInCollection { pool: pool_key, line_id: line.line_id, missed: a.missed, remaining, interest_forgiven });
    if superseded {
        emit_cpi!(AccelerationWithdrawn { pool: pool_key, line_id: line.line_id });
    }
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct WriteOff<'info> {
    pub council: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = council @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        mut,
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Account<'info, CreditLine>,

    /// Read for the share supply (`NavUpdated` and the invariant check).
    #[account(address = pool.share_mint @ PoolError::WrongAccount)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Read for the NAV in the event.
    #[account(address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
}

/// Recognises the unpaid principal of an overdue line as a loss. Council only; not blocked by the
/// pause. Allowed when
///
/// - the line is `Active` and `now > term_end` (else `TermNotEnded`), or
/// - the line is `InCollection`, the collection period has run (`now > accelerated_at +
///   collection_period_seconds`), and either a missed installment is still unpaid and
///   `now > first_missed_due`, or `now > term_end`, or the line is a bullet line
///   accelerated by the council (else `WriteOffTooEarly`).
pub fn write_off(ctx: Context<WriteOff>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    let line = &ctx.accounts.line;
    let after_term = now > line.term_end;
    match line.status {
        LineStatus::Active => require!(after_term, PoolError::TermNotEnded),
        LineStatus::InCollection => {
            // An accelerated bullet line is wholly due at the mark, which the collection period
            // below already waits for.
            let after_missed = match line.schedule() {
                Some(_) => match line_arrears(line, now)?.first_missed_due {
                    Some(due) => now > due,
                    None => false, // nothing is missed
                },
                None => true,
            };
            require!(after_missed || after_term, PoolError::WriteOffTooEarly);
            let period_end = line
                .accelerated_at
                .checked_add(i64::from(pool.collection_period_seconds))
                .ok_or(PoolError::Overflow)?;
            require!(now > period_end, PoolError::WriteOffTooEarly);
        }
        _ => return err!(PoolError::LineStatus),
    }

    let line = &mut ctx.accounts.line;

    let loss = line.drawn.checked_sub(line.principal_repaid).ok_or(PoolError::Overflow)?;
    let loss_usdc = line.usdc_drawn.checked_sub(line.usdc_cost_repaid).ok_or(PoolError::Overflow)?;
    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_sub(loss_usdc).ok_or(PoolError::Overflow)?;
    pool.written_off = pool.written_off.checked_add(loss_usdc).ok_or(PoolError::Overflow)?;
    line.status = LineStatus::WrittenOff;
    line.closed_at = now;

    // No CPI in this instruction, so the balances read by Anchor are current.
    let share_supply = ctx.accounts.share_mint.supply;
    let nav_after = pool.check(ctx.accounts.vault.amount, Some(share_supply))?;
    emit_cpi!(LineWrittenOff {
        pool: pool.key(),
        line_id: line.line_id,
        loss,
        loss_usdc,
        nav_after,
    });
    emit_cpi!(NavUpdated { pool: pool.key(), total_assets: nav_after, share_supply });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct CouncilLine<'info> {
    pub council: Signer<'info>,

    /// CHECK: `council::check` against the pool's stored multisig.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = council @ PoolError::Unauthorized,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        mut,
        seeds = [seeds::LINE, pool.key().as_ref(), &line.line_id.to_le_bytes()],
        bump = line.bump,
        has_one = pool @ PoolError::WrongAccount,
    )]
    pub line: Box<Account<'info, CreditLine>>,
}

impl CouncilLine<'_> {
    fn check_council(&self) -> Result<()> {
        council::check(&self.multisig.to_account_info(), &self.pool.council, self.pool.council_vault_bump)
    }
}

/// Closes a draw window still open, keeping its original end in `availability_end_saved` so
/// `resume_draws` can restore it. A window already closed (by date or by a suspension) has nothing
/// to suspend and is left alone, so `availability_end_saved != 0` always means a suspension the UI
/// shows. Returns the undrawn remainder.
fn suspend(line: &mut CreditLine, now: i64) -> Result<u64> {
    if now < line.availability_end {
        line.availability_end_saved = line.availability_end;
        line.availability_end = now;
    }
    line.available().ok_or(error!(PoolError::Overflow))
}

/// Council only: closes the open draw window of an `Approved` or `Active` line now (`WindowClosed`
/// if it already closed, by date or by a suspension); the line is then suspended
/// (`availability_end_saved != 0`) until `resume_draws`. Repayment, collection and
/// write-off are untouched; the pool is not paused.
pub fn suspend_draws(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let now = Clock::get()?.unix_timestamp;
    let line = &mut ctx.accounts.line;
    require!(matches!(line.status, LineStatus::Approved | LineStatus::Active), PoolError::LineStatus);
    require!(now < line.availability_end, PoolError::WindowClosed);
    let available_forfeited = suspend(line, now)?;
    emit_cpi!(LineDrawsSuspended { pool: ctx.accounts.pool.key(), line_id: line.line_id, available_forfeited });
    Ok(())
}

/// Council only: reopens the draw window of a suspended `Approved` or `Active` line to its original
/// end, never later (`DrawsNotSuspended` if not suspended); refused while an acceleration is
/// pending (`AccelerationState`).
pub fn resume_draws(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let line = &mut ctx.accounts.line;
    require!(matches!(line.status, LineStatus::Approved | LineStatus::Active), PoolError::LineStatus);
    require!(line.availability_end_saved != 0, PoolError::DrawsNotSuspended);
    require!(line.acceleration_due_at == 0, PoolError::AccelerationState);
    line.availability_end = line.availability_end_saved;
    line.availability_end_saved = 0;
    emit_cpi!(LineDrawsResumed { pool: ctx.accounts.pool.key(), line_id: line.line_id, availability_end: line.availability_end });
    Ok(())
}

/// Council only: proposes to collect a drawn (`Active`) line early; a never-drawn line is cleared
/// with `cancel_line` instead (`LineStatus`). An open draw window is suspended at once (one already
/// closed is left alone), and `accelerate_line`
/// may fire from `now + acceleration_notice_seconds`, a deadline the proposal keeps whatever the
/// pool's notice becomes. One at a time (`AccelerationState`).
pub fn propose_acceleration(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let now = Clock::get()?.unix_timestamp;
    let notice = i64::from(ctx.accounts.pool.acceleration_notice_seconds);
    let line = &mut ctx.accounts.line;
    require!(line.status == LineStatus::Active && line.drawn > 0, PoolError::LineStatus);
    require!(line.acceleration_due_at == 0, PoolError::AccelerationState);
    let due_at = now.checked_add(notice).ok_or(PoolError::Overflow)?;
    line.acceleration_due_at = due_at;
    suspend(line, now)?;
    emit_cpi!(AccelerationProposed { pool: ctx.accounts.pool.key(), line_id: line.line_id, due_at });
    Ok(())
}

/// Council only: withdraws the pending proposal (`AccelerationState` if none). Draws stay
/// suspended until `resume_draws`.
pub fn withdraw_acceleration(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let line = &mut ctx.accounts.line;
    require!(line.status == LineStatus::Active && line.acceleration_due_at != 0, PoolError::AccelerationState);
    line.acceleration_due_at = 0;
    emit_cpi!(AccelerationWithdrawn { pool: ctx.accounts.pool.key(), line_id: line.line_id });
    Ok(())
}

/// The acceleration both routes share: the ordinary interest kept at the mark and the remaining
/// balance. A scheduled line keeps the interest of the installments due at the mark (`due_count`);
/// a bullet line keeps the interest accrued on each tranche from its draw to the mark. Never less
/// than what was repaid. Returns `(kept, interest_forgiven, remaining)`.
fn accelerated_balance(line: &CreditLine, due_count: u16, now: i64) -> Result<(u64, u64, u64)> {
    let kept = match line.schedule() {
        Some(s) => math::accelerated_interest(line.interest_due, due_count, s.installments).ok_or(PoolError::Overflow)?.0,
        // Rounded once over every tranche, the accrual can pass the per-tranche `interest_due`
        // by a few units at the last due date: clamped (see `math::bullet_accrued_interest`).
        None => math::bullet_accrued_interest(line.drawn, line.draw_time_weight, line.rate_bps, now)
            .ok_or(PoolError::Overflow)?
            .min(line.interest_due),
    };
    let kept = kept.max(line.interest_repaid);
    let interest_forgiven = line.interest_due.checked_sub(kept).ok_or(PoolError::Overflow)?;
    let remaining = line
        .drawn
        .checked_sub(line.principal_repaid)
        .and_then(|v| v.checked_add(kept.checked_sub(line.interest_repaid)?))
        .ok_or(PoolError::Overflow)?;
    Ok((kept, interest_forgiven, remaining))
}

/// Council only: fires a pending proposal once its notice has run (`AccelerationNotDue`): the
/// line moves to `InCollection` with `accelerated_at = now`, as `mark_in_collection` would, without
/// missed installments. A scheduled line keeps the ordinary interest of the installments due now
/// (none due: only what was repaid); a bullet line keeps the interest accrued to now; the rest is
/// forgiven. `acceleration_due_at` stays set: with `accelerated_at` it records the council's route.
pub fn accelerate_line(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let now = Clock::get()?.unix_timestamp;
    let pool_key = ctx.accounts.pool.key();
    let line = &ctx.accounts.line;
    require!(line.status == LineStatus::Active && line.acceleration_due_at != 0, PoolError::AccelerationState);
    require!(now >= line.acceleration_due_at, PoolError::AccelerationNotDue);
    let due_count = match line.schedule() {
        Some(_) => line_arrears(line, now)?.due,
        None => 0,
    };
    let (kept, interest_forgiven, remaining) = accelerated_balance(line, due_count, now)?;
    let line = &mut ctx.accounts.line;
    line.status = LineStatus::InCollection;
    line.accelerated_at = now;
    line.interest_due = kept;
    emit_cpi!(LineAccelerated { pool: pool_key, line_id: line.line_id, remaining, interest_forgiven });
    Ok(())
}

/// Council only: clears an approved commitment nobody drew (`Approved`, `drawn == 0`); once a
/// tranche exists the line follows repayment, collection or write-off instead. `Cancelled` is
/// terminal; the line id stays taken.
pub fn cancel_line(ctx: Context<CouncilLine>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let now = Clock::get()?.unix_timestamp;
    let line = &mut ctx.accounts.line;
    require!(line.status == LineStatus::Approved && line.drawn == 0, PoolError::LineStatus);
    line.status = LineStatus::Cancelled;
    line.closed_at = now;
    emit_cpi!(LineCancelled { pool: ctx.accounts.pool.key(), line_id: line.line_id });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{dec, dec_u, hex};

    #[test]
    fn decimal_text_matches_the_standard_formatting_at_the_edges() {
        for n in [0u64, 9, 10, 99, 1_000_000, u64::MAX] {
            assert_eq!(dec_u(n), n.to_string());
        }
        for n in [0i64, -1, 1, i64::MAX, i64::MIN] {
            assert_eq!(dec(n), n.to_string());
        }
    }

    #[test]
    fn hex_is_lower_case_and_two_digits_per_byte() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(hex(&[0xab; 32]).len(), 64);
    }
}
