use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct QueryCmd {
    #[command(subcommand)]
    command: QuerySubcommand,
}

#[derive(Subcommand, Debug)]
enum QuerySubcommand {
    /// Query the current tip
    Tip {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query UTxOs at an address
    Utxo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query protocol parameters
    ProtocolParameters {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake distribution
    StakeDistribution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake address info
    StakeAddressInfo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query governance state (Conway era)
    GovState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query DRep state (Conway era)
    DrepState {
        #[arg(long)]
        drep_key_hash: Option<String>,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query committee state (Conway era)
    CommitteeState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query the transaction mempool
    TxMempool {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        /// Subcommand: info, next-tx, has-tx
        #[arg(default_value = "info")]
        subcmd: String,
        /// Transaction ID (for has-tx)
        #[arg(long)]
        tx_id: Option<String>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query registered stake pools
    StakePools {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake snapshots (mark/set/go)
    StakeSnapshot {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
        /// Filter by pool ID (bech32 or hex)
        #[arg(long)]
        stake_pool_id: Option<String>,
    },
    /// Query stake pool parameters
    PoolParams {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
        /// Pool ID to query (bech32 or hex)
        #[arg(long)]
        stake_pool_id: Option<String>,
    },
    /// Query treasury and reserves (account state)
    Treasury {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Compute the leader schedule for a stake pool
    LeadershipSchedule {
        /// Path to the VRF signing key file
        #[arg(long)]
        vrf_signing_key_file: PathBuf,
        /// Epoch nonce (64-character hex string)
        #[arg(long)]
        epoch_nonce: String,
        /// First slot of the epoch
        #[arg(long)]
        epoch_start_slot: u64,
        /// Number of slots in the epoch
        #[arg(long, default_value = "432000")]
        epoch_length: u64,
        /// Pool's relative stake (0.0 to 1.0)
        #[arg(long)]
        relative_stake: f64,
        /// Active slot coefficient (default: 0.05)
        #[arg(long, default_value = "0.05")]
        active_slot_coeff: f64,
    },
}

/// Map era index to era name
fn era_name(era: u32) -> &'static str {
    match era {
        0 => "Byron",
        1 => "Shelley",
        2 => "Allegra",
        3 => "Mary",
        4 => "Alonzo",
        5 => "Babbage",
        6 => "Conway",
        _ => "Unknown",
    }
}

/// Connect to the node, perform handshake, and acquire state
async fn connect_and_handshake(
    socket_path: &std::path::Path,
    testnet_magic: Option<u64>,
) -> Result<torsten_network::N2CClient> {
    let mut client = torsten_network::N2CClient::connect(socket_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Cannot connect to node socket '{}': {e}\nIs the node running?",
                socket_path.display()
            )
        })?;

    let magic = testnet_magic.unwrap_or(764824073);

    client
        .handshake(magic)
        .await
        .map_err(|e| anyhow::anyhow!("Handshake failed: {e}"))?;

    Ok(client)
}

async fn connect_and_acquire(
    socket_path: &std::path::Path,
    testnet_magic: Option<u64>,
) -> Result<torsten_network::N2CClient> {
    let mut client = connect_and_handshake(socket_path, testnet_magic).await?;

    client
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to acquire state: {e}"))?;

    Ok(client)
}

/// Release state and disconnect
async fn release_and_done(client: &mut torsten_network::N2CClient) {
    client.release().await.ok();
    client.done().await.ok();
}

impl QueryCmd {
    pub fn run(self) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(self.run_async())
    }

    async fn run_async(self) -> Result<()> {
        match self.command {
            QuerySubcommand::Tip {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;

                let epoch = client.query_epoch().await.unwrap_or(0);
                let era = client.query_era().await.unwrap_or(6);
                let block_no = client.query_block_no().await.unwrap_or(tip.block_no);

                release_and_done(&mut client).await;

                let hash_hex = hex::encode(&tip.hash);
                let era_str = era_name(era);

                // Estimate sync progress based on slot vs current time
                // Shelley mainnet started at Unix time 1596059091 (slot 4492800)
                // Preview/testnet may differ, but this gives a reasonable estimate
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let shelley_start_time = if testnet_magic.is_some() {
                    1_666_656_000u64 // Preview genesis
                } else {
                    1_596_059_091u64 // Mainnet Shelley start
                };
                let elapsed_secs = now_secs.saturating_sub(shelley_start_time);
                let expected_tip_slot = elapsed_secs; // 1 slot = 1 second
                let sync_progress = if expected_tip_slot > 0 {
                    (tip.slot as f64 / expected_tip_slot as f64 * 100.0).min(100.0)
                } else {
                    100.0
                };

                println!("{{");
                println!("    \"slot\": {},", tip.slot);
                println!("    \"hash\": \"{hash_hex}\",");
                println!("    \"block\": {block_no},");
                println!("    \"epoch\": {epoch},");
                println!("    \"era\": \"{era_str}\",");
                println!("    \"syncProgress\": \"{sync_progress:.2}\"");
                println!("}}");
                Ok(())
            }
            QuerySubcommand::Utxo {
                address,
                socket_path,
                testnet_magic,
            } => {
                // Decode bech32 address to raw bytes
                let (_, addr_bytes) = bech32::decode(&address)
                    .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_utxo_by_address(&addr_bytes)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query UTxOs: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, array[map{...}]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);

                if arr_len == 0 {
                    println!("No UTxOs found at {address}");
                    return Ok(());
                }

                println!("{:<68} {:>6} {:>20}", "TxHash#Ix", "Datum", "Lovelace");
                println!("{}", "-".repeat(96));

                for _ in 0..arr_len {
                    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                    let mut tx_hash = String::new();
                    let mut output_index = 0u32;
                    let mut lovelace = 0u64;
                    let mut has_datum = false;

                    for _ in 0..map_len {
                        let key = decoder.str().unwrap_or("");
                        match key {
                            "tx_hash" => tx_hash = hex::encode(decoder.bytes().unwrap_or(&[])),
                            "output_index" => output_index = decoder.u32().unwrap_or(0),
                            "lovelace" => lovelace = decoder.u64().unwrap_or(0),
                            "has_datum" => has_datum = decoder.bool().unwrap_or(false),
                            _ => {
                                decoder.skip().ok();
                            }
                        }
                    }

                    let utxo_ref = format!("{tx_hash}#{output_index}");
                    let datum_str = if has_datum { "yes" } else { "no" };
                    println!("{utxo_ref:<68} {datum_str:>6} {lovelace:>20}");
                }

                println!("\nTotal UTxOs: {arr_len}");
                Ok(())
            }
            QuerySubcommand::ProtocolParameters {
                socket_path,
                out_file,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let json = match client.query_protocol_params().await {
                    Ok(params_json) if !params_json.is_empty() => params_json,
                    _ => {
                        // Fall back to defaults if node doesn't have params
                        let params =
                            torsten_primitives::protocol_params::ProtocolParameters::mainnet_defaults();
                        serde_json::to_string_pretty(&params)?
                    }
                };

                release_and_done(&mut client).await;

                if let Some(out) = out_file {
                    std::fs::write(&out, &json)?;
                    println!("Protocol parameters written to: {}", out.display());
                } else {
                    println!("{json}");
                }
                Ok(())
            }
            QuerySubcommand::StakeDistribution {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_stake_distribution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake distribution: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, map{pool_id => [stake, pledge, cost]}]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                println!(
                    "{:<66} {:>20} {:>20}",
                    "PoolId", "Stake (lovelace)", "Pledge (lovelace)"
                );
                println!("{}", "-".repeat(106));

                for _ in 0..map_len {
                    let pool_id = decoder.bytes().unwrap_or(&[]);
                    let pool_hex = hex::encode(pool_id);
                    let _ = decoder.array();
                    let stake = decoder.u64().unwrap_or(0);
                    let pledge = decoder.u64().unwrap_or(0);
                    let _cost = decoder.u64().unwrap_or(0);
                    println!("{pool_hex:<66} {stake:>20} {pledge:>20}");
                }

                println!("\nTotal pools: {map_len}");
                Ok(())
            }
            QuerySubcommand::StakeAddressInfo {
                address,
                socket_path,
                testnet_magic,
            } => {
                // Decode bech32 address to get credential hash
                let (_, addr_bytes) = bech32::decode(&address)
                    .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;

                // For a reward/stake address, the credential hash is bytes 1..29
                let credential_hex = if addr_bytes.len() >= 29 {
                    hex::encode(&addr_bytes[1..29])
                } else {
                    hex::encode(&addr_bytes)
                };

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_stake_address_info()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake address info: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, array[map{...}]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                let mut found = false;

                println!("[");
                for i in 0..arr_len {
                    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                    let mut cred = String::new();
                    let mut pool = String::new();
                    let mut rewards = 0u64;

                    for _ in 0..map_len {
                        let key = decoder.str().unwrap_or("");
                        match key {
                            "credential" => cred = hex::encode(decoder.bytes().unwrap_or(&[])),
                            "delegated_pool" => {
                                pool = decoder.bytes().map(hex::encode).unwrap_or_default()
                            }
                            "reward_balance" => rewards = decoder.u64().unwrap_or(0),
                            _ => {
                                decoder.skip().ok();
                            }
                        }
                    }

                    // Filter to match the requested address
                    if cred.contains(&credential_hex) || credential_hex.is_empty() {
                        found = true;
                        let comma = if i + 1 < arr_len { "," } else { "" };
                        println!("  {{");
                        println!("    \"address\": \"{address}\",");
                        if pool.is_empty() {
                            println!("    \"delegation\": null,");
                        } else {
                            println!("    \"delegation\": \"{pool}\",");
                        }
                        println!("    \"rewardAccountBalance\": {}", rewards);
                        println!("  }}{comma}");
                    }
                }
                println!("]");

                if !found {
                    println!("No stake address info found for {address}");
                }

                Ok(())
            }
            QuerySubcommand::GovState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_gov_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query governance state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, map{...}]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut drep_count = 0u64;
                let mut committee_count = 0u64;
                let mut treasury = 0u64;
                let mut proposals = Vec::new();

                for _ in 0..map_len {
                    let key = decoder.str().unwrap_or("");
                    match key {
                        "drep_count" => drep_count = decoder.u64().unwrap_or(0),
                        "committee_member_count" => committee_count = decoder.u64().unwrap_or(0),
                        "treasury" => treasury = decoder.u64().unwrap_or(0),
                        "proposals" => {
                            let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                let pmap_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                let mut tx_id = String::new();
                                let mut action_idx = 0u32;
                                let mut action_type = String::new();
                                let mut yes = 0u64;
                                let mut no = 0u64;
                                let mut abstain = 0u64;
                                for _ in 0..pmap_len {
                                    let pkey = decoder.str().unwrap_or("");
                                    match pkey {
                                        "tx_id" => {
                                            tx_id = hex::encode(decoder.bytes().unwrap_or(&[]))
                                        }
                                        "action_index" => action_idx = decoder.u32().unwrap_or(0),
                                        "action_type" => {
                                            action_type = decoder.str().unwrap_or("").to_string()
                                        }
                                        "yes_votes" => yes = decoder.u64().unwrap_or(0),
                                        "no_votes" => no = decoder.u64().unwrap_or(0),
                                        "abstain_votes" => abstain = decoder.u64().unwrap_or(0),
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                proposals.push((tx_id, action_idx, action_type, yes, no, abstain));
                            }
                        }
                        _ => {
                            decoder.skip().ok();
                        }
                    }
                }

                println!("Governance State (Conway)");
                println!("========================");
                println!("Treasury:         {} ADA", treasury / 1_000_000);
                println!("Registered DReps: {drep_count}");
                println!("Committee Members: {committee_count}");
                println!("Active Proposals: {}", proposals.len());

                if !proposals.is_empty() {
                    println!("\nProposals:");
                    println!(
                        "{:<20} {:<8} {:>6} {:>6} {:>8}",
                        "Type", "TxId", "Yes", "No", "Abstain"
                    );
                    println!("{}", "-".repeat(52));
                    for (tx_id, idx, action_type, yes, no, abstain) in &proposals {
                        let short_tx = if tx_id.len() > 8 {
                            format!("{}#{idx}", &tx_id[..8])
                        } else {
                            format!("{tx_id}#{idx}")
                        };
                        println!("{action_type:<20} {short_tx:<8} {yes:>6} {no:>6} {abstain:>8}");
                    }
                }

                Ok(())
            }
            QuerySubcommand::DrepState {
                drep_key_hash,
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_drep_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query DRep state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, array[map{...}]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                let mut dreps = Vec::new();

                for _ in 0..arr_len {
                    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                    let mut cred = String::new();
                    let mut deposit = 0u64;
                    let mut anchor = String::new();
                    let mut reg_epoch = 0u64;

                    for _ in 0..map_len {
                        let key = decoder.str().unwrap_or("");
                        match key {
                            "credential" => cred = hex::encode(decoder.bytes().unwrap_or(&[])),
                            "deposit" => deposit = decoder.u64().unwrap_or(0),
                            "anchor_url" => {
                                anchor = decoder.str().map(|s| s.to_string()).unwrap_or_default()
                            }
                            "registered_epoch" => reg_epoch = decoder.u64().unwrap_or(0),
                            _ => {
                                decoder.skip().ok();
                            }
                        }
                    }
                    dreps.push((cred, deposit, anchor, reg_epoch));
                }

                // Filter by key hash if provided
                let filtered: Vec<_> = if let Some(ref hash) = drep_key_hash {
                    dreps
                        .iter()
                        .filter(|(c, _, _, _)| c.contains(hash))
                        .collect()
                } else {
                    dreps.iter().collect()
                };

                println!("DRep State (Conway)");
                println!("===================");
                println!("Total DReps: {}", dreps.len());

                if !filtered.is_empty() {
                    println!(
                        "\n{:<66} {:>16} {:>8}",
                        "Credential Hash", "Deposit (ADA)", "Epoch"
                    );
                    println!("{}", "-".repeat(92));
                    for (cred, deposit, anchor, epoch) in &filtered {
                        let deposit_ada = *deposit / 1_000_000;
                        println!("{cred:<66} {deposit_ada:>16} {epoch:>8}");
                        if !anchor.is_empty() {
                            println!("  Anchor: {anchor}");
                        }
                    }
                }

                Ok(())
            }
            QuerySubcommand::CommitteeState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_committee_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query committee state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, map{"members": [...], "resigned": [...]}]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut members = Vec::new();
                let mut resigned = Vec::new();

