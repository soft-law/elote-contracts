//! Pool creation, deposits and redemptions at net asset value, and the pause switch.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::spl_token_2022::extension::default_account_state::DefaultAccountState;
use anchor_spl::token_interface::spl_token_2022::extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions};
use anchor_spl::token_interface::spl_token_2022::state::{AccountState, Mint as MintState};
use anchor_spl::token_interface::{self as token, Burn, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{
    AttestorSet, Deposited, NavUpdated, PoolCollectionSet, PoolConfigured, PoolFlags, PoolInitialized, PoolLineCapSet,
    PoolPaused, Redeemed,
};
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

    /// The share mint, created by the admin beforehand and validated here (series-shelf 2c):
    /// mint authority = this pool's PDA, supply 0, the asset's decimals, and no extension that
    /// takes or hides value. `DefaultAccountState = Frozen` makes the shares permissioned.
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

    /// Holds the dead shares of the first deposit; nothing ever moves them. On a permissioned
    /// share mint it is born frozen and must be thawed by the mint's freeze authority before the
    /// first deposit (in this transaction or a later one).
    #[account(
        init,
        payer = admin,
        seeds = [seeds::DEAD, pool.key().as_ref()],
        bump,
        token::mint = share_mint,
        token::authority = pool,
        token::token_program = share_token_program,
    )]
    pub dead_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The series' Metaplex Core collection, recorded on the pool. Step 6 of the pool v2 batch
    /// adds the check that it is a Core collection the pool can mint into.
    /// CHECK: recorded by key only until the pagaré-token step validates it.
    pub collection: UncheckedAccount<'info>,

    /// The token program of `asset_mint` (classic SPL Token or Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
    /// The token program of `share_mint` (Token-2022 in production, under Token ACL).
    pub share_token_program: Interface<'info, TokenInterface>,
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

/// The share-mint policy (series-shelf 2c, decision 15). The admin creates the mint and the
/// pool must be able to own and price it: owned by `share_token_program`, mint authority = the
/// pool PDA, supply 0, the asset's decimals. Refused extensions: `TransferFeeConfig`,
/// `TransferHook`, `MintCloseAuthority`, `PermanentDelegate`, `InterestBearingConfig` (a display
/// rate on a NAV-priced share misleads wallets), the three confidential ones (a hidden supply
/// breaks the price per share). `DefaultAccountState = Frozen` makes the shares permissioned and
/// then requires a freeze authority, or nothing could ever be thawed. `NonTransferable` is
/// allowed: deposit mints and redeem burns. Who the freeze authority is (Token ACL's
/// `MintConfig` or a key) is the admin's choice and not recorded. Returns `permissioned`.
fn validate_share_mint(ctx: &Context<InitPool>) -> Result<bool> {
    let share_mint = &ctx.accounts.share_mint;
    require!(
        *share_mint.to_account_info().owner == ctx.accounts.share_token_program.key(),
        PoolError::ShareMintNotAllowed
    );
    require!(share_mint.mint_authority == Some(ctx.accounts.pool.key()).into(), PoolError::ShareMintNotAllowed);
    require!(share_mint.supply == 0, PoolError::ShareMintNotAllowed);
    require!(share_mint.decimals == ctx.accounts.asset_mint.decimals, PoolError::ShareMintNotAllowed);

    let info = share_mint.to_account_info();
    let data = info.data.borrow();
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    let mut permissioned = false;
    for extension in state.get_extension_types()? {
        match extension {
            ExtensionType::TransferFeeConfig
            | ExtensionType::TransferHook
            | ExtensionType::MintCloseAuthority
            | ExtensionType::PermanentDelegate
            | ExtensionType::InterestBearingConfig
            | ExtensionType::ConfidentialTransferMint
            | ExtensionType::ConfidentialTransferFeeConfig
            | ExtensionType::ConfidentialMintBurn => return err!(PoolError::ShareMintNotAllowed),
            ExtensionType::DefaultAccountState => {
                let default_state = state.get_extension::<DefaultAccountState>()?;
                let frozen = AccountState::try_from(default_state.state)
                    .map(|s| s == AccountState::Frozen)
                    .unwrap_or(false);
                if frozen {
                    require!(state.base.freeze_authority.is_some(), PoolError::ShareMintNotAllowed);
                    permissioned = true;
                }
            }
            _ => {}
        }
    }
    Ok(permissioned)
}

