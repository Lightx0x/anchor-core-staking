use anchor_lang::prelude::*;

#[constant]
pub const SEED: &str = "anchor";

pub const SECONDS_PER_DAY: i64 = 86400;

// One-time bonus (in whole reward tokens) paid when a staked NFT is burned
#[constant]
pub const BURN_BONUS_TOKENS: u64 = 1_000;

// Asset attribute keys
pub const STAKED_KEY: &str = "staked";
pub const STAKED_AT_KEY: &str = "staked_at";
pub const LAST_CLAIMED_AT_KEY: &str = "last_claimed_at";

// Collection attribute keys
pub const TOTAL_STAKED_KEY: &str = "total_staked";
