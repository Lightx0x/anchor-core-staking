use anchor_lang::prelude::*;
use anchor_spl::{associated_token::AssociatedToken, token_interface::{Mint, TokenAccount, TokenInterface}};
use mpl_core::{
    ID as MPL_CORE_ID,
    accounts::{BaseAssetV1, BaseCollectionV1},
    types::{UpdateAuthority, Attribute, Attributes, Plugin, PluginType, FreezeDelegate},
    instructions::{UpdatePluginV1CpiBuilder, RemovePluginV1CpiBuilder},
};
use crate::Config;
use crate::constants::*;
use crate::error::ErrorCode;
use crate::utils::{read_stake_info, days_between, calculate_rewards, mint_rewards, update_total_staked};

#[derive(Accounts)]
pub struct Unstake<'info> {
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
    #[account(address = Pubkey::from(MPL_CORE_ID.to_bytes()))]
    pub mpl_core_program: UncheckedAccount<'info>,
}
pub fn handler(ctx: Context<Unstake>) -> Result<()> {

    // Read the Staking attributes (fails if the asset is not staked)
    let stake_info = read_stake_info(&ctx.accounts.asset.to_account_info())?;
    let current_timestamp = Clock::get()?.unix_timestamp;

    // The freeze period is counted from the moment the asset was staked
    let staked_days = days_between(stake_info.staked_at, current_timestamp)?;
    require!(staked_days >= ctx.accounts.config.freeze_period as i64, ErrorCode::FreezePeriodNotElapsed);

    // Rewards are counted from the last claim (or the stake, if never claimed)
    let unclaimed_days = days_between(stake_info.last_claimed_at, current_timestamp)?;

    // Prepare signing seeds for the update authority
    let collection_key = ctx.accounts.collection.key();
    let signer_seeds = &[
        b"update_authority",
        collection_key.as_ref(),
        &[ctx.bumps.update_authority],
    ];

    // Now we update the asset Atributes Plugin (with the existing attributes, including the Staking attributes with reset values)
    let mut attributes_list: Vec<Attribute> = stake_info.other_attributes;
    attributes_list.push(Attribute {
        key: STAKED_KEY.to_string(),
        value: "false".to_string(),
    });
    attributes_list.push(Attribute {
        key: STAKED_AT_KEY.to_string(),
        value: "0".to_string(),
    });
    attributes_list.push(Attribute {
        key: LAST_CLAIMED_AT_KEY.to_string(),
        value: "0".to_string(),
    });

    UpdatePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(&ctx.accounts.update_authority.to_account_info()))
    .system_program(&ctx.accounts.system_program.to_account_info())
    .plugin(Plugin::Attributes(Attributes { attribute_list: attributes_list }))
    .invoke_signed(&[signer_seeds])?;

    // And we Thaw the asset (update the FreezeDelegate Plugin to false)
    UpdatePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
    .asset(&ctx.accounts.asset.to_account_info())
    .collection(Some(&ctx.accounts.collection.to_account_info()))
    .payer(&ctx.accounts.owner.to_account_info())
    .authority(Some(&ctx.accounts.update_authority.to_account_info()))
    .system_program(&ctx.accounts.system_program.to_account_info())
    .plugin(Plugin::FreezeDelegate(FreezeDelegate { frozen: false }))
    .invoke_signed(&[signer_seeds])?;

    // Remove the FreezeDelegate and BurnDelegate Plugins so the owner gets full control back
    // (and the asset can be staked again, which adds them anew)
    // Both are Owner-Managed Plugins, so they are removed by the owner
    for plugin_type in [PluginType::FreezeDelegate, PluginType::BurnDelegate] {
        RemovePluginV1CpiBuilder::new(&ctx.accounts.mpl_core_program.to_account_info())
        .asset(&ctx.accounts.asset.to_account_info())
        .collection(Some(&ctx.accounts.collection.to_account_info()))
        .payer(&ctx.accounts.owner.to_account_info())
        .authority(Some(&ctx.accounts.owner.to_account_info()))
        .system_program(&ctx.accounts.system_program.to_account_info())
        .plugin_type(plugin_type)
        .invoke()?;
    }

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

    // Finally, we mint the unclaimed rewards to the user
    let amount = calculate_rewards(unclaimed_days, ctx.accounts.config.rewards_bps, ctx.accounts.rewards_mint.decimals)?;
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
