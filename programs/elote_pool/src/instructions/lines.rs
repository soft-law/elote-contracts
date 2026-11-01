//! Credit lines approved from the pool: open, draw, repay, collection and write off.
//!
//! A line moves `Approved -> Active -> Repaid | WrittenOff`, and a scheduled line can also move
//! `Active -> InCollection -> Repaid | WrittenOff`. The pause blocks openings and draws (and, in
//! `pool.rs`, deposits and redemptions); `repay`, `mark_in_collection` and `write_off` always
//! run. On a single-maturity line (version 1) repayments go to principal first, then to the fixed
//! interest. On a scheduled line (version 2) they go to late interest, then interest, then
//! principal. Collected interest and late interest raise the NAV; a written-off principal lowers
//! it once.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self as token, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{
    LateInterestCollected, LineCancelled, LineDrawState, LineDrawn, LineDrawsSuspended, LineInCollection, LineOpened,
    LineRepaid, LineScheduled, LineSponsorRole, LineTermsSet, LineWrittenOff, NavUpdated, PagareIssued,
};
use crate::math::{self, Arrears, ScheduleTerms};
use crate::pool_signer;
use crate::state::{
    seeds, CreditLine, LineStatus, Pagare, Pool, Schedule, ScheduleParams, BPS, LINE_VERSION, MAX_DRAWS,
    MAX_INSTALLMENTS, MIN_MAX_MISSED, SPONSOR_GUARANTOR,
};

// -----------------------------------------------------------------------------------------------
// open_line
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
#[instruction(line_id: u64)]
pub struct OpenLine<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    pub borrower: Signer<'info>,

    /// The sponsor named in the arguments; required (and signing) when one is named.
    pub sponsor: Option<Signer<'info>>,

    /// The pool's attestor (D16): the agreements engine attests that the signed agreement behind
    /// `agreement_hash` exists and matches the arguments.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = admin,
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
    require!(ctx.accounts.borrower.key() != pool.admin, PoolError::BorrowerIsAdmin);
    require!(principal > 0, PoolError::WrongAmount);
    // Concentration cap (decision 20): the commitment is at most `max_line_bps` of the NAV at
    // this moment, principal lent out included. An empty pool opens no line.
    let nav = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    let cap = pool.max_line_principal(nav).ok_or(PoolError::Overflow)?;
    require!(principal <= cap, PoolError::LineTooLarge);
    require!(u64::from(rate_bps) <= BPS, PoolError::WrongAmount);
    require!(term_end > now, PoolError::WrongAmount);
    require!(agreement_hash != [0u8; 32], PoolError::WrongAmount);
    require!(expediente_root != [0u8; 32], PoolError::WrongAmount);
    require!(sponsor_role <= SPONSOR_GUARANTOR, PoolError::WrongAmount);
    if let Some(named) = sponsor {
        let signed = ctx.accounts.sponsor.as_ref().is_some_and(|s| s.key() == named);
        require!(signed, PoolError::SponsorRequired);
    }
    // The draw window (D3, decision 25): until the first due date on a scheduled line, until the
    // term on a bullet line; fixed here, since the calendar comes from the contract.
    let (first_due_at, availability_end) = match schedule {
        Some(s) => {
            validate_schedule(&s, principal, rate_bps, now, term_end)?;
            (s.first_due_at, s.first_due_at)
        }
        None => (0, term_end),
    };

    let line = &mut ctx.accounts.line;
    line.version = LINE_VERSION;
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
    line.reserved = [0; 32];
    if let Some(s) = schedule {
        line.set_schedule(&Schedule {
            interval_seconds: s.interval_seconds,
            installments: s.installments,
            late_bps: s.late_bps,
            max_missed: s.max_missed,
            flags: 0,
            late_collected: 0,
            late_paid_until: 0,
        });
    }
    line.bump = ctx.bumps.line;
    line.drawn = 0;
    line.draws = 0;
    line.availability_end = availability_end;
    line.first_due_at = first_due_at;
    line.sponsor_role = sponsor_role;
    line.expediente_root = expediente_root;

    pool.line_count = pool.line_count.checked_add(1).ok_or(PoolError::Overflow)?;

    emit_cpi!(LineOpened {
        pool: pool.key(),
        line_id,
        borrower: line.borrower,
        sponsor,
        agreement_hash,
        principal,
        rate_bps,
        interest_due: 0,
        term_end,
    });
    if let Some(s) = schedule {
        emit_cpi!(LineScheduled {
            pool: pool.key(),
            line_id,
            interval_seconds: s.interval_seconds,
            installments: s.installments,
            late_bps: s.late_bps,
            max_missed: s.max_missed,
        });
    }
    emit_cpi!(LineTermsSet { pool: pool.key(), line_id, first_due_at, availability_end, expediente_root });
    emit_cpi!(LineSponsorRole { pool: pool.key(), line_id, sponsor, role: sponsor_role });
    Ok(())
}

