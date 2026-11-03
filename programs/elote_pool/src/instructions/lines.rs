//! Credit lines approved from the pool: open, draw, repay, collection and write off.
//!
//! A line moves `Approved -> Active -> Repaid | WrittenOff`, or `Approved -> Cancelled` if nothing
//! was drawn; a scheduled line can also move `Active -> InCollection -> Repaid | WrittenOff`. The
//! pause blocks only openings and draws. Repayments go to late interest, then interest,
//! then principal. Collected interest raises the NAV; a written-off principal lowers it once.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self as token, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{
    LateInterestCollected, LineCancelled, LineDrawn, LineDrawsSuspended, LineInCollection, LineOpened,
    LineRepaid, LineWrittenOff, NavUpdated, PagareEntry, PagareIssued,
    PagarePayment, PaymentEntry,
};
use crate::math::{self, Arrears, ScheduleTerms};
use crate::pool_signer;
use crate::registry;
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, ScheduleParams, BPS, KIND_ABONO, KIND_EMISION, KIND_PAGADO,
    MAX_DRAWS, MAX_INSTALLMENTS, MIN_MAX_MISSED, PAGARE_OUTSTANDING, PAGARE_PAID_PENDING_CANCEL,
    SIGNATURE_EFIRMA_WALLET, SIGNATURE_WALLET, SPONSOR_GUARANTOR,
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(line_id: u64)]
pub struct OpenLine<'info> {
    #[account(mut)]
    pub council: Signer<'info>,

    pub borrower: Signer<'info>,

    /// The sponsor named in the arguments; required (and signing) when one is named.
    pub sponsor: Option<Signer<'info>>,

    /// Attests that the signed agreement behind `agreement_hash` exists and matches the arguments.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = council @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = council,
        space = 8 + CreditLine::INIT_SPACE,
        seeds = [seeds::LINE, pool.key().as_ref(), &line_id.to_le_bytes()],
        bump,
    )]
    pub line: Account<'info, CreditLine>,

    /// Read for the NAV the concentration cap is measured against.
    #[account(address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub system_program: Program<'info, System>,
}

/// Opens an `Approved` line, signed by the council, the borrower, the attestor and the named sponsor
/// if any; the borrower may be neither the council nor the attestor. `line_id` must be the pool's next
/// id, and `principal` at most `max_line_bps` of the NAV (`LineTooLarge`). Refused while paused.
#[allow(clippy::too_many_arguments)]
pub fn open_line(
    ctx: Context<OpenLine>,
    line_id: u64,
    principal: u64,
    rate_bps: u16,
    term_end: i64,
    agreement_hash: [u8; 32],
    sponsor: Option<Pubkey>,
    sponsor_role: u8,
    schedule: Option<ScheduleParams>,
    expediente_root: [u8; 32],
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &mut ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    require!(line_id == pool.line_count, PoolError::WrongAccount);
    require!(ctx.accounts.borrower.key() != pool.council, PoolError::BorrowerIsCouncil);
    require!(ctx.accounts.borrower.key() != pool.attestor, PoolError::BorrowerIsAttestor);
    require!(principal > 0, PoolError::WrongAmount);
    // The cap is measured against the NAV at this moment, principal lent out included; an empty
    // pool opens no line.
    let nav = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    let cap = pool.max_line_principal(nav).ok_or(PoolError::Overflow)?;
    require!(principal <= cap, PoolError::LineTooLarge);
    require!(u64::from(rate_bps) <= BPS, PoolError::WrongAmount);
    require!(term_end > now, PoolError::TermEnded);
    require!(agreement_hash != [0u8; 32], PoolError::AgreementHashRequired);
    require!(expediente_root != [0u8; 32], PoolError::ExpedienteRootRequired);
    require!(sponsor_role <= SPONSOR_GUARANTOR, PoolError::SponsorRoleInvalid);
    if let Some(named) = sponsor {
        let signed = ctx.accounts.sponsor.as_ref().is_some_and(|s| s.key() == named);
        require!(signed, PoolError::SponsorRequired);
    }
    // The draw window closes at the first due date on a scheduled line, at the term on a bullet
    // line.
    let (first_due_at, availability_end) = match schedule {
        Some(s) => {
            validate_schedule(&s, principal, rate_bps, now, term_end)?;
            (s.first_due_at, s.first_due_at)
        }
        None => (0, term_end),
    };

    let line = &mut ctx.accounts.line;
    line.pool = pool.key();
    line.line_id = line_id;
    line.borrower = ctx.accounts.borrower.key();
    line.sponsor = sponsor;
    line.agreement_hash = agreement_hash;
    line.principal = principal;
    line.rate_bps = rate_bps;
    line.interest_due = 0;
    line.term_end = term_end;
    line.principal_repaid = 0;
    line.interest_repaid = 0;
    line.status = LineStatus::Approved;
    line.opened_at = now;
    line.drawn_at = 0;
    line.closed_at = 0;
    let (interval_seconds, installments, late_bps, max_missed) =
        schedule.map_or((0, 0, 0, 0), |s| (s.interval_seconds, s.installments, s.late_bps, s.max_missed));
    line.interval_seconds = interval_seconds;
    line.installments = installments;
    line.late_bps = late_bps;
    line.max_missed = max_missed;
    line.late_collected = 0;
    line.late_paid_until = 0;
    line.pagares_paid = 0;
    line.bump = ctx.bumps.line;
    line.reserved = [0; 64];
    line.drawn = 0;
    line.draws = 0;
    line.availability_end = availability_end;
    line.first_due_at = first_due_at;
    line.sponsor_role = sponsor_role;
    line.expediente_root = expediente_root;
    line.accelerated_at = 0;

    pool.line_count = pool.line_count.checked_add(1).ok_or(PoolError::Overflow)?;

    emit_cpi!(LineOpened {
        pool: pool.key(),
        line_id,
        borrower: line.borrower,
        sponsor,
        sponsor_role,
        agreement_hash,
        expediente_root,
        principal,
        rate_bps,
        term_end,
        availability_end,
        first_due_at,
        interval_seconds,
        installments,
        late_bps,
        max_missed,
    });
    Ok(())
}

