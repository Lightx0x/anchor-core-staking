use anchor_lang::prelude::*;
use anchor_spl::{associated_token::AssociatedToken, token_interface::{Mint, TokenAccount, TokenInterface}};
use mpl_core::{
    ID as MPL_CORE_ID,
    accounts::{BaseAssetV1, BaseCollectionV1},
    types::{UpdateAuthority, Attribute, Attributes, Plugin},
    instructions::UpdatePluginV1CpiBuilder,
};
use crate::Config;
use crate::constants::*;
use crate::error::ErrorCode;
use crate::utils::{read_stake_info, days_between, calculate_rewards, mint_rewards};

#[derive(Accounts)]
pub struct ClaimRewards<'info> {
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
pub fn handler(ctx: Context<ClaimRewards>) -> Result<()> {

    // Read the Staking attributes (fails if the asset is not staked)
    let stake_info = read_stake_info(&ctx.accounts.asset.to_account_info())?;
    let current_timestamp = Clock::get()?.unix_timestamp;

    // Only whole days are rewarded
    let unclaimed_days = days_between(stake_info.last_claimed_at, current_timestamp)?;
    let amount = calculate_rewards(unclaimed_days, ctx.accounts.config.rewards_bps, ctx.accounts.rewards_mint.decimals)?;

    // Move "last_claimed_at" forward by the rewarded days only, so the partial day keeps accruing.
    // "staked_at" is left untouched: claiming does not restart the freeze period, and the asset stays staked and frozen.
    let last_claimed_at = stake_info.last_claimed_at
        .checked_add(unclaimed_days.checked_mul(SECONDS_PER_DAY).ok_or(ErrorCode::InvalidTimestamp)?)
        .ok_or(ErrorCode::InvalidTimestamp)?;

    let mut attributes_list: Vec<Attribute> = stake_info.other_attributes;
    attributes_list.push(Attribute {
        key: STAKED_KEY.to_string(),
        value: "true".to_string(),
    });
    attributes_list.push(Attribute {
        key: STAKED_AT_KEY.to_string(),
        value: stake_info.staked_at.to_string(),
    });
    attributes_list.push(Attribute {
        key: LAST_CLAIMED_AT_KEY.to_string(),
        value: last_claimed_at.to_string(),
    });

    // Prepare signing seeds for the update authority
    let collection_key = ctx.accounts.collection.key();
    let signer_seeds = &[
        b"update_authority",
        collection_key.as_ref(),
        &[ctx.bumps.update_authority],
    ];

    UpdatePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(&ctx.accounts.update_authority.to_account_info()))
    .system_program(&ctx.accounts.system_program.to_account_info())
    .plugin(Plugin::Attributes(Attributes { attribute_list: attributes_list }))
    .invoke_signed(&[signer_seeds])?;

    // Mint the rewards to the user
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
