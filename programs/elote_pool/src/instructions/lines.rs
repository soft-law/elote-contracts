//! Credit lines approved from the pool: open, draw, repay and write off.
//!
//! A line moves `Approved -> Active -> Repaid | WrittenOff`. Repayments go to principal first,
//! then to the fixed interest; collected interest raises the NAV, a written-off principal lowers
//! it once.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::PoolError;
use crate::events::{LineDrawn, LineOpened, LineRepaid, LineScheduled, LineWrittenOff};
use crate::math;
use crate::pool_signer;
use crate::state::{
    seeds, CreditLine, LineStatus, Pool, Schedule, ScheduleParams, BPS, LINE_VERSION, LINE_VERSION_SCHEDULED,
    MAX_INSTALLMENTS,
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
    schedule: Option<ScheduleParams>,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &mut ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    require!(line_id == pool.line_count, PoolError::WrongAccount);
    require!(principal > 0, PoolError::WrongAmount);
    require!(u64::from(rate_bps) <= BPS, PoolError::WrongAmount);
    require!(term_end > now, PoolError::WrongAmount);
    require!(agreement_hash != [0u8; 32], PoolError::WrongAmount);
    if let Some(named) = sponsor {
        let signed = ctx.accounts.sponsor.as_ref().is_some_and(|s| s.key() == named);
        require!(signed, PoolError::SponsorRequired);
    }
    let interest_due = math::interest_due(principal, rate_bps).ok_or(PoolError::Overflow)?;
    if let Some(s) = schedule {
        validate_schedule(&s, principal, interest_due, now, term_end)?;
    }

    let line = &mut ctx.accounts.line;
    line.version = if schedule.is_some() { LINE_VERSION_SCHEDULED } else { LINE_VERSION };
    line.pool = pool.key();
    line.line_id = line_id;
    line.borrower = ctx.accounts.borrower.key();
    line.sponsor = sponsor;
    line.agreement_hash = agreement_hash;
    line.principal = principal;
    line.rate_bps = rate_bps;
    line.interest_due = interest_due;
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
        });
    }
    line.bump = ctx.bumps.line;

    pool.line_count = pool.line_count.checked_add(1).ok_or(PoolError::Overflow)?;

    emit_cpi!(LineOpened {
        pool: pool.key(),
        line_id,
        borrower: line.borrower,
        sponsor,
        agreement_hash,
        principal,
        rate_bps,
        interest_due,
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
    Ok(())
}

/// Checks the schedule against the line's terms: at least one and at most `MAX_INSTALLMENTS`
/// installments of at least one unit each, a positive interval, `late_bps <= BPS`,
/// `1 <= max_missed <= installments`, and the last due date no later than `term_end` if the line
/// were drawn now (`draw` checks it again at the actual draw).
fn validate_schedule(s: &ScheduleParams, principal: u64, interest_due: u64, now: i64, term_end: i64) -> Result<()> {
    require!(s.installments >= 1 && s.installments <= MAX_INSTALLMENTS, PoolError::InvalidSchedule);
    require!(s.interval_seconds > 0, PoolError::InvalidSchedule);
    require!(u64::from(s.late_bps) <= BPS, PoolError::InvalidSchedule);
    require!(s.max_missed >= 1 && u16::from(s.max_missed) <= s.installments, PoolError::InvalidSchedule);
    let total = principal.checked_add(interest_due).ok_or(PoolError::Overflow)?;
    require!(total >= u64::from(s.installments), PoolError::InvalidSchedule);
    let last_due = schedule_end(now, s.interval_seconds, s.installments)?;
    require!(last_due <= term_end, PoolError::ScheduleEndsAfterTerm);
    Ok(())
}

/// The last due date of a schedule drawn at `start`: `start + installments * interval`.
fn schedule_end(start: i64, interval_seconds: i64, installments: u16) -> Result<i64> {
    let span = interval_seconds.checked_mul(i64::from(installments)).ok_or(PoolError::Overflow)?;
    Ok(start.checked_add(span).ok_or(PoolError::Overflow)?)
}

// -----------------------------------------------------------------------------------------------
// draw
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct Draw<'info> {
    pub borrower: Signer<'info>,

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

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = borrower_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = borrower_asset_account.owner == borrower.key() @ PoolError::WrongAccount,
    )]
    pub borrower_asset_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn draw(ctx: Context<Draw>) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    require!(!pool.paused, PoolError::Paused);
    require!(line.status == LineStatus::Approved, PoolError::LineStatus);
    let now = Clock::get()?.unix_timestamp;
    require!(now <= line.term_end, PoolError::TermEnded);
    // Due dates count from the draw: the last one must still fall within the term.
    if let Some(s) = line.schedule() {
        let last_due = schedule_end(now, s.interval_seconds, s.installments)?;
        require!(last_due <= line.term_end, PoolError::ScheduleEndsAfterTerm);
    }
    let amount = line.principal;
    require!(amount <= ctx.accounts.vault.amount, PoolError::InsufficientLiquidity);

    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.borrower_asset_account.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            pool_signer!(pool),
        ),
        amount,
    )?;

    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_add(amount).ok_or(PoolError::Overflow)?;
    let line = &mut ctx.accounts.line;
    line.status = LineStatus::Active;
    line.drawn_at = now;

    emit_cpi!(LineDrawn {
        pool: pool.key(),
        line_id: line.line_id,
        amount,
        outstanding_after: pool.outstanding_principal,
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

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = payer_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = payer_asset_account.owner == payer.key() @ PoolError::WrongAccount,
    )]
    pub payer_asset_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let line = &ctx.accounts.line;
    require!(!pool.paused, PoolError::Paused);
    require!(line.status == LineStatus::Active, PoolError::LineStatus);

    let principal_left = line.principal.checked_sub(line.principal_repaid).ok_or(PoolError::Overflow)?;
    let interest_left = line.interest_due.checked_sub(line.interest_repaid).ok_or(PoolError::Overflow)?;
    let remaining = principal_left.checked_add(interest_left).ok_or(PoolError::Overflow)?;
    require!(amount > 0 && amount <= remaining, PoolError::WrongAmount);

    // Principal first, then interest.
    let principal_part = amount.min(principal_left);
    let interest_part = amount.checked_sub(principal_part).ok_or(PoolError::Overflow)?;

    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            Transfer {
                from: ctx.accounts.payer_asset_account.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            },
        ),
        amount,
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
    let remaining_after = remaining.checked_sub(amount).ok_or(PoolError::Overflow)?;
    if remaining_after == 0 {
        line.status = LineStatus::Repaid;
        line.closed_at = Clock::get()?.unix_timestamp;
    }

    ctx.accounts.vault.reload()?;
    let nav_after = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    emit_cpi!(LineRepaid {
        pool: pool.key(),
        line_id: line.line_id,
        payer: ctx.accounts.payer.key(),
        principal_part,
        interest_part,
        remaining: remaining_after,
        nav_after,
    });
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

    /// Read for the NAV in the event.
    #[account(address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<Account<'info, TokenAccount>>,
}

/// Recognises the unpaid principal of an overdue line as a loss. Not blocked by the pause.
pub fn write_off(ctx: Context<WriteOff>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let line = &mut ctx.accounts.line;
    require!(line.status == LineStatus::Active, PoolError::LineStatus);
    require!(now > line.term_end, PoolError::TermNotEnded);

    let loss = line.principal.checked_sub(line.principal_repaid).ok_or(PoolError::Overflow)?;
    let pool = &mut ctx.accounts.pool;
    pool.outstanding_principal = pool.outstanding_principal.checked_sub(loss).ok_or(PoolError::Overflow)?;
    pool.written_off = pool.written_off.checked_add(loss).ok_or(PoolError::Overflow)?;
    line.status = LineStatus::WrittenOff;
    line.closed_at = now;

    let nav_after = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    emit_cpi!(LineWrittenOff {
        pool: pool.key(),
        line_id: line.line_id,
        loss,
        nav_after,
    });
    Ok(())
}
