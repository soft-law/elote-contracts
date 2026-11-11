//! Pool creation, deposits and redemptions at net asset value, and the pause switch.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::spl_token_2022::extension::default_account_state::DefaultAccountState;
use anchor_spl::token_interface::spl_token_2022::extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions};
use anchor_spl::token_interface::spl_token_2022::state::{AccountState, Mint as MintState};
use anchor_spl::token_interface::{self as token, Burn, Mint, MintTo, TokenAccount, TokenInterface, TransferChecked};

use crate::errors::PoolError;
use crate::events::{
    AccelerationNoticeSet, CollectionPeriodSet, CotejoKeysSet, CouncilAccepted, CouncilProposed, Deposited, FixBoundsSet, FixPosted, FxFeederSet,
    MaxLineBpsSet, NavUpdated, ProvisionSet, OutstandingCapSet, PoolInitialized, PoolPaused, PoolResumed, Redeemed,
};
use crate::council;
use crate::fx;
use crate::math;
use crate::state::{self, 
    min_first_deposit, seeds, Pool, ASSET_CONTROL_FREEZE, ASSET_CONTROL_PAUSABLE, BPS, DEAD_SHARES,
    COUNCIL_SQUADS, MIN_ACCELERATION_NOTICE_SECONDS,
    MIN_COLLECTION_PERIOD_SECONDS,
};

/// Signs CPIs as the pool PDA.
#[macro_export]
macro_rules! pool_signer {
    ($pool:expr) => {
        &[&[
            $crate::state::seeds::POOL,
            $pool.creator.as_ref(),
            $pool.asset_mint.as_ref(),
            &[$pool.bump],
        ]]
    };
}

#[event_cpi]
#[derive(Accounts)]
pub struct InitPool<'info> {
    /// Pays for the pool and seeds its address; no other power.
    #[account(mut)]
    pub creator: Signer<'info>,

    /// Classic SPL Token or Token-2022; `token_program` must be the mint's own program.
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        payer = creator,
        space = 8 + Pool::INIT_SPACE,
        seeds = [seeds::POOL, creator.key().as_ref(), asset_mint.key().as_ref()],
        bump,
    )]
    pub pool: Account<'info, Pool>,

    /// The share mint, created by the creator beforehand and validated here: mint authority = this
    /// pool's PDA, supply 0, the asset's decimals, and no extension that takes or hides value.
    /// `DefaultAccountState = Frozen` makes the shares permissioned.
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    /// A PDA token account, so nobody can pre-create it and block the pool.
    #[account(
        init,
        payer = creator,
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
        payer = creator,
        seeds = [seeds::DEAD, pool.key().as_ref()],
        bump,
        token::mint = share_mint,
        token::authority = pool,
        token::token_program = share_token_program,
    )]
    pub dead_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The series' Metaplex Core collection, created by the creator beforehand with the pool PDA as
    /// update authority.
    /// CHECK: owner, key and update authority are checked in `validate_collection`.
    pub collection: UncheckedAccount<'info>,

    /// The Squads v4 multisig whose vault 0 becomes the council; it does not sign here.
    /// CHECK: read at fixed offsets by `council::check`.
    pub multisig: UncheckedAccount<'info>,

    /// The token program of `asset_mint` (classic SPL Token or Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
    /// The token program of `share_mint` (Token-2022 in production, under Token ACL).
    pub share_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// The asset-mint policy. Refused (`AssetMintNotAllowed`):
/// - `TransferFeeConfig`: the vault would receive less than the depositor sends.
/// - `PermanentDelegate`: a third party could move the vault's tokens.
/// - `TransferHook`: foreign code would run on every transfer into and out of the vault.
/// - `NonTransferable`: no deposit could reach the vault.
/// - the confidential extensions: hidden balances make `vault.amount` unreliable as the NAV's
///   first term.
/// - `MintCloseAuthority`: the mint could be closed and re-created at its address with other
///   extensions.
/// - `DefaultAccountState` other than `Initialized`: the vault would be born frozen.
///
/// Accepted and recorded as `asset_controls` bits, the issuer controls that can stop the vault
/// without touching the program: a freeze authority and `Pausable`. Everything else is accepted
/// and not recorded. A classic mint has no extensions and may have a freeze authority.
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