/// Checks the schedule against the line's terms: at least one and at most `MAX_INSTALLMENTS`
/// installments, a positive interval, `late_bps <= BPS`, `MIN_MAX_MISSED <= max_missed <=
/// installments`, a first due date after the opening, the last due date
/// (`first_due_at + (installments - 1) * interval`) no later than `term_end`, and at least one
/// unit per installment if the whole commitment were drawn now (the fully-drawn-today interest;
/// `draw` re-checks payability on what is actually drawn, D3b). Nothing is re-checked at the
/// draw about the dates: they are fixed by the contract (decision 25).
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

// -----------------------------------------------------------------------------------------------
// draw
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct Draw<'info> {
    pub borrower: Signer<'info>,

    /// The pool's attestor (D14): the agreements engine attests that the pagaré behind
    /// `document_hash` was signed by the borrower and is conserved. No money moves without it.
    #[account(constraint = attestor.key() == pool.attestor @ PoolError::AttestorRequired)]
    pub attestor: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
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

    // -- the pagaré (decision 24) --

    /// Pays the rent of the pagaré asset and its sidecar: the app's relayer, since SME wallets
    /// rarely hold SOL. Any signer.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The Metaplex Core asset that is the pagaré, created by the Core program in this
    /// instruction at a program-derived address, so clients need no keypair for it.
    /// CHECK: a PDA this program derives and signs for; the Core program initialises it.
    #[account(
        mut,
        seeds = [seeds::PAGARE_ASSET, line.key().as_ref(), &line.draws.to_le_bytes()],
        bump,
    )]
    pub asset: UncheckedAccount<'info>,

    /// The program's record of the pagaré.
    #[account(
        init,
        payer = payer,
        space = 8 + Pagare::INIT_SPACE,
        seeds = [seeds::PAGARE, line.key().as_ref(), &line.draws.to_le_bytes()],
        bump,
    )]
    pub pagare: Account<'info, Pagare>,

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

/// Decimal text of `n`, without the formatting machinery (`to_string` on an integer costs a few
/// hundred CU on chain; the pagaré carries five of them and the name two more).
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

