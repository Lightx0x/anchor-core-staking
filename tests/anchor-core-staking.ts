import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AnchorCoreStaking } from "../target/types/anchor_core_staking";
import { ComputeBudgetProgram, SystemProgram } from "@solana/web3.js";
import { MPL_CORE_PROGRAM_ID, deserializeAssetV1, deserializeCollectionV1 } from "@metaplex-foundation/mpl-core";
import { publicKey as umiPublicKey } from "@metaplex-foundation/umi";
import { ASSOCIATED_TOKEN_PROGRAM_ID, getAssociatedTokenAddressSync, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { assert } from "chai";

const MILLISECONDS_PER_DAY = 86400000;
const REWARDS_BPS = 10000;
const FREEZE_PERIOD_IN_DAYS = 7;
const TIME_TRAVEL_IN_DAYS = 8;
const BURN_BONUS_TOKENS = 1000;

describe("anchor-core-staking", () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.anchorCoreStaking as Program<AnchorCoreStaking>;

  // Generate a keypair for the collection
  const collectionKeypair = anchor.web3.Keypair.generate();

  // Find the update authority for the collection (PDA)
  const updateAuthority = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("update_authority"), collectionKeypair.publicKey.toBuffer()],
    program.programId
  )[0];

  // Generate a keypair for the nft asset
  const nftKeypair = anchor.web3.Keypair.generate();

  // Generate a keypair for a second nft asset (the one that will be burned)
  const burnNftKeypair = anchor.web3.Keypair.generate();

  // Find the config account (PDA)
  const config = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("config"), collectionKeypair.publicKey.toBuffer()],
    program.programId
  )[0];

  // Find the rewards mint account (PDA)
  const rewardsMint = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("rewards_mint"), config.toBuffer()],
    program.programId
  )[0];

  // User rewards ATA
  const userRewardsAta = getAssociatedTokenAddressSync(rewardsMint, provider.wallet.publicKey, false, TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID);

  // Helper to build an Umi RpcAccount so we can use the mpl-core deserializers
  async function fetchRpcAccount(address: anchor.web3.PublicKey): Promise<any> {
    const info = await provider.connection.getAccountInfo(address);
    if (!info) return null;
    return {
      publicKey: umiPublicKey(address.toBase58()),
      owner: umiPublicKey(info.owner.toBase58()),
      executable: info.executable,
      lamports: { basisPoints: BigInt(info.lamports), identifier: "SOL", decimals: 9 },
      rentEpoch: info.rentEpoch,
      data: new Uint8Array(info.data),
    };
  }

  async function fetchAsset(address: anchor.web3.PublicKey) {
    return deserializeAssetV1(await fetchRpcAccount(address));
  }

  function getAttribute(attributes: { attributeList: { key: string; value: string }[] } | undefined, key: string) {
    return attributes?.attributeList.find((attribute) => attribute.key === key)?.value;
  }

  async function getTotalStaked(): Promise<string | undefined> {
    const collection = deserializeCollectionV1(await fetchRpcAccount(collectionKeypair.publicKey));
    return getAttribute(collection.attributes, "total_staked");
  }

  async function getRewardsBalance(): Promise<number> {
    try {
      return (await provider.connection.getTokenAccountBalance(userRewardsAta)).value.uiAmount ?? 0;
    } catch {
      return 0;
    }
  }

  // Accounts shared by the instructions that mint rewards
  function rewardsAccounts(asset: anchor.web3.PublicKey) {
    return {
      owner: provider.wallet.publicKey,
      updateAuthority,
      config,
      rewardsMint,
      userRewardsAta,
      asset,
      collection: collectionKeypair.publicKey,
      mplCoreProgram: MPL_CORE_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    };
  }

  function stakeAccounts(asset: anchor.web3.PublicKey) {
    return {
      owner: provider.wallet.publicKey,
      updateAuthority,
      config,
      asset,
      collection: collectionKeypair.publicKey,
      systemProgram: SystemProgram.programId,
      mplCoreProgram: MPL_CORE_PROGRAM_ID,
    };
  }

  // Repeating an identical instruction within the same blockhash produces a byte-identical transaction,
  // which the validator rejects as "already processed". A unique priority fee makes each one distinct.
  let uniqueNonce = 0;
  function uniqueIx() {
    return ComputeBudgetProgram.setComputeUnitPrice({ microLamports: ++uniqueNonce });
  }

  // Helper function to advance time with Surfpool 
  async function advanceTime(params: { absoluteEpoch?: number; absoluteSlot?: number; absoluteTimestamp?: number }): Promise<void> {
    const rpcResponse = await fetch(provider.connection.rpcEndpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "surfnet_timeTravel",
        params: [params],
      }),
    });

    const result = await rpcResponse.json() as { error?: any; result?: any };
    if (result.error) {
      throw new Error(`Time travel failed: ${JSON.stringify(result.error)}`);
    }
    
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }

  it("Create a collection", async () => {
    const collectionName = "Test Collection";
    const collectionUri = "https://example.com/collection";
    const tx = await program.methods.createCollection(collectionName, collectionUri)
    .accountsPartial({
      payer: provider.wallet.publicKey,
      collection: collectionKeypair.publicKey,
      updateAuthority,
      systemProgram: SystemProgram.programId,
      mplCoreProgram: MPL_CORE_PROGRAM_ID,
    })
    .signers([collectionKeypair])
    .rpc();
    console.log("\nYour transaction signature", tx);
    console.log("Collection address", collectionKeypair.publicKey.toBase58());
  });

  it("Mint an NFT", async () => {
    const nftName = "Test NFT";
    const nftUri = "https://example.com/nft";
    const tx = await program.methods.mintAsset(nftName, nftUri)
    .accountsPartial({
      user: provider.wallet.publicKey,
      asset: nftKeypair.publicKey,
      collection: collectionKeypair.publicKey,
      updateAuthority,
      systemProgram: SystemProgram.programId,
      mplCoreProgram: MPL_CORE_PROGRAM_ID,
    })
    .signers([nftKeypair])
    .rpc();
    console.log("\nYour transaction signature", tx);
    console.log("NFT address", nftKeypair.publicKey.toBase58());
  });

  it("Mint a second NFT (to be burned)", async () => {
    const tx = await program.methods.mintAsset("Burn NFT", "https://example.com/burn-nft")
    .accountsPartial({
      user: provider.wallet.publicKey,
      asset: burnNftKeypair.publicKey,
      collection: collectionKeypair.publicKey,
      updateAuthority,
      systemProgram: SystemProgram.programId,
      mplCoreProgram: MPL_CORE_PROGRAM_ID,
    })
    .signers([burnNftKeypair])
    .rpc();
    console.log("\nYour transaction signature", tx);
  });

  it("Initialize Config", async () => {
    const tx = await program.methods.initialize(REWARDS_BPS, FREEZE_PERIOD_IN_DAYS)
    .accountsPartial({
      admin: provider.wallet.publicKey,
      collection: collectionKeypair.publicKey,
      updateAuthority,
      config,
      rewardsMint,
      systemProgram: SystemProgram.programId,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .rpc();
    console.log("\nYour transaction signature", tx);
    console.log("Config address", config.toBase58());
    console.log("Rewards BPS", REWARDS_BPS);
    console.log("Freeze period in days", FREEZE_PERIOD_IN_DAYS);
    console.log("Rewards mint address", rewardsMint.toBase58());
  });

  it("Stake an NFT", async () => {
    const tx = await program.methods.stake()
    .accountsPartial(stakeAccounts(nftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);

    const asset = await fetchAsset(nftKeypair.publicKey);
    assert.equal(getAttribute(asset.attributes, "staked"), "true");
    assert.equal(asset.freezeDelegate?.frozen, true);
    assert.isDefined(asset.burnDelegate);
    assert.equal(await getTotalStaked(), "1");
  });

  it("Stake the second NFT", async () => {
    const tx = await program.methods.stake()
    .accountsPartial(stakeAccounts(burnNftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);
    assert.equal(await getTotalStaked(), "2");
  });

  it("Try to unstake an NFT before the freeze period ends", async () => {
    try {
      const tx = await program.methods.unstake()
      .accountsPartial(rewardsAccounts(nftKeypair.publicKey))
      .rpc();
      throw new Error(`Unstake should have failed before freeze period elapsed, but succeeded with tx: ${tx}`);
    } catch (err) {
      if (err instanceof anchor.AnchorError && err.error.errorCode.code === "FreezePeriodNotElapsed") {
        console.log("\nUnstake failed as expected:", err.error.errorMessage);
      } else {
        throw err;
      }
    }
  });

  it("Time travel to the future", async () => {
    // Advance time in milliseconds
    const currentTimestamp = Date.now();
    await advanceTime({ absoluteTimestamp: currentTimestamp + TIME_TRAVEL_IN_DAYS * MILLISECONDS_PER_DAY });
    console.log("\nTime traveled in days", TIME_TRAVEL_IN_DAYS)
  });

  it("Claim rewards without unstaking", async () => {
    const stakedAtBefore = getAttribute((await fetchAsset(nftKeypair.publicKey)).attributes, "staked_at");
    const balanceBefore = await getRewardsBalance();

    const tx = await program.methods.claimRewards()
    .accountsPartial(rewardsAccounts(nftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);

    const claimed = (await getRewardsBalance()) - balanceBefore;
    console.log("Claimed rewards", claimed);
    assert.isAtLeast(claimed, TIME_TRAVEL_IN_DAYS * REWARDS_BPS / 10000);

    // The NFT is still staked and frozen, and the freeze period clock was not reset
    const asset = await fetchAsset(nftKeypair.publicKey);
    assert.equal(getAttribute(asset.attributes, "staked"), "true");
    assert.equal(getAttribute(asset.attributes, "staked_at"), stakedAtBefore);
    assert.equal(asset.freezeDelegate?.frozen, true);
    assert.equal(await getTotalStaked(), "2");
  });

  it("Claiming again right away mints nothing", async () => {
    const balanceBefore = await getRewardsBalance();
    await program.methods.claimRewards()
    .accountsPartial(rewardsAccounts(nftKeypair.publicKey))
    .preInstructions([uniqueIx()])
    .rpc();
    assert.equal(await getRewardsBalance(), balanceBefore);
  });

  it("Unstake an NFT", async () => {
    const balanceBefore = await getRewardsBalance();
    const tx = await program.methods.unstake()
    .accountsPartial(rewardsAccounts(nftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);
    console.log("User rewards balance", await getRewardsBalance());

    // Rewards were already claimed, so unstaking mints nothing more
    assert.equal(await getRewardsBalance(), balanceBefore);

    const asset = await fetchAsset(nftKeypair.publicKey);
    assert.equal(getAttribute(asset.attributes, "staked"), "false");
    assert.isUndefined(asset.freezeDelegate);
    assert.isUndefined(asset.burnDelegate);
    assert.equal(await getTotalStaked(), "1");
  });

  it("Try to claim rewards for an unstaked NFT", async () => {
    try {
      await program.methods.claimRewards()
      .accountsPartial(rewardsAccounts(nftKeypair.publicKey))
      .preInstructions([uniqueIx()])
      .rpc();
      throw new Error("Claim should have failed for an unstaked NFT");
    } catch (err) {
      if (err instanceof anchor.AnchorError && err.error.errorCode.code === "AssetNotStaked") {
        console.log("\nClaim failed as expected:", err.error.errorMessage);
      } else {
        throw err;
      }
    }
  });

  it("Burn a staked NFT for the bonus", async () => {
    const balanceBefore = await getRewardsBalance();
    const tx = await program.methods.burnStakedNft()
    .accountsPartial(rewardsAccounts(burnNftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);

    const received = (await getRewardsBalance()) - balanceBefore;
    console.log("Burn rewards (unclaimed + bonus)", received);
    assert.isAtLeast(received, BURN_BONUS_TOKENS + TIME_TRAVEL_IN_DAYS * REWARDS_BPS / 10000);

    // A burned Core asset is left as a single byte (Key::Uninitialized)
    const burned = await provider.connection.getAccountInfo(burnNftKeypair.publicKey);
    assert.isTrue(burned === null || burned.data.length <= 1);
    assert.equal(await getTotalStaked(), "0");
  });

  it("Re-stake the unstaked NFT", async () => {
    const tx = await program.methods.stake()
    .accountsPartial(stakeAccounts(nftKeypair.publicKey))
    .rpc();
    console.log("\nYour transaction signature", tx);

    const asset = await fetchAsset(nftKeypair.publicKey);
    assert.equal(getAttribute(asset.attributes, "staked"), "true");
    assert.equal(asset.freezeDelegate?.frozen, true);
    assert.equal(await getTotalStaked(), "1");
  });
});