/// The share-mint policy, so the pool can own and price the mint: owned by `share_token_program`,
/// mint authority = the pool PDA, supply 0, the asset's decimals. Refused (`ShareMintNotAllowed`):
/// - `TransferFeeConfig`: a transfer would take a cut of the holder's shares.
/// - `TransferHook`: foreign code would run on every share transfer.
/// - `MintCloseAuthority`: the mint could be closed and re-created at its address with other
///   extensions.
/// - `PermanentDelegate`: a third party could move or burn holders' shares.
/// - `InterestBearingConfig`, `ScaledUiAmount`: a display rate or multiplier on a NAV-priced share
///   misleads wallets.
/// - `Pausable`: a pause of every deposit and redemption outside `set_paused` and its event.
/// - the confidential extensions: a hidden supply breaks the price per share.
///
/// `DefaultAccountState = Frozen` makes the shares permissioned and then requires a freeze
/// authority, or nothing could ever be thawed; who holds it is the council's choice and is not
/// recorded. `NonTransferable` is allowed: deposit mints and redeem burns. Returns `permissioned`.
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
            | ExtensionType::ScaledUiAmount
            | ExtensionType::Pausable
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

/// The series' collection: a bare Metaplex Core `CollectionV1` whose update authority is this
/// pool's PDA, so only the program can add assets to it (`CollectionNotAllowed` otherwise). Any
/// plugin is refused, because Core applies a collection's permanent plugins to every asset in it,
/// so the account must be exactly the base record (key, update authority, name, URI,
/// `num_minted`, `current_size`) with no plugin header after it, and nothing minted yet. Every
/// read is at a fixed offset or bounded by a length read before it; a full decoder would allocate
/// the name and URI from a length prefix that, on a foreign account, can be anything.
fn validate_collection(collection: &AccountInfo, pool: &Pubkey) -> Result<()> {
    require!(*collection.owner == mpl_core::ID, PoolError::CollectionNotAllowed);
    let data = collection.data.borrow();
    require!(data.len() >= 33, PoolError::CollectionNotAllowed);
    require!(data[0] == mpl_core::types::Key::CollectionV1 as u8, PoolError::CollectionNotAllowed);
    let update_authority = Pubkey::try_from(&data[1..33]).map_err(|_| PoolError::CollectionNotAllowed)?;
    require!(update_authority == *pool, PoolError::CollectionNotAllowed);
    let u32_at = |at: usize| -> Result<u32> {
        let end = at.checked_add(4).ok_or(PoolError::CollectionNotAllowed)?;
        require!(data.len() >= end, PoolError::CollectionNotAllowed);
        Ok(u32::from_le_bytes(data[at..end].try_into().map_err(|_| PoolError::CollectionNotAllowed)?))
    };
    let name_len = usize::try_from(u32_at(33)?).map_err(|_| PoolError::CollectionNotAllowed)?;
    let uri_at = 37usize.checked_add(name_len).ok_or(PoolError::CollectionNotAllowed)?;
    let uri_len = usize::try_from(u32_at(uri_at)?).map_err(|_| PoolError::CollectionNotAllowed)?;
    let counts_at = uri_at.checked_add(4).and_then(|v| v.checked_add(uri_len)).ok_or(PoolError::CollectionNotAllowed)?;
    let num_minted = u32_at(counts_at)?;
    let current_size = u32_at(counts_at + 4)?;
    require!(data.len() == counts_at + 8, PoolError::CollectionNotAllowed);
    require!(num_minted == 0 && current_size == 0, PoolError::CollectionNotAllowed);
    Ok(())
}

