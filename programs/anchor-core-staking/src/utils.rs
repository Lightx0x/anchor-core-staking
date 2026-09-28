use anchor_lang::prelude::*;
use anchor_spl::token_interface::{mint_to_checked, MintToChecked};
use mpl_core::{
    accounts::{BaseAssetV1, BaseCollectionV1},
    fetch_plugin,
    instructions::{AddCollectionPluginV1CpiBuilder, UpdateCollectionPluginV1CpiBuilder},
    types::{Attribute, Attributes, Plugin, PluginAuthority, PluginType},
};
use crate::constants::*;
use crate::error::ErrorCode;

/// Staking data read from the asset Attributes Plugin of a staked asset
pub struct StakeInfo {
    pub staked_at: i64,
    pub last_claimed_at: i64,
    /// All the attributes that are not staking related
    pub other_attributes: Vec<Attribute>,
}

/// Reads the staking attributes of the asset, failing if the asset is not staked
pub fn read_stake_info(asset: &AccountInfo) -> Result<StakeInfo> {
    let attributes = fetch_plugin::<BaseAssetV1, Attributes>(asset, PluginType::Attributes)
        .map(|(_, attrs, _)| attrs)
        .map_err(|_| ErrorCode::AssetNotStaked)?;

    let mut staked = false;
    let mut staked_at: Option<i64> = None;
    let mut last_claimed_at: Option<i64> = None;
    let mut other_attributes = Vec::with_capacity(attributes.attribute_list.len());

    for attribute in attributes.attribute_list {
        match attribute.key.as_str() {
            STAKED_KEY => staked = attribute.value == "true",
            STAKED_AT_KEY => staked_at = Some(parse_timestamp(&attribute.value)?),
            LAST_CLAIMED_AT_KEY => last_claimed_at = Some(parse_timestamp(&attribute.value)?),
            _ => other_attributes.push(attribute),
        }
    }

    require!(staked, ErrorCode::AssetNotStaked);
    let staked_at = staked_at.ok_or(ErrorCode::InvalidTimestamp)?;

    Ok(StakeInfo {
        staked_at,
        // Assets staked before claiming existed have no "last_claimed_at"
        last_claimed_at: last_claimed_at.unwrap_or(staked_at),
        other_attributes,
    })
}

fn parse_timestamp(value: &str) -> Result<i64> {
    value.parse::<i64>().map_err(|_| ErrorCode::InvalidTimestamp.into())
}

/// Whole days elapsed between `from` and `to`
pub fn days_between(from: i64, to: i64) -> Result<i64> {
    let seconds = to.checked_sub(from).ok_or(ErrorCode::InvalidTimestamp)?;
    require!(seconds >= 0, ErrorCode::InvalidTimestamp);
    Ok(seconds / SECONDS_PER_DAY)
}

/// Rewards (in base units) earned for staking `days` days
pub fn calculate_rewards(days: i64, rewards_bps: u16, decimals: u8) -> Result<u64> {
    Ok((days as u64)
        .checked_mul(rewards_bps as u64)
        .ok_or(ErrorCode::InvalidRewardsBps)?
        .checked_mul(10u64.pow(decimals as u32))
        .ok_or(ErrorCode::InvalidRewardsBps)?
        .checked_div(10000u64)
        .ok_or(ErrorCode::InvalidRewardsBps)?)
}

/// Mints reward tokens, signed by the config PDA (the mint authority)
pub fn mint_rewards<'info>(
    token_program: AccountInfo<'info>,
    mint: AccountInfo<'info>,
    to: AccountInfo<'info>,
    config: AccountInfo<'info>,
    collection_key: &Pubkey,
    config_bump: u8,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }

    let config_seeds = &[b"config", collection_key.as_ref(), &[config_bump]];

    mint_to_checked(
        CpiContext::new_with_signer(
            token_program,
            MintToChecked { mint, to, authority: config },
            &[&config_seeds[..]],
        ),
        amount,
        decimals,
    )
}

/// Increments (or decrements) the "total_staked" Attribute on the Collection account.
/// The Attributes Plugin is Authority-Managed, so it is signed by the update authority PDA.
pub fn update_total_staked<'info>(
    mpl_core_program: &AccountInfo<'info>,
    collection: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    update_authority: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    update_authority_bump: u8,
    increment: bool,
) -> Result<()> {
    let attributes_fetched: Option<Attributes> =
        fetch_plugin::<BaseCollectionV1, Attributes>(collection, PluginType::Attributes)
            .ok()
            .map(|(_, attrs, _)| attrs);

    // Keep every other collection attribute, and read the current counter
    let mut total_staked: u64 = 0;
    let mut attributes_list: Vec<Attribute> = Vec::new();
    if let Some(attributes) = &attributes_fetched {
        for attribute in &attributes.attribute_list {
            if attribute.key == TOTAL_STAKED_KEY {
                total_staked = attribute.value.parse::<u64>().map_err(|_| ErrorCode::InvalidTotalStaked)?;
            } else {
                attributes_list.push(attribute.clone());
            }
        }
    }

    total_staked = if increment {
        total_staked.checked_add(1)
    } else {
        total_staked.checked_sub(1)
    }
    .ok_or(ErrorCode::InvalidTotalStaked)?;

    attributes_list.push(Attribute {
        key: TOTAL_STAKED_KEY.to_string(),
        value: total_staked.to_string(),
    });

    let collection_key = collection.key();
    let signer_seeds = &[b"update_authority", collection_key.as_ref(), &[update_authority_bump]];
    let plugin = Plugin::Attributes(Attributes { attribute_list: attributes_list });

    if attributes_fetched.is_none() {
        AddCollectionPluginV1CpiBuilder::new(mpl_core_program)
            .collection(collection)
            .payer(payer)
            .authority(Some(update_authority))
            .system_program(system_program)
            .plugin(plugin)
            .init_authority(PluginAuthority::UpdateAuthority)
            .invoke_signed(&[signer_seeds])?;
    } else {
        UpdateCollectionPluginV1CpiBuilder::new(mpl_core_program)
            .collection(collection)
            .payer(payer)
            .authority(Some(update_authority))
            .system_program(system_program)
            .plugin(plugin)
            .invoke_signed(&[signer_seeds])?;
    }

    Ok(())
}
