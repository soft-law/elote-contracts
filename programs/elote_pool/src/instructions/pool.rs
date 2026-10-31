//! Pool creation, deposits and redemptions at net asset value, and the pause switch.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::spl_token_2022::extension::default_account_state::DefaultAccountState;
use anchor_spl::token_interface::spl_token_2022::extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions};
use anchor_spl::token_interface::spl_token_2022::state::{AccountState, Mint as MintState};
use anchor_spl::token_interface::{self as token, Burn, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{Deposited, PoolConfigured, PoolFlags, PoolInitialized, PoolLineCapSet, PoolPaused, Redeemed};
use crate::math;
use crate::state::{
    min_first_deposit, seeds, Pool, ASSET_CONTROL_FREEZE, ASSET_CONTROL_PAUSABLE, BPS, DEAD_SHARES, POOL_VERSION,
};

/// Signs CPIs as the pool PDA.
#[macro_export]
macro_rules! pool_signer {
    ($pool:expr) => {
        &[&[
            $crate::state::seeds::POOL,
            $pool.admin.as_ref(),
            $pool.asset_mint.as_ref(),
            &[$pool.bump],
        ]]
    };
}

// -----------------------------------------------------------------------------------------------
// init_pool
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct InitPool<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Classic SPL Token or Token-2022; `token_program` must be the mint's own program.
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        payer = admin,
        space = 8 + Pool::INIT_SPACE,
        seeds = [seeds::POOL, admin.key().as_ref(), asset_mint.key().as_ref()],
        bump,
    )]
    pub pool: Account<'info, Pool>,

    #[account(
        init,
        payer = admin,
        seeds = [seeds::SHARES, pool.key().as_ref()],
        bump,
        mint::decimals = asset_mint.decimals,
        mint::authority = pool,
        mint::token_program = token_program,
    )]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    /// A PDA token account, so nobody can pre-create it and block the pool.
    #[account(
        init,
        payer = admin,
        seeds = [seeds::VAULT, pool.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = pool,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Holds the dead shares of the first deposit; nothing ever moves them.
    #[account(
        init,
        payer = admin,
        seeds = [seeds::DEAD, pool.key().as_ref()],
        bump,
        token::mint = share_mint,
        token::authority = pool,
        token::token_program = token_program,
    )]
    pub dead_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The token program of `asset_mint` (classic SPL Token or Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// The asset-mint policy (series-shelf 2b, decision 15). Refused, because they take or hide
/// value or would freeze the vault at creation: `TransferFeeConfig`, `PermanentDelegate`,
/// `TransferHook`, `NonTransferable`, every confidential extension (`ConfidentialTransferMint`,
/// `ConfidentialTransferFeeConfig`, `ConfidentialMintBurn`: a mint that can hold confidential
/// balances makes `vault.amount` unreliable as the NAV's first term), `MintCloseAuthority`, and
/// `DefaultAccountState` unless the state is `Initialized`. Accepted and recorded as
/// `asset_controls` bits: a freeze authority and `Pausable`, the issuer controls that can stop
/// the vault without touching the program. Everything else (metadata, interest-bearing display
/// rate, scaled UI amount, group membership) is accepted and not recorded. Works on a classic
/// mint too: it has no extensions and may have a freeze authority.
fn asset_controls(asset_mint: &AccountInfo) -> Result<u8> {
    let data = asset_mint.data.borrow();
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    let mut controls = 0u8;
    if state.base.freeze_authority.is_some() {
        controls |= ASSET_CONTROL_FREEZE;
    }
    for extension in state.get_extension_types()? {
        match extension {
            ExtensionType::TransferFeeConfig
            | ExtensionType::PermanentDelegate
            | ExtensionType::TransferHook
            | ExtensionType::NonTransferable
            | ExtensionType::ConfidentialTransferMint
            | ExtensionType::ConfidentialTransferFeeConfig
            | ExtensionType::ConfidentialMintBurn
            | ExtensionType::MintCloseAuthority => return err!(PoolError::AssetMintNotAllowed),
            ExtensionType::DefaultAccountState => {
                let default_state = state.get_extension::<DefaultAccountState>()?;
                let initialized = AccountState::try_from(default_state.state)
                    .map(|s| s == AccountState::Initialized)
                    .unwrap_or(false);
                require!(initialized, PoolError::AssetMintNotAllowed);
            }
            ExtensionType::Pausable => controls |= ASSET_CONTROL_PAUSABLE,
            _ => {}
        }
    }
    Ok(controls)
}

/// `grace_period_seconds` (zero means no grace) and `max_line_bps` are fixed for the life of
/// the pool.
pub fn init_pool(ctx: Context<InitPool>, grace_period_seconds: u32, max_line_bps: u16) -> Result<()> {
    require!(max_line_bps >= 1 && u64::from(max_line_bps) <= BPS, PoolError::WrongAmount);
    // A mint whose `10^decimals` does not fit a u64 could never meet the first-deposit minimum.
    require!(min_first_deposit(ctx.accounts.asset_mint.decimals).is_some(), PoolError::WrongAccount);
    let controls = asset_controls(&ctx.accounts.asset_mint.to_account_info())?;
    let pool = &mut ctx.accounts.pool;
    pool.version = POOL_VERSION;
    pool.admin = ctx.accounts.admin.key();
    pool.asset_mint = ctx.accounts.asset_mint.key();
    pool.vault = ctx.accounts.vault.key();
    pool.share_mint = ctx.accounts.share_mint.key();
    pool.dead_shares = ctx.accounts.dead_shares.key();
    pool.outstanding_principal = 0;
    pool.interest_collected = 0;
    pool.written_off = 0;
    pool.line_count = 0;
    pool.paused = false;
    pool.reserved = [0; 32];
    pool.set_grace_period_seconds(grace_period_seconds);
    pool.set_max_line_bps(max_line_bps);
    pool.set_asset_controls(controls);
    // The share mint is created here without `DefaultAccountState`, so holders need no thaw.
    pool.set_permissioned(false);
    pool.bump = ctx.bumps.pool;

    emit_cpi!(PoolInitialized {
        pool: pool.key(),
        asset_mint: pool.asset_mint,
        share_mint: pool.share_mint,
        admin: pool.admin,
    });
    emit_cpi!(PoolConfigured { pool: pool.key(), grace_period_seconds });
    emit_cpi!(PoolLineCapSet { pool: pool.key(), max_line_bps });
    emit_cpi!(PoolFlags { pool: pool.key(), asset_controls: controls, permissioned: pool.permissioned() });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// deposit
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct Deposit<'info> {
    pub depositor: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    /// Read for its decimals (`transfer_checked`).
    #[account(address = pool.asset_mint @ PoolError::WrongAccount)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.share_mint @ PoolError::WrongAccount)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut, address = pool.dead_shares @ PoolError::WrongAccount)]
    pub dead_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = depositor_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = depositor_asset_account.owner == depositor.key() @ PoolError::WrongAccount,
    )]
    pub depositor_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = depositor_share_account.mint == pool.share_mint @ PoolError::WrongAccount,
        constraint = depositor_share_account.owner == depositor.key() @ PoolError::WrongAccount,
    )]
    pub depositor_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