/// The `init_pool` parameters.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct InitPoolParams {
    /// `1..=BPS`; fixed for the pool's life.
    pub max_line_bps: u16,
    /// At least `MIN_COLLECTION_PERIOD_SECONDS`.
    pub collection_period_seconds: u32,
    /// At least 1.
    pub max_outstanding_principal: u64,
    /// At least `MIN_ACCELERATION_NOTICE_SECONDS`.
    pub acceleration_notice_seconds: u32,
    /// The lowest and highest FIX `post_fix` accepts (`fx::bounds_valid`); zero and zero for none.
    pub min_fix_rate: u64,
    pub max_fix_rate: u64,
}

/// The council is vault 0 of `multisig`, checked by `council::check`; it does not sign, since
/// a valid multisig's vault can sign by construction. The creator pays and seeds the address and
/// keeps no power.
pub fn init_pool(ctx: Context<InitPool>, p: InitPoolParams) -> Result<()> {
    require!(p.max_line_bps >= 1 && u64::from(p.max_line_bps) <= BPS, PoolError::WrongAmount);
    require!(p.max_outstanding_principal >= 1, PoolError::WrongAmount);
    require!(p.collection_period_seconds >= MIN_COLLECTION_PERIOD_SECONDS, PoolError::CollectionPeriodTooShort);
    require!(p.acceleration_notice_seconds >= MIN_ACCELERATION_NOTICE_SECONDS, PoolError::AccelerationNoticeTooShort);
    require!(fx::bounds_valid(p.min_fix_rate, p.max_fix_rate), PoolError::WrongAmount);
    let multisig = ctx.accounts.multisig.to_account_info();
    let (council, council_vault_bump) = council::canonical_vault(multisig.key);
    council::check(&multisig, &council, council_vault_bump)?;
    require!(state::asset_listed(&ctx.accounts.asset_mint.key()), PoolError::AssetNotListed);
    require_keys_neq!(ctx.accounts.asset_mint.key(), ctx.accounts.share_mint.key(), PoolError::AssetIsShareMint);
    // `fx::usdc_for_mxn` counts the asset in millionths (USDC, MEXe).
    require!(ctx.accounts.asset_mint.decimals == state::ASSET_DECIMALS, PoolError::AssetDecimalsInvalid);
    let controls = asset_controls(&ctx.accounts.asset_mint.to_account_info())?;
    let permissioned = validate_share_mint(&ctx)?;
    validate_collection(&ctx.accounts.collection.to_account_info(), &ctx.accounts.pool.key())?;
    let pool = &mut ctx.accounts.pool;
    pool.set_inner(Pool {
        council,
        asset_mint: ctx.accounts.asset_mint.key(),
        share_mint: ctx.accounts.share_mint.key(),
        vault: ctx.accounts.vault.key(),
        dead_shares: ctx.accounts.dead_shares.key(),
        collection: ctx.accounts.collection.key(),
        outstanding_principal: 0,
        interest_collected: 0,
        written_off: 0,
        line_count: 0,
        collection_period_seconds: p.collection_period_seconds,
        max_line_bps: p.max_line_bps,
        asset_controls: controls,
        permissioned,
        paused: false,
        bump: ctx.bumps.pool,
        creator: ctx.accounts.creator.key(),
        council_kind: COUNCIL_SQUADS,
        multisig: *multisig.key,
        council_vault_bump,
        pending_council: Pubkey::default(),
        pending_council_kind: 0,
        max_outstanding_principal: p.max_outstanding_principal,
        acceleration_notice_seconds: p.acceleration_notice_seconds,
        impaired_principal: 0,
        provision_bps: BPS as u16,
        fx_feeder: Pubkey::default(),
        fix_rate: 0,
        fix_date: 0,
        fix_posted_at: 0,
        fx_realized: 0,
        recoverable: 0,
        min_fix_rate: p.min_fix_rate,
        max_fix_rate: p.max_fix_rate,
        cotejo_keys: [Pubkey::default(); 3],
        reserved: [0; 48],
    });

    emit_cpi!(PoolInitialized {
        pool: pool.key(),
        council,
        creator: pool.creator,
        multisig: pool.multisig,
        asset_mint: pool.asset_mint,
        share_mint: pool.share_mint,
        collection: pool.collection,
        collection_period_seconds: p.collection_period_seconds,
        max_line_bps: p.max_line_bps,
        max_outstanding_principal: p.max_outstanding_principal,
        acceleration_notice_seconds: p.acceleration_notice_seconds,
        asset_controls: controls,
        permissioned,
        min_fix_rate: p.min_fix_rate,
        max_fix_rate: p.max_fix_rate,
    });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct SetCollectionPeriod<'info> {
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
}