/// Checks the schedule against the line's terms: at least one and at most `MAX_INSTALLMENTS`
/// installments, a positive interval, `late_bps <= BPS`, `MIN_MAX_MISSED <= max_missed <=
/// installments`, a first due date after the opening, the last due date
/// (`first_due_at + (installments - 1) * interval`) no later than `term_end`, and at least one
/// unit per installment if the whole commitment were drawn now (`draw` re-checks on what is
/// actually drawn). The dates are fixed by the contract and not re-checked at the draw.
fn validate_schedule(s: &ScheduleParams, principal: u64, rate_bps: u16, now: i64, term_end: i64) -> Result<()> {
    require!(s.installments >= 1 && s.installments <= MAX_INSTALLMENTS, PoolError::InvalidSchedule);
    require!(s.interval_seconds > 0, PoolError::InvalidSchedule);
    require!(u64::from(s.late_bps) <= BPS, PoolError::InvalidSchedule);
    require!(s.max_missed >= MIN_MAX_MISSED && u16::from(s.max_missed) <= s.installments, PoolError::InvalidSchedule);
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
#[instruction(amount: u64, document_hash: [u8; 32], asset_bump: u8, pagare_bump: u8)]
pub struct Draw<'info> {
    pub borrower: Signer<'info>,

    /// Attests that the pagaré behind `document_hash` was signed by the borrower and is conserved.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

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
        has_one = borrower @ PoolError::Unauthorized,
    )]
    pub line: Account<'info, CreditLine>,

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
/// - `ImmutableMetadata` and `Attributes` (the nine facts, 32-byte values as hex), authority
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
        (record.draw_index, record.amount, record.pagare_hash, record.issued_at, record.due_at);
    use mpl_core::types::{
        AddBlocker, Attribute, Attributes, ImmutableMetadata, PermanentFreezeDelegate, Plugin, PluginAuthority,
        PluginAuthorityPair,
    };
    let pool = &accounts.pool;
    let line = &accounts.line;
    // Hex, not base58: base58 of a 32-byte key is too costly on chain.
    let attributes = vec![
        Attribute { key: "pagare_hash".into(), value: hex(&document_hash) },
        Attribute { key: "amount".into(), value: dec_u(amount) },
        Attribute { key: "currency".into(), value: hex(pool.asset_mint.as_ref()) },
        Attribute { key: "issued_at".into(), value: dec(issued_at) },
        Attribute { key: "due_at".into(), value: dec(due_at) },
        Attribute { key: "line".into(), value: dec_u(line.line_id) },
        Attribute { key: "draw_index".into(), value: dec_u(draw_index.into()) },
        Attribute { key: "cert_fingerprint".into(), value: hex(&record.cert_fingerprint) },
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

/// One tranche, signed by the borrower and the attestor, with its pagaré asset and record. Every
/// check runs before anything is written, in this order: not paused;
/// `Approved` or `Active`; `0 < amount <= principal - drawn`; `now < availability_end`, strictly
/// (an installment counts as due at the second it falls due); on a scheduled line
/// `drawn + interest_due >= installments` after the tranche, so every installment keeps at least
/// one unit; fewer than `MAX_DRAWS` tranches; the vault has the liquidity; a non-zero
/// `document_hash`; a valid signature method. The tranche's interest, at the annual rate from now
/// to the last due date, is fixed here and added to `interest_due`.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: Context<Draw>,
    amount: u64,
    document_hash: [u8; 32],
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
    require!(amount > 0 && amount <= available, PoolError::WrongAmount);
    let now = Clock::get()?.unix_timestamp;
    require!(now < line.availability_end, PoolError::AvailabilityEnded);
    let last_due = line.last_due().ok_or(PoolError::Overflow)?;
    let seconds = u64::try_from(last_due.checked_sub(now).ok_or(PoolError::Overflow)?).map_err(|_| PoolError::Overflow)?;
    let interest = math::tranche_interest(amount, line.rate_bps, seconds).ok_or(PoolError::Overflow)?;
    let drawn_after = line.drawn.checked_add(amount).ok_or(PoolError::Overflow)?;
    let interest_after = line.interest_due.checked_add(interest).ok_or(PoolError::Overflow)?;
    if let Some(s) = line.schedule() {
        let total_after = drawn_after.checked_add(interest_after).ok_or(PoolError::Overflow)?;
        require!(total_after >= u64::from(s.installments), PoolError::InvalidSchedule);
    }
    require!(line.draws < MAX_DRAWS, PoolError::TooManyDraws);
    require!(amount <= ctx.accounts.vault.amount, PoolError::InsufficientLiquidity);
    require!(document_hash != [0u8; 32], PoolError::DocumentHashRequired);
    // The certificate fingerprint may be zero only for a wallet-only signature.
    require!(
        signature_method <= SIGNATURE_EFIRMA_WALLET && (signature_method == SIGNATURE_WALLET || cert_fingerprint != [0u8; 32]),
        PoolError::SignatureMethodInvalid
    );

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
        amount,
        ctx.accounts.asset_mint.decimals,
    )?;

    let draw_index = ctx.accounts.line.draws;
    let record = Pagare {
        line: ctx.accounts.line.key(),
        draw_index,
        asset: ctx.accounts.asset.key(),
        pagare_hash: document_hash,
        amount,
        issued_at: now,
        due_at: last_due,
        drawn_through: drawn_after,
        cert_fingerprint,
        signature_method,
        status: PAGARE_OUTSTANDING,
        paid_at: 0,
        receipt_hash: [0; 32],
        cancel_receipt_hash: [0; 32],
        cancel_after: 0,
        requested_receipt_hash: [0; 32],
        procurador: Pubkey::default(),
        flags: 0,
        log_root: [0; 32],
        entry_count: 0,
        last_kind: 0,
        bump: pagare_bump,
        reserved: [0; 32],
    };
    // The registry opens with the issuance entry, from and to the pool, over the pagaré's hash.
    let mut record = record;
    let pool_key = ctx.accounts.pool.key();
    let entry_hash =
        registry::append(&mut record, KIND_EMISION, &pool_key, &pool_key, &document_hash, now).ok_or(PoolError::Overflow)?;
    issue_pagare(ctx.accounts, asset_bump, &record)?;
    create_sidecar(ctx.accounts, pagare_bump, &record)?;

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_add(amount).ok_or(PoolError::Overflow)?;
    let line = &mut ctx.accounts.line;
    line.drawn = drawn_after;
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
        amount,
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
        amount,
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
/// must cover the late interest due (`LateInterestUnpaid`; zero on a bullet line); the payment
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
/// once the cursor has passed the last draw (`PagareAccountsMismatch` otherwise). A covered pagaré
/// gets a `Pagado` entry, status `PAGARE_PAID_PENDING_CANCEL`, `paid_at` and `receipt_hash` (only
/// the burn waits), and the cursor `pagares_paid` moves past it. The uncovered one gets an `Abono`
/// entry only if the payment applied principal to it. `receipt_hash` is the payer's, unverified and
/// provisional until the holder annotates the real one (`annotate_payment`); zero means "receipt to
/// follow".
pub fn repay<'info>(ctx: Context<'info, Repay<'info>>, amount: u64, receipt_hash: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let schedule = line.schedule();
    require!(matches!(line.status, LineStatus::Active | LineStatus::InCollection), PoolError::LineStatus);

    let principal_left = line.drawn.checked_sub(line.principal_repaid).ok_or(PoolError::Overflow)?;
    let interest_left = line.interest_due.checked_sub(line.interest_repaid).ok_or(PoolError::Overflow)?;
    let remaining = principal_left.checked_add(interest_left).ok_or(PoolError::Overflow)?;
    require!(amount > 0, PoolError::WrongAmount);

    let late_part = match schedule {
        Some(_) => line_arrears(line, pool, now)?.late_due,
        None => 0,
    };
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

    // The records, exactly `[pagares_paid ..= last_touched]`, are verified before any write.
    let line_key = line.key();
    let cursor = line.pagares_paid;
    let draws = line.draws;
    let sidecars = ctx.remaining_accounts;
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

    if transfer > 0 {
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
            transfer,
            ctx.accounts.asset_mint.decimals,
        )?;
    }

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool
        .outstanding_principal
        .checked_sub(principal_part)
        .ok_or(PoolError::Overflow)?;
    pool.interest_collected = pool.interest_collected.checked_add(interest_part).ok_or(PoolError::Overflow)?;

    let line = &mut ctx.accounts.line;
    line.principal_repaid = line.principal_repaid.checked_add(principal_part).ok_or(PoolError::Overflow)?;
    line.interest_repaid = line.interest_repaid.checked_add(interest_part).ok_or(PoolError::Overflow)?;
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
    emit_cpi!(LineRepaid {
        pool: pool.key(),
        line_id: line.line_id,
        payer: ctx.accounts.payer.key(),
        principal_part,
        interest_part,
        remaining: remaining_after,
        nav_after,
    });
    emit_cpi!(NavUpdated { pool: pool.key(), total_assets: nav_after, share_supply });
    Ok(())
}