/// `grace_period_seconds` (zero means no grace) and `max_line_bps` are fixed for the life of
/// the pool.
pub fn init_pool(ctx: Context<InitPool>, grace_period_seconds: u32, max_line_bps: u16, attestor: Pubkey) -> Result<()> {
    require!(max_line_bps >= 1 && u64::from(max_line_bps) <= BPS, PoolError::WrongAmount);
    // The zero key could never co-sign: a pool with it would open no line.
    require!(attestor != Pubkey::default(), PoolError::WrongAccount);
    // A mint whose `10^decimals` does not fit a u64 could never meet the first-deposit minimum.
    require!(min_first_deposit(ctx.accounts.asset_mint.decimals).is_some(), PoolError::WrongAccount);
    let controls = asset_controls(&ctx.accounts.asset_mint.to_account_info())?;
    let permissioned = validate_share_mint(&ctx)?;
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
    pool.set_permissioned(permissioned);
    pool.bump = ctx.bumps.pool;
    pool.attestor = attestor;
    pool.collection = ctx.accounts.collection.key();

    emit_cpi!(PoolInitialized {
        pool: pool.key(),
        asset_mint: pool.asset_mint,
        share_mint: pool.share_mint,
        admin: pool.admin,
    });
    emit_cpi!(PoolConfigured { pool: pool.key(), grace_period_seconds });
    emit_cpi!(PoolLineCapSet { pool: pool.key(), max_line_bps });
    emit_cpi!(PoolFlags { pool: pool.key(), asset_controls: controls, permissioned: pool.permissioned() });
    emit_cpi!(AttestorSet { pool: pool.key(), attestor });
    emit_cpi!(PoolCollectionSet { pool: pool.key(), collection: pool.collection });
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

    /// The asset's token program.
    pub token_program: Interface<'info, TokenInterface>,
    /// The share mint's token program.
    pub share_token_program: Interface<'info, TokenInterface>,
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
    let share_token_program = ctx.accounts.share_token_program.key();
    token::mint_to(
        CpiContext::new_with_signer(
            share_token_program,
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
                share_token_program,
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
    emit_cpi!(NavUpdated {
        pool: ctx.accounts.pool.key(),
        total_assets: nav_after,
        share_supply: ctx.accounts.share_mint.supply,
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

    /// The asset's token program.
    pub token_program: Interface<'info, TokenInterface>,
    /// The share mint's token program.
    pub share_token_program: Interface<'info, TokenInterface>,
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
            ctx.accounts.share_token_program.key(),
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
    emit_cpi!(NavUpdated {
        pool: ctx.accounts.pool.key(),
        total_assets: nav_after,
        share_supply: ctx.accounts.share_mint.supply,
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

// -----------------------------------------------------------------------------------------------
// set_attestor
// -----------------------------------------------------------------------------------------------

#[event_cpi]
#[derive(Accounts)]
pub struct SetAttestor<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.admin.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
        has_one = admin @ PoolError::Unauthorized,
    )]
    pub pool: Account<'info, Pool>,
}

/// Admin only. The attestor can block every opening, draw and endoso by withholding its
/// signature and can move nothing; replacing it is the recovery when its key rotates or is lost.
pub fn set_attestor(ctx: Context<SetAttestor>, attestor: Pubkey) -> Result<()> {
    require!(attestor != Pubkey::default(), PoolError::WrongAccount);
    let pool = &mut ctx.accounts.pool;
    pool.attestor = attestor;
    emit_cpi!(AttestorSet { pool: pool.key(), attestor });
    Ok(())
}