/// Council only; refused below `MIN_COLLECTION_PERIOD_SECONDS`. Applies to every line from its
/// next `write_off` check, marked or not.
pub fn set_collection_period(ctx: Context<SetCollectionPeriod>, collection_period_seconds: u32, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(collection_period_seconds >= MIN_COLLECTION_PERIOD_SECONDS, PoolError::CollectionPeriodTooShort);
    let pool = &mut ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    pool.collection_period_seconds = collection_period_seconds;
    emit_cpi!(CollectionPeriodSet { pool: pool.key(), collection_period_seconds });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct Deposit<'info> {
    pub depositor: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
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
/// depositor receives the rest. `min_shares_out` is the depositor's slippage floor. Deposits are
/// priced at the NAV **with no provision** (`Pool::entry_nav`, decision 40b — Maple's dual
/// price): the entrant buys any impaired book and any claim written off and still owed at par
/// (decision 51), so a recovery hands the entrant nothing
/// and depositing just before one nets zero or less; redemptions price at the provisioned NAV.
pub fn deposit(ctx: Context<Deposit>, assets: u64, min_shares_out: u64) -> Result<()> {
    let pool = &ctx.accounts.pool;
    require!(!pool.paused, PoolError::Paused);
    require!(assets > 0, PoolError::WrongAmount);

    // Price the deposit before any token moves.
    let supply = ctx.accounts.share_mint.supply;
    let nav = pool.entry_nav(ctx.accounts.vault.amount).ok_or(PoolError::Overflow)?;
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

#[event_cpi]
#[derive(Accounts)]
pub struct Redeem<'info> {
    pub holder: Signer<'info>,

    #[account(
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
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
    let nav = pool.nav(ctx.accounts.vault.amount).ok_or(PoolError::Overflow)?;
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

/// Council only, like every administrative decision (decision 39): pauses the pool through a
/// Squads vote; no member, and no other key, pauses alone. A paused pool refuses deposits,
/// redemptions, requests, approvals and draws; repayments, the FIX, the pagaré instructions,
/// collection and write-offs go on. Only the council resumes.
pub fn pause(ctx: Context<CouncilPool>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.paused = true;
    emit_cpi!(PoolPaused { pool: pool.key(), by: pool.council });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct CouncilPool<'info> {
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
}

impl CouncilPool<'_> {
    fn check_council(&self) -> Result<()> {
        council::check(&self.multisig.to_account_info(), &self.pool.council, self.pool.council_vault_bump)
    }
}

/// Council only: ends a pause.
pub fn resume(ctx: Context<CouncilPool>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.paused = false;
    emit_cpi!(PoolResumed { pool: pool.key() });
    Ok(())
}

/// Council only: the provision the NAV takes on the lines in collection, `0..=BPS` of their
/// outstanding cost (decision 40); `init_pool` starts it at `BPS`, a full provision.
pub fn set_provision_bps(ctx: Context<CouncilPool>, provision_bps: u16, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(u64::from(provision_bps) <= BPS, PoolError::WrongAmount);
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.provision_bps = provision_bps;
    emit_cpi!(ProvisionSet { pool: pool.key(), provision_bps });
    Ok(())
}

/// Council only: the line cap, `1..=BPS` of the NAV, measured at every approval and every draw;
/// lines already approved keep their principal and draw within the new cap.
pub fn set_max_line_bps(ctx: Context<CouncilPool>, max_line_bps: u16, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(max_line_bps >= 1 && u64::from(max_line_bps) <= BPS, PoolError::WrongAmount);
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.max_line_bps = max_line_bps;
    emit_cpi!(MaxLineBpsSet { pool: pool.key(), max_line_bps });
    Ok(())
}

/// Council only: the most USDC cost the pool may have lent out at once, checked at every draw
/// (at least one base unit); lowering it below the outstanding only stops new draws.
pub fn set_outstanding_cap(ctx: Context<CouncilPool>, max_outstanding_principal: u64, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(max_outstanding_principal >= 1, PoolError::WrongAmount);
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.max_outstanding_principal = max_outstanding_principal;
    emit_cpi!(OutstandingCapSet { pool: pool.key(), max_outstanding_principal });
    Ok(())
}

/// Council only: the lowest and highest FIX `post_fix` accepts from the feeder and the council
/// (`fx::bounds_valid`, `WrongAmount` otherwise); zero and zero remove the bounds. The FIX in force
/// is not re-checked.
pub fn set_fix_bounds(ctx: Context<CouncilPool>, min_fix_rate: u64, max_fix_rate: u64, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(fx::bounds_valid(min_fix_rate, max_fix_rate), PoolError::WrongAmount);
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.min_fix_rate = min_fix_rate;
    pool.max_fix_rate = max_fix_rate;
    emit_cpi!(FixBoundsSet { pool: pool.key(), min_fix_rate, max_fix_rate });
    Ok(())
}

/// Council only: the notice between `propose_acceleration` and `accelerate_line`, at least
/// `MIN_ACCELERATION_NOTICE_SECONDS`; a pending proposal keeps its own deadline.
pub fn set_acceleration_notice(ctx: Context<CouncilPool>, acceleration_notice_seconds: u32, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(acceleration_notice_seconds >= MIN_ACCELERATION_NOTICE_SECONDS, PoolError::AccelerationNoticeTooShort);
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.acceleration_notice_seconds = acceleration_notice_seconds;
    emit_cpi!(AccelerationNoticeSet { pool: pool.key(), acceleration_notice_seconds });
    Ok(())
}

/// Council only, also while paused: replaces the cotejo keys as a whole (decision 56). The zero
/// key is an empty slot, and all zero lets draws go without a co-signer. `CotejoKeyInvalid` when a
/// non-zero key repeats or is the pool, the council or this program.
pub fn set_cotejo_keys(ctx: Context<CouncilPool>, keys: [Pubkey; 3], valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let (pool_key, council_key) = (ctx.accounts.pool.key(), ctx.accounts.pool.council);
    for (i, key) in keys.iter().enumerate() {
        if *key == Pubkey::default() {
            continue;
        }
        require!(
            !keys[..i].contains(key) && *key != pool_key && *key != council_key && *key != crate::ID,
            PoolError::CotejoKeyInvalid
        );
    }
    ctx.accounts.check_council()?;
    let pool = &mut ctx.accounts.pool;
    pool.cotejo_keys = keys;
    emit_cpi!(CotejoKeysSet { pool: pool_key, keys });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct ProposeCouncil<'info> {
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
}

/// Council only. Names the authority that becomes the council once it signs `accept_council`;
/// `kind` must be `COUNCIL_SQUADS` (`CouncilKindInvalid` otherwise). The zero key withdraws a
/// pending proposal. A new proposal replaces the pending one.
pub fn propose_council(ctx: Context<ProposeCouncil>, new_council: Pubkey, kind: u8, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    require!(kind == COUNCIL_SQUADS, PoolError::CouncilKindInvalid);
    let pool = &mut ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    pool.pending_council = new_council;
    pool.pending_council_kind = kind;
    emit_cpi!(CouncilProposed { pool: pool.key(), council: new_council, kind });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct AcceptCouncil<'info> {
    /// The proposed authority: vault 0 of `multisig`, signing through Squads.
    pub new_council: Signer<'info>,

    /// CHECK: read at fixed offsets by `council::check`.
    pub multisig: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,
}

/// The proposed authority signs (`NoPendingCouncil` otherwise) and becomes the council. Only
/// `COUNCIL_SQUADS` is accepted (`CouncilKindInvalid`), and only after the same check as
/// `init_pool` on `multisig`, so the pool never rotates to an address that cannot sign or to a
/// multisig without a majority threshold.
pub fn accept_council(ctx: Context<AcceptCouncil>, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let new_council = ctx.accounts.new_council.key();
    let pool = &ctx.accounts.pool;
    require!(pool.pending_council != Pubkey::default() && pool.pending_council == new_council, PoolError::NoPendingCouncil);
    require!(pool.pending_council_kind == COUNCIL_SQUADS, PoolError::CouncilKindInvalid);
    let multisig = ctx.accounts.multisig.to_account_info();
    let (vault, vault_bump) = council::canonical_vault(multisig.key);
    require_keys_eq!(vault, new_council, PoolError::CouncilInvalid);
    council::check(&multisig, &vault, vault_bump)?;
    let pool = &mut ctx.accounts.pool;
    let old = pool.council;
    pool.council = new_council;
    pool.council_kind = COUNCIL_SQUADS;
    pool.multisig = *multisig.key;
    pool.council_vault_bump = vault_bump;
    pool.pending_council = Pubkey::default();
    pool.pending_council_kind = 0;
    emit_cpi!(CouncilAccepted { pool: pool.key(), old, new: new_council, kind: COUNCIL_SQUADS });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct SetFxFeeder<'info> {
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
}

/// Council only: names the key that publishes the FIX, or the zero key for none.
pub fn set_fx_feeder(ctx: Context<SetFxFeeder>, fx_feeder: Pubkey, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let pool = &mut ctx.accounts.pool;
    council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
    pool.fx_feeder = fx_feeder;
    emit_cpi!(FxFeederSet { pool: pool.key(), fx_feeder });
    Ok(())
}

#[event_cpi]
#[derive(Accounts)]
pub struct PostFix<'info> {
    /// The feeder or the council.
    pub poster: Signer<'info>,

    /// CHECK: `council::check` when the council posts.
    #[account(address = pool.multisig @ PoolError::CouncilInvalid)]
    pub multisig: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [seeds::POOL, pool.creator.as_ref(), pool.asset_mint.as_ref()],
        bump = pool.bump,
    )]
    pub pool: Account<'info, Pool>,

    /// RedStone's MXNe price account, the market reference the feeder's FIX is checked against.
    /// CHECK: the address here; the owner and the layout in `reference`.
    #[account(address = fx::REFERENCE_ACCOUNT @ PoolError::FxReferenceInvalid)]
    pub fx_reference: UncheckedAccount<'info>,
}