                for _ in 0..map_len {
                    let key = decoder.str().unwrap_or("");
                    match key {
                        "members" => {
                            let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                let mmap_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                let mut cold = String::new();
                                let mut hot = String::new();
                                for _ in 0..mmap_len {
                                    let mkey = decoder.str().unwrap_or("");
                                    match mkey {
                                        "cold" => {
                                            cold = hex::encode(decoder.bytes().unwrap_or(&[]))
                                        }
                                        "hot" => hot = hex::encode(decoder.bytes().unwrap_or(&[])),
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                members.push((cold, hot));
                            }
                        }
                        "resigned" => {
                            let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                resigned.push(hex::encode(decoder.bytes().unwrap_or(&[])));
                            }
                        }
                        _ => {
                            decoder.skip().ok();
                        }
                    }
                }

                println!("Constitutional Committee State (Conway)");
                println!("=======================================");
                println!("Active Members: {}", members.len());
                println!("Resigned Members: {}", resigned.len());

                if !members.is_empty() {
                    println!("\n{:<66} {:<66}", "Cold Credential", "Hot Credential");
                    println!("{}", "-".repeat(134));
                    for (cold, hot) in &members {
                        println!("{cold:<66} {hot:<66}");
                    }
                }

                if !resigned.is_empty() {
                    println!("\nResigned:");
                    for cred in &resigned {
                        println!("  {cred}");
                    }
                }