/// Mints the pagaré of a disposición (decision 24): one Metaplex Core asset in the pool's
/// collection, owned by the pool PDA (the vehicle's custody) and frozen from creation, with
/// `PermanentFreezeDelegate` and `PermanentTransferDelegate` held by the pool PDA (so only the
/// program can ever move it), `ImmutableMetadata`, an `Attributes` plugin nobody can change
/// (`pagare_hash`, `amount`, `currency`, `issued_at`, `due_at`, `line`, `draw_index`) and an
/// empty `AppData` section whose data authority is the pool PDA, for the endoso log. The name
/// carries the line id and the draw index and nothing else; the URI is empty. Nothing personal.
#[allow(clippy::too_many_arguments)]
fn issue_pagare<'info>(
    accounts: &Draw<'info>,
    asset_bump: u8,
    draw_index: u16,
    amount: u64,
    document_hash: [u8; 32],
    issued_at: i64,
    due_at: i64,
) -> Result<()> {
    use mpl_core::types::{
        AppDataInitInfo, Attribute, Attributes, ExternalPluginAdapterInitInfo, ExternalPluginAdapterSchema,
        ImmutableMetadata, PermanentFreezeDelegate, PermanentTransferDelegate, Plugin, PluginAuthority,
        PluginAuthorityPair,
    };
    let pool = &accounts.pool;
    let line = &accounts.line;
    let pool_key = pool.key();
    let by_pool = Some(PluginAuthority::Address { address: pool_key });
    // Hex, not base58: a base58 encoding of a 32-byte key costs tens of thousands of CU on chain.
    let attributes = vec![
        Attribute { key: "pagare_hash".into(), value: hex(&document_hash) },
        Attribute { key: "amount".into(), value: dec_u(amount) },
        Attribute { key: "currency".into(), value: hex(pool.asset_mint.as_ref()) },
        Attribute { key: "issued_at".into(), value: dec(issued_at) },
        Attribute { key: "due_at".into(), value: dec(due_at) },
        Attribute { key: "line".into(), value: dec_u(line.line_id) },
        Attribute { key: "draw_index".into(), value: dec_u(draw_index.into()) },
    ];
    let plugins = vec![
        PluginAuthorityPair {
            plugin: Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: true }),
            authority: by_pool.clone(),
        },
        PluginAuthorityPair { plugin: Plugin::PermanentTransferDelegate(PermanentTransferDelegate {}), authority: by_pool.clone() },
        PluginAuthorityPair { plugin: Plugin::ImmutableMetadata(ImmutableMetadata {}), authority: Some(PluginAuthority::None) },
        PluginAuthorityPair {
            plugin: Plugin::Attributes(Attributes { attribute_list: attributes }),
            authority: Some(PluginAuthority::None),
        },
    ];
    let adapters = vec![ExternalPluginAdapterInitInfo::AppData(AppDataInitInfo {
        data_authority: PluginAuthority::Address { address: pool_key },
        init_plugin_authority: by_pool,
        schema: Some(ExternalPluginAdapterSchema::Binary),
    })];

    let index_bytes = draw_index.to_le_bytes();
    let line_key = line.key();
    let pool_seeds = pool_signer!(pool);
    let asset_seeds: &[&[u8]] = &[seeds::PAGARE_ASSET, line_key.as_ref(), &index_bytes, &[asset_bump]];
    // The asset address is predictable once the line is open, and Core creates it with a plain
    // `create_account`, which refuses a funded address: left alone, a stranger could block every
    // draw of a line for the price of rent. The address is this program's PDA, so sweep whatever
    // was sent there to the payer first.
    let stray = accounts.asset.lamports();
    if stray > 0 {
        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(
                accounts.system_program.key(),
                anchor_lang::system_program::Transfer {
                    from: accounts.asset.to_account_info(),
                    to: accounts.payer.to_account_info(),
                },
                &[asset_seeds],
            ),
            stray,
        )?;
    }
    mpl_core::instructions::CreateV2CpiBuilder::new(&accounts.mpl_core_program.to_account_info())
        .asset(&accounts.asset.to_account_info())
        .collection(Some(&accounts.collection.to_account_info()))
        .authority(Some(&accounts.pool.to_account_info()))
        .payer(&accounts.payer.to_account_info())
        .owner(Some(&accounts.pool.to_account_info()))
        .system_program(&accounts.system_program.to_account_info())
        .name(["Pagare ", &dec_u(line.line_id), "-", &dec_u(draw_index.into())].concat())
        .uri(String::new())
        .plugins(plugins)
        .external_plugin_adapters(adapters)
        .invoke_signed(&[pool_seeds[0], asset_seeds])?;
    Ok(())
}

