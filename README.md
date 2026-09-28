# Anchor Core Staking

An Anchor program for staking [Metaplex Core](https://developers.metaplex.com/core) NFTs. Staked NFTs are frozen in the owner's wallet (no escrow). Rewards are minted from a program-controlled SPL token.

This README covers three features built on top of the base stake / unstake flow:

1. [Claim rewards without unstaking](#1-claim-rewards-without-unstaking): `claim_rewards`
2. [Burn-to-earn with BurnDelegate](#2-burn-to-earn-with-burndelegate): `burn_staked_nft`
3. [Collection-level staking stats](#3-collection-level-staking-stats): `total_staked` attribute on the Collection

## How staking state is stored

All staking state lives on the Core accounts themselves, as plugins. The program has no per-stake accounts.

| Account    | Plugin           | Authority                  | Purpose                                                     |
| ---------- | ---------------- | -------------------------- | ----------------------------------------------------------- |
| Asset      | `Attributes`     | Update authority PDA       | `staked`, `staked_at`, `last_claimed_at`                    |
| Asset      | `FreezeDelegate` | Update authority PDA       | Keeps the NFT frozen while staked                           |
| Asset      | `BurnDelegate`   | Update authority PDA       | Lets the program burn the NFT in `burn_staked_nft`          |
| Collection | `Attributes`     | Update authority PDA       | `total_staked` counter                                      |

The update authority PDA is derived from `["update_authority", collection]`. It is the collection's update authority, so it signs every Authority-Managed plugin change. `FreezeDelegate` and `BurnDelegate` are Owner-Managed plugins, so the owner signs to add them on `stake` and to remove them on `unstake`.

### Asset attributes

| Key               | Set on stake | Meaning                                                                 |
| ----------------- | ------------ | ----------------------------------------------------------------------- |
| `staked`          | `"true"`     | Whether the asset is currently staked                                   |
| `staked_at`       | now          | When the asset was staked. Used **only** for the freeze period check.   |
| `last_claimed_at` | now          | Start of the current reward accrual window. Used **only** for rewards.  |

On `unstake` these are reset to `"false"` / `"0"` / `"0"`. Any other attributes on the asset are preserved.

### Reward formula

Rewards are paid per **whole day** elapsed:

```
days   = floor((now - last_claimed_at) / 86400)
amount = days * rewards_bps * 10^decimals / 10000     // base units
```

With `rewards_bps = 10000`, an NFT earns 1 token per day. `rewards_bps` and `freeze_period` (in days) are set per collection by `initialize`.

---

## 1. Claim rewards without unstaking

**Instruction:** `claim_rewards`
**Source:** [`claim_rewards.rs`](programs/anchor-core-staking/src/instructions/claim_rewards.rs)

Mints accumulated rewards to the owner's rewards ATA, which is created if needed. The NFT stays staked and frozen.

**Behaviour**

- Fails with `AssetNotStaked` if the asset is not staked.
- Pays whole days since `last_claimed_at`. If less than a day has passed, it mints nothing and still succeeds.
- Moves `last_claimed_at` forward by exactly the days paid (`last_claimed_at += days * 86400`). The leftover partial day keeps accruing and is not lost.
- Does **not** touch `staked_at`. Claiming never restarts the freeze period, so a user can claim on day 3 and still unstake on day 7.
- Does **not** require the freeze period to have elapsed.
- `unstake` pays only the days since the last claim, so rewards are never paid twice.

**Accounts**

| Account                    | Notes                                              |
| -------------------------- | -------------------------------------------------- |
| `owner` (signer, mut)      | Must own the asset; pays for the ATA if created    |
| `config`                   | PDA `["config", collection]`                       |
| `asset` (mut)              | Must belong to `collection`                        |
| `collection` (mut)         |                                                    |
| `update_authority`         | PDA `["update_authority", collection]`             |
| `rewards_mint` (mut)       | PDA `["rewards_mint", config]`                     |
| `user_rewards_ata` (mut)   | Owner's ATA for the rewards mint (`init_if_needed`)|
| `token_program`, `associated_token_program`, `system_program`, `mpl_core_program` | |

**Example**

```ts
await program.methods
  .claimRewards()
  .accountsPartial({
    owner: wallet.publicKey,
    config,
    asset,
    collection,
    updateAuthority,
    rewardsMint,
    userRewardsAta,
    mplCoreProgram: MPL_CORE_PROGRAM_ID,
    tokenProgram: TOKEN_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  })
  .rpc();
```

---

## 2. Burn-to-earn with BurnDelegate

**Instruction:** `burn_staked_nft`
**Source:** [`burn_staked_nft.rs`](programs/anchor-core-staking/src/instructions/burn_staked_nft.rs)

Permanently burns a staked NFT and mints its unclaimed rewards plus a one-time bonus to the owner.

**How the BurnDelegate is used**

1. `stake` adds a `BurnDelegate` plugin to the asset, with the update authority PDA as its authority. The owner signs this.
2. `burn_staked_nft` first thaws the asset. The `FreezeDelegate` rejects burning a frozen asset, and a delegate's approval can't override that rejection.
3. It then calls Core's `BurnV1` with the **update authority PDA as the burn authority**, acting as the BurnDelegate.
4. `unstake` removes the `BurnDelegate` (and the `FreezeDelegate`), so an unstaked NFT carries no program delegates.

**Payout**

```
amount = unclaimed_rewards + BURN_BONUS_TOKENS * 10^decimals
```

`BURN_BONUS_TOKENS` is `1_000` and lives in [`constants.rs`](programs/anchor-core-staking/src/constants.rs).

**Behaviour**

- Fails with `AssetNotStaked` if the asset is not staked.
- Only the asset owner can call it (`has_one = owner`).
- There is no freeze period requirement: a staked NFT can be burned at any time.
- Decrements the collection's `total_staked`.
- After the burn, the Core asset account is left as a single uninitialized byte, and its rent is returned to the owner.

**Accounts:** the same as `claim_rewards`.

---

## 3. Collection-level staking stats

**Source:** `update_total_staked` in [`utils.rs`](programs/anchor-core-staking/src/utils.rs)

The Collection account holds an `Attributes` plugin with a `total_staked` counter, stored as a decimal string.

| Instruction       | Effect on `total_staked` |
| ----------------- | ------------------------ |
| `stake`           | `+1`                     |
| `unstake`         | `-1`                     |
| `burn_staked_nft` | `-1`                     |
| `claim_rewards`   | no change                |

- The plugin is created on the first `stake` in the collection (via `AddCollectionPluginV1`). After that it is updated in place (via `UpdateCollectionPluginV1`), signed by the update authority PDA.
- Other attributes on the collection are preserved.
- Overflow or underflow fails with `InvalidTotalStaked`.

**Reading it from a client**

```ts
import { fetchCollection } from "@metaplex-foundation/mpl-core";

const collection = await fetchCollection(umi, collectionAddress);
const totalStaked = collection.attributes?.attributeList
  .find((a) => a.key === "total_staked")?.value; // e.g. "2"
```

---

## Other change: re-staking

Previously `unstake` only thawed the `FreezeDelegate` and never removed it, so staking the same NFT again failed when `stake` tried to add the plugin a second time. `unstake` now removes both `FreezeDelegate` and `BurnDelegate` (owner-signed), so an NFT can be staked, unstaked and staked again.

## Errors

| Code | Name                      | When                                                  |
| ---- | ------------------------- | ----------------------------------------------------- |
| 6000 | `InvalidOwner`            | Signer does not own the asset                         |
| 6001 | `InvalidUpdateAuthority`  | Asset/collection not controlled by this program's PDA |
| 6002 | `AlreadyStaked`           | `stake` on a staked asset                             |
| 6003 | `AssetNotStaked`          | `unstake` / `claim_rewards` / `burn_staked_nft` on an unstaked asset |
| 6004 | `InvalidTimestamp`        | Malformed or future timestamp attribute               |
| 6005 | `FreezePeriodNotElapsed`  | `unstake` before `freeze_period` days                 |
| 6006 | `InvalidRewardsBps`       | Reward amount overflow                                |
| 6007 | `InvalidTotalStaked`      | `total_staked` malformed, overflow or underflow       |

## Building and testing

Requires Anchor CLI `0.31.1`, Solana `3.1.10` (pinned in `Anchor.toml`), yarn, and [surfpool](https://surfpool.run). The tests use surfpool's `surfnet_timeTravel` to skip past the freeze period.

```bash
anchor build
```

In one terminal, start surfpool. It deploys the program through the txtx runbook in `runbooks/deployment`:

```bash
surfpool start
```

Once it logs `Runbook 'deployment' execution completed`, run the tests in another terminal:

```bash
anchor test --skip-local-validator --skip-deploy --skip-build
```

Restart surfpool between test runs. The time-travel step moves the chain clock 8 days forward, and a second run on the same instance would already be past the freeze period.

The suite covers: staking two NFTs, the freeze period check, claiming (8 tokens after 8 days, then 0 on an immediate re-claim), unstaking after a claim (no double pay), claiming on an unstaked NFT (fails), burning (unclaimed + 1,000 bonus), the collection counter at each step, and re-staking an unstaked NFT.