                Ok(())
            }
            QuerySubcommand::TxMempool {
                socket_path,
                subcmd,
                tx_id,
                testnet_magic,
            } => {
                let mut client = connect_and_handshake(&socket_path, testnet_magic).await?;

                match subcmd.as_str() {
                    "info" => {
                        let slot = client
                            .monitor_acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor acquire failed: {e}"))?;
                        let (capacity, size, num_txs) = client
                            .monitor_get_sizes()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor get sizes failed: {e}"))?;

                        println!("Mempool snapshot at slot {slot}:");
                        println!("  Capacity:     {capacity} bytes");
                        println!("  Size:         {size} bytes");
                        println!("  Transactions: {num_txs}");

                        let _ = client.monitor_done().await;
                    }
                    "has-tx" => {
                        let id = tx_id.ok_or_else(|| {
                            anyhow::anyhow!("--tx-id is required for has-tx subcommand")
                        })?;
                        let hash_bytes = hex::decode(&id)
                            .map_err(|e| anyhow::anyhow!("Invalid tx ID hex: {e}"))?;
                        if hash_bytes.len() != 32 {
                            return Err(anyhow::anyhow!(
                                "Transaction ID must be 32 bytes (64 hex chars)"
                            ));
                        }
                        let mut tx_hash = [0u8; 32];
                        tx_hash.copy_from_slice(&hash_bytes);

                        let _slot = client
                            .monitor_acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor acquire failed: {e}"))?;
                        let has_tx = client
                            .monitor_has_tx(&tx_hash)
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor has-tx failed: {e}"))?;

                        if has_tx {
                            println!("Transaction {id} is in the mempool");
                        } else {
                            println!("Transaction {id} is NOT in the mempool");
                        }

                        let _ = client.monitor_done().await;
                    }
                    _ => {
                        println!("Available subcommands: info, has-tx");
                    }
                }
                Ok(())
            }
            QuerySubcommand::StakePools {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_stake_distribution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake pools: {e}"))?;

                release_and_done(&mut client).await;

                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                struct PoolInfo {
                    pool_id: String,
                    stake: u64,
                    pledge: u64,
                    cost: u64,
                    margin_num: u64,
                    margin_den: u64,
                }
                let mut pools = Vec::new();
                for _ in 0..arr_len {
                    let mut pi = PoolInfo {
                        pool_id: String::new(),
                        stake: 0,
                        pledge: 0,
                        cost: 0,
                        margin_num: 0,
                        margin_den: 1,
                    };
                    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                    for _ in 0..map_len {
                        let key = decoder.str().unwrap_or("");
                        match key {
                            "pool_id" => {
                                pi.pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                            }
                            "stake" => {
                                pi.stake = decoder.u64().unwrap_or(0);
                            }
                            "pledge" => {
                                pi.pledge = decoder.u64().unwrap_or(0);
                            }
                            "cost" => {
                                pi.cost = decoder.u64().unwrap_or(0);
                            }
                            "margin_num" => {
                                pi.margin_num = decoder.u64().unwrap_or(0);
                            }
                            "margin_den" => {
                                pi.margin_den = decoder.u64().unwrap_or(1);
                            }
                            _ => {
                                decoder.skip().ok();
                            }
                        }
                    }
                    pools.push(pi);
                }

                println!(
                    "{:<58} {:>15} {:>15} {:>8}",
                    "PoolId", "Pledge (ADA)", "Cost (ADA)", "Margin"
                );
                println!("{}", "-".repeat(100));
                for p in &pools {
                    let margin_pct = if p.margin_den > 0 {
                        p.margin_num as f64 / p.margin_den as f64 * 100.0
                    } else {
                        0.0
                    };
                    println!(
                        "{:<58} {:>15.6} {:>15.6} {:>7.2}%",
                        p.pool_id,
                        p.pledge as f64 / 1_000_000.0,
                        p.cost as f64 / 1_000_000.0,
                        margin_pct
                    );
                }
                println!("\nTotal pools: {}", pools.len());
                Ok(())
            }
            QuerySubcommand::StakeSnapshot {
                socket_path,
                testnet_magic,
                stake_pool_id,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let result = client
                    .query_stake_snapshot()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake snapshot: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, map{"pools": [...], "total_mark_stake": n, ...}]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut total_mark = 0u64;
                let mut total_set = 0u64;
                let mut total_go = 0u64;
                let mut pools: Vec<(String, u64, u64, u64)> = Vec::new();

                for _ in 0..map_len {
                    let key = decoder.str().unwrap_or("");
                    match key {
                        "pools" => {
                            let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                let pmap_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                let mut pool_id = String::new();
                                let mut mark = 0u64;
                                let mut set = 0u64;
                                let mut go = 0u64;
                                for _ in 0..pmap_len {
                                    let pkey = decoder.str().unwrap_or("");
                                    match pkey {
                                        "pool_id" => {
                                            pool_id = hex::encode(decoder.bytes().unwrap_or(&[]))
                                        }
                                        "mark_stake" => mark = decoder.u64().unwrap_or(0),
                                        "set_stake" => set = decoder.u64().unwrap_or(0),
                                        "go_stake" => go = decoder.u64().unwrap_or(0),
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                pools.push((pool_id, mark, set, go));
                            }
                        }
                        "total_mark_stake" => total_mark = decoder.u64().unwrap_or(0),
                        "total_set_stake" => total_set = decoder.u64().unwrap_or(0),
                        "total_go_stake" => total_go = decoder.u64().unwrap_or(0),
                        _ => {
                            decoder.skip().ok();
                        }
                    }
                }

                println!("Stake Snapshot");
                println!("==============");
                println!("Total Mark Stake: {} ADA", total_mark / 1_000_000);
                println!("Total Set Stake:  {} ADA", total_set / 1_000_000);
                println!("Total Go Stake:   {} ADA", total_go / 1_000_000);
                println!("Pools: {}", pools.len());

                // Filter by pool ID if provided
                let filtered: Vec<_> = if let Some(ref pool_id) = stake_pool_id {
                    pools
                        .iter()
                        .filter(|(id, _, _, _)| id.contains(pool_id))
                        .collect()
                } else {
                    pools.iter().collect()
                };

                if !filtered.is_empty() {
                    println!(
                        "\n{:<58} {:>16} {:>16} {:>16}",
                        "Pool ID", "Mark (ADA)", "Set (ADA)", "Go (ADA)"
                    );
                    println!("{}", "-".repeat(108));
                    for (id, mark, set, go) in &filtered {
                        println!(
                            "{id:<58} {:>16} {:>16} {:>16}",
                            mark / 1_000_000,
                            set / 1_000_000,
                            go / 1_000_000
                        );
                    }
                }
                Ok(())
            }
            QuerySubcommand::PoolParams {
                socket_path,
                testnet_magic,
                stake_pool_id,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let result = client
                    .query_pool_params()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query pool params: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, array[map{...}]]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }

                let arr_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);

                struct PoolInfo {
                    pool_id: String,
                    vrf_keyhash: String,
                    pledge: u64,
                    cost: u64,
                    margin_num: u64,
                    margin_den: u64,
                    relays: Vec<String>,
                    reward_account: String,
                    owners: Vec<String>,
                    metadata_url: Option<String>,
                    metadata_hash: Option<String>,
                }

                let mut pools: Vec<PoolInfo> = Vec::new();

                for _ in 0..arr_len {
                    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                    let mut info = PoolInfo {
                        pool_id: String::new(),
                        vrf_keyhash: String::new(),
                        pledge: 0,
                        cost: 0,
                        margin_num: 0,
                        margin_den: 1,
                        relays: Vec::new(),
                        reward_account: String::new(),
                        owners: Vec::new(),
                        metadata_url: None,
                        metadata_hash: None,
                    };
                    for _ in 0..map_len {
                        let key = decoder.str().unwrap_or("");
                        match key {
                            "pool_id" => info.pool_id = hex::encode(decoder.bytes().unwrap_or(&[])),
                            "vrf_keyhash" => {
                                info.vrf_keyhash = hex::encode(decoder.bytes().unwrap_or(&[]))
                            }
                            "pledge" => info.pledge = decoder.u64().unwrap_or(0),
                            "cost" => info.cost = decoder.u64().unwrap_or(0),
                            "margin_num" => info.margin_num = decoder.u64().unwrap_or(0),
                            "margin_den" => info.margin_den = decoder.u64().unwrap_or(1),
                            "relays" => {
                                let relay_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                                for _ in 0..relay_len {
                                    info.relays.push(decoder.str().unwrap_or("").to_string());
                                }
                            }
                            "reward_account" => {
                                info.reward_account = hex::encode(decoder.bytes().unwrap_or(&[]))
                            }
                            "owners" => {
                                let owner_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                                for _ in 0..owner_len {
                                    info.owners
                                        .push(hex::encode(decoder.bytes().unwrap_or(&[])));
                                }
                            }
                            "metadata" => {
                                let mmap_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                for _ in 0..mmap_len {
                                    let mkey = decoder.str().unwrap_or("");
                                    match mkey {
                                        "url" => {
                                            info.metadata_url =
                                                Some(decoder.str().unwrap_or("").to_string())
                                        }
                                        "hash" => {
                                            info.metadata_hash =
                                                decoder.bytes().ok().map(hex::encode);
                                        }
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                            }
                            _ => {
                                decoder.skip().ok();
                            }
                        }
                    }
                    pools.push(info);
                }

                // Filter by pool ID if provided
                let filtered: Vec<_> = if let Some(ref pid) = stake_pool_id {
                    pools.iter().filter(|p| p.pool_id.contains(pid)).collect()
                } else {
                    pools.iter().collect()
                };

                println!("Pool Parameters");
                println!("===============");
                println!("Total Pools: {}", pools.len());

                for pool in &filtered {
                    println!("\nPool ID:       {}", pool.pool_id);
                    println!("VRF Key:       {}", pool.vrf_keyhash);
                    println!("Pledge:        {} ADA", pool.pledge / 1_000_000);
                    println!("Cost:          {} ADA", pool.cost / 1_000_000);
                    if pool.margin_den > 0 {
                        let margin_pct = (pool.margin_num as f64 / pool.margin_den as f64) * 100.0;
                        println!("Margin:        {margin_pct:.2}%");
                    }
                    if !pool.reward_account.is_empty() {
                        println!("Reward Acct:   {}", pool.reward_account);
                    }
                    if !pool.owners.is_empty() {
                        println!("Owners:        {}", pool.owners.join(", "));
                    }
                    if !pool.relays.is_empty() {
                        println!("Relays:        {}", pool.relays.join(", "));
                    }
                    if let Some(url) = &pool.metadata_url {
                        println!("Metadata URL:  {url}");
                    }
                    if let Some(hash) = &pool.metadata_hash {
                        println!("Metadata Hash: {hash}");
                    }
                }
                Ok(())
            }
            QuerySubcommand::Treasury {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_account_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query account state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [treasury, reserves]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                let _ = decoder.array();
                let treasury = decoder.u64().unwrap_or(0);
                let reserves = decoder.u64().unwrap_or(0);

                println!("Account State");
                println!("=============");
                println!("Treasury: {} ADA", treasury / 1_000_000);
                println!("Reserves: {} ADA", reserves / 1_000_000);

                Ok(())
            }
            QuerySubcommand::LeadershipSchedule {
                vrf_signing_key_file,
                epoch_nonce,
                epoch_start_slot,
                epoch_length,
                relative_stake,
                active_slot_coeff,
            } => {
                // Load VRF signing key
                let vrf_content = std::fs::read_to_string(&vrf_signing_key_file)?;
                let vrf_env: serde_json::Value = serde_json::from_str(&vrf_content)?;
                let vrf_cbor_hex = vrf_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in VRF skey file"))?;
                let vrf_cbor = hex::decode(vrf_cbor_hex)?;
                // Strip CBOR wrapper
                let vrf_key_bytes = if vrf_cbor.len() > 2 && vrf_cbor[0] == 0x58 {
                    &vrf_cbor[2..]
                } else if vrf_cbor.len() > 1 && (vrf_cbor[0] & 0xe0) == 0x40 {
                    &vrf_cbor[1..]
                } else {
                    &vrf_cbor
                };
                if vrf_key_bytes.len() != 32 {
                    anyhow::bail!(
                        "VRF secret key must be 32 bytes, got {}",
                        vrf_key_bytes.len()
                    );
                }
                let mut vrf_skey = [0u8; 32];
                vrf_skey.copy_from_slice(vrf_key_bytes);

                // Parse epoch nonce
                let nonce = torsten_primitives::hash::Hash32::from_hex(&epoch_nonce)
                    .map_err(|e| anyhow::anyhow!("Invalid epoch nonce hex: {e}"))?;

                println!(
                    "Computing leader schedule for epoch starting at slot {epoch_start_slot}..."
                );
                println!("Epoch length: {epoch_length} slots");
                println!("Relative stake: {relative_stake:.6}");
                println!("Active slot coefficient: {active_slot_coeff}");
                println!();

                let schedule = torsten_consensus::compute_leader_schedule(
                    &vrf_skey,
                    &nonce,
                    epoch_start_slot,
                    epoch_length,
                    relative_stake,
                    active_slot_coeff,
                );

                if schedule.is_empty() {
                    println!("No leader slots found for this epoch.");
                } else {
                    println!("{:<12} VRF Output (first 16 bytes)", "SlotNo");
                    println!("{}", "-".repeat(50));
                    for leader in &schedule {
                        println!(
                            "{:<12} {}",
                            leader.slot.0,
                            hex::encode(&leader.vrf_output[..16])
                        );
                    }
                    println!("\nTotal leader slots: {}", schedule.len());
                    println!(
                        "Expected: ~{:.0} (f={active_slot_coeff}, stake={relative_stake:.6})",
                        epoch_length as f64
                            * (1.0 - (1.0 - active_slot_coeff).powf(relative_stake))
                    );
                }
                Ok(())
            }
        }
    }
}