/// One tranche (D2 to D9). Every check runs before anything is written (D8), in this order: not
/// paused; `Approved` or `Active`; `0 < amount <= principal - drawn`; `now < availability_end`,
/// strictly, since an installment counts as due at the second it falls due; on a scheduled line
/// `drawn + interest_due >= installments` after the tranche, so every installment keeps at least
/// one unit (D3b); fewer than `MAX_DRAWS` tranches; the vault has the liquidity; a non-zero
/// `document_hash`. The tranche's interest, at the annual rate from now to the last due date, is
/// added to `interest_due` and never changes again.
pub fn draw(ctx: Context<Draw>, amount: u64, document_hash: [u8; 32]) -> Result<()> {
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

    // The pagaré: the Core asset (the título) and its sidecar, indexed by the draw being made.
    let draw_index = ctx.accounts.line.draws;
    issue_pagare(&ctx.accounts, ctx.bumps.asset, draw_index, amount, document_hash, now, last_due)?;
    let pagare = &mut ctx.accounts.pagare;
    pagare.line = ctx.accounts.line.key();
    pagare.draw_index = draw_index;
    pagare.asset = ctx.accounts.asset.key();
    pagare.pagare_hash = document_hash;
    pagare.amount = amount;
    pagare.issued_at = now;
    pagare.due_at = last_due;
    pagare.bump = ctx.bumps.pagare;

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
        amount,
        outstanding_after: pool.outstanding_principal,
    });
    emit_cpi!(LineDrawState {
        pool: pool.key(),
        line_id: line.line_id,
        amount,
        drawn: line.drawn,
        available: line.available().ok_or(PoolError::Overflow)?,
        interest_due: line.interest_due,
        availability_end: line.availability_end,
        document_hash,
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
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// repay
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct Repay<'info> {
    /// Anyone may repay: the borrower, a sponsor paying the interest, or a third party.
    pub payer: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
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

/// `amount` must cover the late interest due (zero on a bullet line); the repayment settles the
/// late interest, then interest, then principal, and takes at most what is due: the payer's
/// account is debited only that, never `amount` when it is larger. Accepted in `Active` and
/// `InCollection`. What is owed is what was drawn plus the interest fixed at each draw; a full
/// repayment closes the line and the undrawn remainder lapses with it (D5).
///
/// Never blocked by the pause: a borrower can always pay.
pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
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
    let rest = amount.checked_sub(late_part).ok_or(PoolError::Overflow)?.min(remaining);
    let interest_part = rest.min(interest_left);
    let principal_part = rest.checked_sub(interest_part).ok_or(PoolError::Overflow)?;
    let transfer = late_part
        .checked_add(interest_part)
        .and_then(|v| v.checked_add(principal_part))
        .ok_or(PoolError::Overflow)?;

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

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool
        .outstanding_principal
        .checked_sub(principal_part)
        .ok_or(PoolError::Overflow)?;
    pool.interest_collected = pool.interest_collected.checked_add(interest_part).ok_or(PoolError::Overflow)?;

    let line = &mut ctx.accounts.line;
    line.principal_repaid = line.principal_repaid.checked_add(principal_part).ok_or(PoolError::Overflow)?;
    line.interest_repaid = line.interest_repaid.checked_add(interest_part).ok_or(PoolError::Overflow)?;
    if let Some(mut s) = schedule {
        // The late interest accrued until now is settled, and the watermark moves, only when
        // something was actually collected. When the accrual rounds down to zero the watermark
        // stays, so the sub-unit accrual keeps running instead of being erased by every payment.
        if late_part > 0 {
            s.late_collected = s.late_collected.checked_add(late_part).ok_or(PoolError::Overflow)?;
            s.late_paid_until = now;
            line.set_schedule(&s);
        }
    }
    let remaining_after = remaining
        .checked_sub(interest_part)
        .and_then(|v| v.checked_sub(principal_part))
        .ok_or(PoolError::Overflow)?;
    if remaining_after == 0 {
        line.status = LineStatus::Repaid;
        line.closed_at = now;
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
/// contract's calendar (`first_due_at`, decision 25), and late interest accrues on the principal
/// share, `drawn`, of what is overdue.
fn line_arrears(line: &CreditLine, pool: &Pool, now: i64) -> Result<Arrears> {
    let s = line.schedule().ok_or(PoolError::NotScheduled)?;
    let terms = ScheduleTerms {
        total: line.drawn.checked_add(line.interest_due).ok_or(PoolError::Overflow)?,
        anchor: line.anchor().ok_or(PoolError::Overflow)?,
        interval: s.interval_seconds,
        installments: s.installments,
        late_bps: s.late_bps,
        grace: pool.grace_period_seconds(),
        late_paid_until: s.late_paid_until,
        principal: line.drawn,
        late_collected: s.late_collected,
    };
    let paid = line.principal_repaid.checked_add(line.interest_repaid).ok_or(PoolError::Overflow)?;
    Ok(math::arrears(&terms, paid, now).ok_or(PoolError::Overflow)?)
}

// -----------------------------------------------------------------------------------------------
// mark_in_collection
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct MarkInCollection<'info> {
    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
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
/// before `first_due_at`. It only records facts, so it is not blocked by the pause (repayments
/// are not either).
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
        .and_then(|due| due.checked_add(i64::from(pool.grace_period_seconds())))
        .ok_or(PoolError::Overflow)?;
    require!(now > grace_end, PoolError::NotEnoughMissed);

    let remaining = line
        .drawn
        .checked_sub(line.principal_repaid)
        .and_then(|v| v.checked_add(line.interest_due.checked_sub(line.interest_repaid)?))
        .ok_or(PoolError::Overflow)?;
    let line = &mut ctx.accounts.line;
    line.status = LineStatus::InCollection;
    emit_cpi!(LineInCollection { pool: pool.key(), line_id: line.line_id, missed: a.missed, remaining });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// write_off
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct WriteOff<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
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

/// Recognises the unpaid principal of an overdue line as a loss. Admin only; not blocked by the
/// pause. Allowed when
///
/// - the line is `Active` and `now > term_end + grace` (else `TermNotEnded`), or
/// - the line is `InCollection` and either a missed installment is still unpaid and
///   `now > first_missed_due + grace`, or `now > term_end + grace` (else `WriteOffTooEarly`).
///
/// `grace` is the pool's grace period; with zero grace a single-maturity line behaves as before.
pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    let grace = i64::from(pool.grace_period_seconds());
    let after_term = now > line.term_end.checked_add(grace).ok_or(PoolError::Overflow)?;
    match line.status {
        LineStatus::Active => require!(after_term, PoolError::TermNotEnded),
        LineStatus::InCollection => {
            let after_missed = match line_arrears(line, pool, now)?.first_missed_due {
                Some(due) => now > due.checked_add(grace).ok_or(PoolError::Overflow)?,
                None => false, // cured: nothing is missed
            };
            require!(after_missed || after_term, PoolError::WriteOffTooEarly);
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

// -----------------------------------------------------------------------------------------------
// suspend_draws
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct SuspendDraws<'info> {
    pub admin: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
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

/// D10: the admin closes the draw window of an `Approved` or `Active` line now, so no further
/// tranche is drawn (a KYC or KYB failure, a default event). The undrawn remainder is forfeited;
/// repayment, collection and write-off are untouched. It is not a pause: the pool-wide
/// `set_paused` stays as it is.
pub fn suspend_draws(ctx: Context<SuspendDraws>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let line = &mut ctx.accounts.line;
    require!(matches!(line.status, LineStatus::Approved | LineStatus::Active), PoolError::LineStatus);
    let available_forfeited = line.available().ok_or(PoolError::Overflow)?;
    line.availability_end = now;
    emit_cpi!(LineDrawsSuspended { pool: ctx.accounts.pool.key(), line_id: line.line_id, available_forfeited });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// cancel_line
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct CancelLine<'info> {
    pub admin: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
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

/// Decision 25: an approved commitment nobody drew (`Approved`, `drawn == 0`) is cleared. Once a
/// tranche exists something is owed, and the line follows repayment, collection or write-off
/// instead. `Cancelled` is terminal; the line id stays taken.
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