/// The first deposit locks `DEAD_SHARES` of its shares in the pool-owned dead account; the
/// depositor receives the rest. `min_shares_out` is the depositor's slippage floor.
pub fn deposit(ctx: Context<Deposit>, assets: u64, min_shares_out: u64) -> Result<()> {
    let pool = &ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    require!(assets > 0, PoolError::WrongAmount);

    // Price the deposit before any token moves.
    let supply = ctx.accounts.share_mint.supply;
    let nav = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    if supply == 0 {
        // The share mint has the asset's decimals (fixed at `init_pool`), so it stands in for
        // the asset mint here.
        let min = min_first_deposit(ctx.accounts.share_mint.decimals).ok_or(PoolError::Overflow)?;
        require!(assets >= min, PoolError::FirstDepositTooSmall);
    }
    let minted = math::shares_for_deposit(assets, supply, nav).ok_or(PoolError::Overflow)?;
    let dead = if supply == 0 { DEAD_SHARES } else { 0 };
    let shares = minted.checked_sub(dead).ok_or(PoolError::Overflow)?;
    require!(shares > 0, PoolError::ZeroShares);
    require!(shares >= min_shares_out, PoolError::SlippageExceeded);

    let token_program = ctx.accounts.token_program.key();
    token::transfer_checked(
        CpiContext::new(
            token_program,
            TransferChecked {
                from: ctx.accounts.depositor_asset_account.to_account_info(),
                mint: ctx.accounts.asset_mint.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.depositor.to_account_info(),
            },
        ),
        assets,
        ctx.accounts.asset_mint.decimals,
    )?;
    token::mint_to(
        CpiContext::new_with_signer(
            token_program,
            MintTo {
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.depositor_share_account.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            pool_signer!(pool),
        ),
        shares,
    )?;
    if dead > 0 {
        token::mint_to(
            CpiContext::new_with_signer(
                token_program,
                MintTo {
                    mint: ctx.accounts.share_mint.to_account_info(),
                    to: ctx.accounts.dead_shares.to_account_info(),
                    authority: ctx.accounts.pool.to_account_info(),
                },
                pool_signer!(pool),
            ),
            dead,
        )?;
    }

    ctx.accounts.vault.reload()?;
    ctx.accounts.share_mint.reload()?;
    let nav_after = ctx.accounts.pool.check(ctx.accounts.vault.amount, Some(ctx.accounts.share_mint.supply))?;
    emit_cpi!(Deposited {
        pool: ctx.accounts.pool.key(),
        depositor: ctx.accounts.depositor.key(),
        assets,
        shares,
        nav_after,
        supply_after: ctx.accounts.share_mint.supply,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// redeem
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct Redeem<'info> {
    pub holder: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    /// Read for its decimals (`transfer_checked`).
    #[account(address = pool.asset_mint @ PoolError::WrongAccount)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.share_mint @ PoolError::WrongAccount)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, address = pool.vault @ PoolError::WrongAccount)]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = holder_share_account.mint == pool.share_mint @ PoolError::WrongAccount,
        constraint = holder_share_account.owner == holder.key() @ PoolError::WrongAccount,
    )]
    pub holder_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = holder_asset_account.mint == pool.asset_mint @ PoolError::WrongAccount,
        constraint = holder_asset_account.owner == holder.key() @ PoolError::WrongAccount,
    )]
    pub holder_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