/// The arrears of a scheduled line at `now`: the installments repay `drawn + interest_due` on the
/// contract's calendar, and late interest accrues on the principal share, `drawn`, of what is
/// overdue.
fn line_arrears(line: &CreditLine, pool: &Pool, now: i64) -> Result<Arrears> {
    let s = line.schedule().ok_or(PoolError::NotScheduled)?;
    let terms = ScheduleTerms {
        total: line.drawn.checked_add(line.interest_due).ok_or(PoolError::Overflow)?,
        anchor: line.anchor().ok_or(PoolError::Overflow)?,
        interval: s.interval_seconds,
        installments: s.installments,
        late_bps: s.late_bps,
        grace: pool.grace_period_seconds,
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
/// missed and the grace period of the last of them has run: with `covered` installments paid,
/// `now > anchor + (covered + max_missed) * interval + grace`, where the anchor is one interval
/// before `first_due_at`. The mark accelerates the balance (vencimiento anticipado):
/// `accelerated_at = now`, from which every installment is due and late interest runs on all
/// remaining principal, and the ordinary interest of the installments not yet due is forgiven
/// (`interest_due` shrinks to the share of the installments due, never below what was repaid; no
/// NAV effect, since interest counts when collected). It records facts and moves no funds, so it
/// is not blocked by the pause.
pub fn mark_in_collection(ctx: Context<MarkInCollection>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let s = line.schedule().ok_or(PoolError::NotScheduled)?;
    require!(line.status == LineStatus::Active, PoolError::LineStatus);
    let a = line_arrears(line, pool, now)?;
    let k = a.covered.checked_add(u16::from(s.max_missed)).ok_or(PoolError::Overflow)?;
    require!(k <= s.installments, PoolError::NotEnoughMissed);
    let anchor = line.anchor().ok_or(PoolError::Overflow)?;
    let grace_end = s
        .interval_seconds
        .checked_mul(i64::from(k))
        .and_then(|span| anchor.checked_add(span))
        .and_then(|due| due.checked_add(i64::from(pool.grace_period_seconds)))
        .ok_or(PoolError::Overflow)?;
    require!(now > grace_end, PoolError::NotEnoughMissed);

    // The ordinary interest of the installments not yet due is forgiven: the line keeps the share
    // of the installments due at the mark, at least the ones that put it in collection, and never
    // less than what was already repaid.
    let due_count = a.due.max(k);
    let (kept, _) = math::accelerated_interest(line.interest_due, due_count, s.installments).ok_or(PoolError::Overflow)?;
    let kept = kept.max(line.interest_repaid);
    let interest_forgiven = line.interest_due.checked_sub(kept).ok_or(PoolError::Overflow)?;
    let remaining = line
        .drawn
        .checked_sub(line.principal_repaid)
        .and_then(|v| v.checked_add(kept.checked_sub(line.interest_repaid)?))
        .ok_or(PoolError::Overflow)?;
    let line = &mut ctx.accounts.line;
    line.status = LineStatus::InCollection;
    line.accelerated_at = now;
    line.interest_due = kept;
    emit_cpi!(LineInCollection { pool: pool.key(), line_id: line.line_id, missed: a.missed, remaining, interest_forgiven });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct WriteOff<'info> {
    pub council: Signer<'info>,

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
/// - the line is `Active` and `now > term_end + grace` (else `TermNotEnded`), or
/// - the line is `InCollection`, the collection period has run (`now > accelerated_at +
///   collection_period_seconds`), and either a missed installment is still unpaid and
///   `now > first_missed_due + grace`, or `now > term_end + grace` (else `WriteOffTooEarly`).
///
/// `grace` is the pool's grace period.
pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let grace = i64::from(pool.grace_period_seconds);
    let after_term = now > line.term_end.checked_add(grace).ok_or(PoolError::Overflow)?;
    match line.status {
        LineStatus::Active => require!(after_term, PoolError::TermNotEnded),
        LineStatus::InCollection => {
            let after_missed = match line_arrears(line, pool, now)?.first_missed_due {
                Some(due) => now > due.checked_add(grace).ok_or(PoolError::Overflow)?,
                None => false, // nothing is missed
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
    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_sub(loss).ok_or(PoolError::Overflow)?;
    pool.written_off = pool.written_off.checked_add(loss).ok_or(PoolError::Overflow)?;
    line.status = LineStatus::WrittenOff;
    line.closed_at = now;

    // No CPI in this instruction, so the balances read by Anchor are current.
    let share_supply = ctx.accounts.share_mint.supply;
    let nav_after = pool.check(ctx.accounts.vault.amount, Some(share_supply))?;
    emit_cpi!(LineWrittenOff {
        pool: pool.key(),
        line_id: line.line_id,
        loss,
        nav_after,
    });
    emit_cpi!(NavUpdated { pool: pool.key(), total_assets: nav_after, share_supply });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct SuspendDraws<'info> {
    pub council: Signer<'info>,

    #[account(
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
}

/// Council only: closes the draw window of an `Approved` or `Active` line now. The undrawn
/// remainder is forfeited; repayment, collection and write-off are untouched; the pool is not
/// paused.
pub fn suspend_draws(ctx: Context<SuspendDraws>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let line = &mut ctx.accounts.line;
    require!(matches!(line.status, LineStatus::Approved | LineStatus::Active), PoolError::LineStatus);
    let available_forfeited = line.available().ok_or(PoolError::Overflow)?;
    line.availability_end = now;
    emit_cpi!(LineDrawsSuspended { pool: ctx.accounts.pool.key(), line_id: line.line_id, available_forfeited });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct CancelLine<'info> {
    pub council: Signer<'info>,

    #[account(
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
}

/// Council only: clears an approved commitment nobody drew (`Approved`, `drawn == 0`); once a
/// tranche exists the line follows repayment, collection or write-off instead. `Cancelled` is
/// terminal; the line id stays taken.
pub fn cancel_line(ctx: Context<CancelLine>) -> Result<()> {
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
