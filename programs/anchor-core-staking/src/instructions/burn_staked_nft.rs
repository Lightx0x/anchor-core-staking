use anchor_lang::prelude::*;
use anchor_spl::{associated_token::AssociatedToken, token_interface::{Mint, TokenAccount, TokenInterface}};
use mpl_core::{
    ID as MPL_CORE_ID,
    accounts::{BaseAssetV1, BaseCollectionV1},
    types::{UpdateAuthority, Plugin, FreezeDelegate},
    instructions::{UpdatePluginV1CpiBuilder, BurnV1CpiBuilder},
};
use crate::Config;
use crate::constants::*;
use crate::error::ErrorCode;
use crate::utils::{read_stake_info, days_between, calculate_rewards, mint_rewards, update_total_staked};

#[derive(Accounts)]
pub struct BurnStakedNft<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        seeds = [b"config", collection.key().as_ref()],
        bump = config.bump,
    )]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        has_one = owner @ ErrorCode::InvalidOwner,
        constraint = asset.update_authority == UpdateAuthority::Collection(collection.key()) @ ErrorCode::InvalidUpdateAuthority,
    )]
    pub asset: Account<'info, BaseAssetV1>,
    #[account(
        mut,
        has_one = update_authority @ ErrorCode::InvalidUpdateAuthority
    )]
    pub collection: Account<'info, BaseCollectionV1>,
    /// CHECK: This account data is not used, we only verify the address
    #[account(
        seeds = [b"update_authority", collection.key().as_ref()],
        bump,
    )]
    pub update_authority: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [b"rewards_mint", config.key().as_ref()],
        bump = config.rewards_bump,
    )]
    pub rewards_mint: InterfaceAccount<'info, Mint>,
    #[account(
        init_if_needed,
        payer = owner,
        associated_token::mint = rewards_mint,
        associated_token::authority = owner,
    )]
    pub user_rewards_ata: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    /// CHECK: This is the MPL Core program
    #[account(address = MPL_CORE_ID)]
    pub mpl_core_program: UncheckedAccount<'info>,
}
pub fn handler(ctx: Context<BurnStakedNft>) -> Result<()> {

    // Read the Staking attributes (fails if the asset is not staked)
    let stake_info = read_stake_info(&ctx.accounts.asset.to_account_info())?;
    let current_timestamp = Clock::get()?.unix_timestamp;

    // Unclaimed staking rewards plus the one-time burn bonus
    let unclaimed_days = days_between(stake_info.last_claimed_at, current_timestamp)?;
    let decimals = ctx.accounts.rewards_mint.decimals;
    let amount = calculate_rewards(unclaimed_days, ctx.accounts.config.rewards_bps, decimals)?
        .checked_add(
            BURN_BONUS_TOKENS
                .checked_mul(10u64.pow(decimals as u32))
                .ok_or(ErrorCode::InvalidRewardsBps)?,
        )
        .ok_or(ErrorCode::InvalidRewardsBps)?;

    // Prepare signing seeds for the update authority
    let collection_key = ctx.accounts.collection.key();
    let signer_seeds = &[
        b"update_authority",
        collection_key.as_ref(),
        &[ctx.bumps.update_authority],
    ];

    // A frozen asset can't be burned, so we Thaw it first (the FreezeDelegate authority is the update authority PDA)
    UpdatePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(&ctx.accounts.update_authority.to_account_info()))
    .system_program(&ctx.accounts.system_program.to_account_info())
    .plugin(Plugin::FreezeDelegate(FreezeDelegate { frozen: false }))
    .invoke_signed(&[signer_seeds])?;

    // Burn the asset, signed by the update authority PDA acting as the BurnDelegate (added on stake)
    BurnV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(&ctx.accounts.update_authority.to_account_info()))
    .system_program(Some(&ctx.accounts.system_program.to_account_info()))
    .invoke_signed(&[signer_seeds])?;

    // Decrement the "total_staked" counter on the Collection
    update_total_staked(
        &ctx.accounts.mpl_core_program.to_account_info(),
        &ctx.accounts.collection.to_account_info(),
        &ctx.accounts.owner.to_account_info(),
        &ctx.accounts.update_authority.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        ctx.bumps.update_authority,
        false,
    )?;

    // Mint the rewards and burn bonus to the user
    mint_rewards(
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.rewards_mint.to_account_info(),
        ctx.accounts.user_rewards_ata.to_account_info(),
        ctx.accounts.config.to_account_info(),
        &collection_key,
        ctx.accounts.config.bump,
        amount,
        ctx.accounts.rewards_mint.decimals,
    )?;

    Ok(())
}