/// `min_assets_out` is the holder's slippage floor.
pub fn redeem(ctx: Context<Redeem>, shares: u64, min_assets_out: u64) -> Result<()> {
    let pool = &ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    require!(shares > 0, PoolError::WrongAmount);

    // Price the redemption against the pre-burn supply.
    let supply = ctx.accounts.share_mint.supply;
    let nav = math::nav(ctx.accounts.vault.amount, pool.outstanding_principal).ok_or(PoolError::Overflow)?;
    let assets = math::assets_for_redeem(shares, supply, nav).ok_or(PoolError::Overflow)?;
    require!(assets > 0, PoolError::WrongAmount);
    require!(assets >= min_assets_out, PoolError::SlippageExceeded);
    require!(assets <= ctx.accounts.vault.amount, PoolError::InsufficientLiquidity);

    let token_program = ctx.accounts.token_program.key();
    token::burn(
        CpiContext::new(
            token_program,
            Burn {
                mint: ctx.accounts.share_mint.to_account_info(),
                from: ctx.accounts.holder_share_account.to_account_info(),
                authority: ctx.accounts.holder.to_account_info(),
            },
        ),
        shares,
    )?;
    token::transfer_checked(
        CpiContext::new_with_signer(
            token_program,
            TransferChecked {
                from: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.asset_mint.to_account_info(),
                to: ctx.accounts.holder_asset_account.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            pool_signer!(pool),
        ),
        assets,
        ctx.accounts.asset_mint.decimals,
    )?;

    ctx.accounts.vault.reload()?;
    ctx.accounts.share_mint.reload()?;
    let nav_after = ctx.accounts.pool.check(ctx.accounts.vault.amount, Some(ctx.accounts.share_mint.supply))?;
    emit_cpi!(Redeemed {
        pool: ctx.accounts.pool.key(),
        holder: ctx.accounts.holder.key(),
        shares,
        assets,
        nav_after,
        supply_after: ctx.accounts.share_mint.supply,
    });
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// set_paused
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct SetPaused<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,
}

pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    let pool = &mut ctx.accounts.pool;
    pool.paused = paused;
    emit_cpi!(PoolPaused { pool: pool.key(), paused });
    Ok(())
}