/// The market reference, `(rate, at)`: owned by `fx::REFERENCE_PROGRAM` and read by
/// `fx::reference_rate` (`FxReferenceInvalid` otherwise). On a localnet or devnet build, where the
/// account does not exist, an empty system account is `None`; a mainnet build always requires it.
fn reference(account: &AccountInfo) -> Result<Option<(u64, i64)>> {
    #[cfg(any(feature = "localnet", feature = "devnet"))]
    if account.data_is_empty() && *account.owner == anchor_lang::system_program::ID {
        return Ok(None);
    }
    require_keys_eq!(*account.owner, fx::REFERENCE_PROGRAM, PoolError::FxReferenceInvalid);
    let data = account.try_borrow_data()?;
    fx::reference_rate(&data).map(Some)
}

/// Publishes Banxico's FIX for `fix_date` (`yyyymmdd`, not after today in Mexico City), within the
/// pool's bounds (`FixOutOfBounds`). The feeder publishes a later date within the band of
/// `fx::within_band` from the FIX in force: 5% a day between the two dates, counting one to three
/// days (`FixJump`), once per Mexico City day: not on a day a FIX was already published, by the
/// feeder or the council (`FixAlreadyPostedToday`). A feeder that fell behind publishes today's
/// date alone, within the band of the days elapsed; anything else is the council's. The council
/// publishes the first FIX, larger moves and same-day corrections, dated at most
/// `fx::MAX_FIX_AGE_DAYS` before today. The date never moves back (`FixDateInvalid`). The feeder also needs the market reference (`fx_reference`) at most
/// `fx::MAX_REFERENCE_AGE_SECONDS` old (`FxReferenceStale`) and its FIX within
/// `fx::MAX_FIX_REFERENCE_GAP_BPS` of it (`FixOffReference`); the council publishes without that
/// check. `FixPosted` records the reference either way.
pub fn post_fix(ctx: Context<PostFix>, fix_rate: u64, fix_date: u32, valid_until: i64) -> Result<()> {
    council::fresh(valid_until)?;
    let now = Clock::get()?;
    let poster = ctx.accounts.poster.key();
    let reference = reference(&ctx.accounts.fx_reference.to_account_info());
    let pool = &mut ctx.accounts.pool;
    require!(fix_rate > 0, PoolError::WrongAmount);
    let day = fx::fix_day(fix_date)?;
    let today = fx::mexico_city_day(now.unix_timestamp);
    require!(day <= today, PoolError::FixDateInvalid);
    let (reference_rate, reference_at) = if poster == pool.council {
        council::check(&ctx.accounts.multisig.to_account_info(), &pool.council, pool.council_vault_bump)?;
        require!(fix_date >= pool.fix_date, PoolError::FixDateInvalid);
        require!(day >= today - fx::MAX_FIX_AGE_DAYS, PoolError::FixDateInvalid);
        reference.ok().flatten().unwrap_or((0, 0))
    } else {
        require!(pool.fx_feeder != Pubkey::default() && poster == pool.fx_feeder, PoolError::Unauthorized);
        require!(today > fx::mexico_city_day(pool.fix_posted_at), PoolError::FixAlreadyPostedToday);
        require!(fix_date > pool.fix_date, PoolError::FixDateInvalid);
        require!(pool.fix_rate != 0, PoolError::FixJump);
        let days = day - fx::fix_day(pool.fix_date)?;
        require!(fx::within_band(pool.fix_rate, fix_rate, days), PoolError::FixJump);
        match reference? {
            Some((rate, at)) => {
                require!(now.unix_timestamp.saturating_sub(at) <= fx::MAX_REFERENCE_AGE_SECONDS, PoolError::FxReferenceStale);
                require!(fx::within_bps(rate, fix_rate, fx::MAX_FIX_REFERENCE_GAP_BPS), PoolError::FixOffReference);
                (rate, at)
            }
            None => (0, 0),
        }
    };
    require!(fx::within_bounds(fix_rate, pool.min_fix_rate, pool.max_fix_rate), PoolError::FixOutOfBounds);
    let previous_rate = pool.fix_rate;
    pool.fix_rate = fix_rate;
    pool.fix_date = fix_date;
    pool.fix_posted_at = now.unix_timestamp;
    emit_cpi!(FixPosted {
        pool: pool.key(),
        fix_rate,
        fix_date,
        previous_rate,
        by: poster,
        slot: now.slot,
        posted_at: now.unix_timestamp,
        reference_rate,
        reference_at,
    });
    Ok(())
}
