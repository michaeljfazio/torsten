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
    /// Query UTxOs at an address or by specific transaction input(s).
    ///
    /// Use --address to query all UTxOs at a bech32 address, or --tx-in to query
    /// one or more specific UTxOs by transaction input reference (tx_hash#index).
    /// Both flags may be repeated. If neither flag is provided, --address is required.
    Utxo {
        /// Bech32 address to query UTxOs for
        #[arg(long)]
        address: Option<String>,
        /// Specific UTxO input reference to query (format: tx_hash#index). May be repeated.
        #[arg(long = "tx-in", value_name = "TX_HASH#INDEX")]
        tx_in: Vec<String>,
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
    /// Query the current constitution
    Constitution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query ratification state (enacted/expired proposals, delayed flag)
    RatifyState {
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
/// Parse a simple ISO-8601 UTC timestamp to Unix seconds.
/// Supports "YYYY-MM-DDThh:mm:ssZ" format.
fn parse_iso8601_to_unix(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u64 = parts[0].parse().ok()?;
    let month: u64 = parts[1].parse().ok()?;
    let day: u64 = parts[2].parse().ok()?;

    let time_parts: Vec<&str> = time.split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hours: u64 = time_parts[0].parse().ok()?;
    let minutes: u64 = time_parts[1].parse().ok()?;
    let seconds: u64 = time_parts[2].parse().ok()?;

    // Days from year 1970 to start of given year
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    // Days from start of year to start of given month
    let is_leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_months: [u64; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for &d in &days_in_months[..(month - 1) as usize] {
        days += d;
    }
    days += day - 1;

    Some(days * 86400 + hours * 3600 + minutes * 60 + seconds)
}

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

/// Parse and print a UTxO query response in the standard tabular format.
///
/// The `raw` bytes are a full LocalStateQuery MsgResult payload:
///   `[4, [Map<[tx_hash, index], TransactionOutput>]]`
/// where the inner array(1) is the HFC success wrapper.
///
/// Matches cardano-cli output: columns TxHash#Ix, Datum, Lovelace.
fn print_utxo_result(raw: &[u8]) -> Result<()> {
    // Parse MsgResult [4, array[map{...}]]
    let mut decoder = minicbor::Decoder::new(raw);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        anyhow::bail!("Expected MsgResult(4), got {tag}");
    }

    // Strip HFC success wrapper: array(1) around the actual map
    let pos = decoder.position();
    if let Ok(Some(1)) = decoder.array() {
        // HFC wrapper consumed
    } else {
        decoder.set_position(pos);
    }

    // UTxO result: CBOR Map<[tx_hash, index], TransactionOutput>
    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

    println!("{:<68} {:>6} {:>20}", "TxHash#Ix", "Datum", "Lovelace");
    println!("{}", "-".repeat(96));

    for _ in 0..map_len {
        // Key: [tx_hash_bytes, output_index]
        let _ = decoder.array(); // consume array(2)
        let tx_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
        let output_index = decoder.u32().unwrap_or(0);

        // Value: PostAlonzo TransactionOutput as CBOR map {0: addr, 1: value, 2: datum, 3: script_ref}
        let mut lovelace = 0u64;
        let mut has_datum = false;
        let output_map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
        for _ in 0..output_map_len {
            let key = decoder.u32().unwrap_or(999);
            match key {
                0 => {
                    // address bytes — skip
                    decoder.skip().ok();
                }
                1 => {
                    // value: either integer (ADA-only) or [coin, multiasset_map]
                    let val_pos = decoder.position();
                    if let Ok(coin) = decoder.u64() {
                        lovelace = coin;
                    } else {
                        decoder.set_position(val_pos);
                        if let Ok(Some(_)) = decoder.array() {
                            lovelace = decoder.u64().unwrap_or(0);
                            decoder.skip().ok(); // skip multiasset map
                        }
                    }
                }
                2 => {
                    // datum option
                    has_datum = true;
                    decoder.skip().ok();
                }
                3 => {
                    // script_ref
                    decoder.skip().ok();
                }
                _ => {
                    decoder.skip().ok();
                }
            }
        }

        let utxo_ref = format!("{tx_hash}#{output_index}");
        let datum_str = if has_datum { "yes" } else { "no" };
        println!("{utxo_ref:<68} {datum_str:>6} {lovelace:>20}");
    }

    println!("\nTotal UTxOs: {map_len}");
    Ok(())
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

                // Query system start time from the node for accurate sync progress
                let system_start_str = client.query_system_start().await.ok();

                release_and_done(&mut client).await;

                let hash_hex = hex::encode(&tip.hash);
                let era_str = era_name(era);

                // Calculate sync progress using the node's system start time
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let genesis_unix = system_start_str
                    .as_deref()
                    .and_then(parse_iso8601_to_unix)
                    .unwrap_or(match testnet_magic {
                        Some(2) => 1_666_656_000u64, // Preview
                        Some(1) => 1_654_041_600u64, // Preprod
                        _ => 1_596_059_091u64,       // Mainnet Shelley
                    });
                let elapsed_secs = now_secs.saturating_sub(genesis_unix);
                let expected_tip_slot = elapsed_secs; // 1 slot ≈ 1 second (Shelley+)
                let sync_progress = if expected_tip_slot > 0 {
                    (tip.slot as f64 / expected_tip_slot as f64 * 100.0).min(100.0)
                } else {
                    100.0
                };

                let network_name = torsten_primitives::network::NetworkId::name_from_magic(
                    testnet_magic.unwrap_or(764824073),
                );
                println!("{{");
                println!("    \"slot\": {},", tip.slot);
                println!("    \"hash\": \"{hash_hex}\",");
                println!("    \"block\": {block_no},");
                println!("    \"epoch\": {epoch},");
                println!("    \"era\": \"{era_str}\",");
                println!("    \"syncProgress\": \"{sync_progress:.2}\",");
                println!("    \"network\": \"{network_name}\"");
                println!("}}");
                Ok(())
            }
            QuerySubcommand::Utxo {
                address,
                tx_in,
                socket_path,
                testnet_magic,
            } => {
                // Validate: must have at least --address or --tx-in
                if address.is_none() && tx_in.is_empty() {
                    anyhow::bail!(
                        "At least one of --address or --tx-in must be provided.\n\
                         Examples:\n  \
                         --address addr1...\n  \
                         --tx-in <txhash>#<index>"
                    );
                }

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Collect all UTxO responses into a single raw CBOR payload.
                // For --tx-in queries we issue a single GetUTxOByTxIn (tag 15) with all inputs;
                // for --address we issue GetUTxOByAddress (tag 6).
                let raw: Vec<u8> = if !tx_in.is_empty() {
                    // Parse each "tx_hash#index" argument
                    let mut inputs: Vec<(Vec<u8>, u32)> = Vec::new();
                    for arg in &tx_in {
                        let (hash_str, idx_str) = arg.split_once('#').ok_or_else(|| {
                            anyhow::anyhow!(
                                "Invalid --tx-in format '{arg}': expected tx_hash#index"
                            )
                        })?;
                        let hash_bytes = hex::decode(hash_str)
                            .map_err(|e| anyhow::anyhow!("Invalid tx hash in '{arg}': {e}"))?;
                        if hash_bytes.len() != 32 {
                            anyhow::bail!(
                                "Tx hash must be 32 bytes (64 hex chars), got {} bytes in '{arg}'",
                                hash_bytes.len()
                            );
                        }
                        let index: u32 = idx_str
                            .parse()
                            .map_err(|e| anyhow::anyhow!("Invalid output index in '{arg}': {e}"))?;
                        inputs.push((hash_bytes, index));
                    }
                    client
                        .query_utxo_by_txin(&inputs)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to query UTxOs by tx-in: {e}"))?
                } else {
                    // --address mode
                    let addr = address.as_deref().unwrap();
                    let (_, addr_bytes) = bech32::decode(addr)
                        .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;
                    client
                        .query_utxo_by_address(&addr_bytes)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to query UTxOs: {e}"))?
                };

                release_and_done(&mut client).await;

                print_utxo_result(&raw)?;
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

                // Parse MsgResult [4, [map{pool_id => [tag(30)[num,den], vrf_hash]}]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                println!("{:<60} {:>18}", "PoolId", "Stake Fraction");
                println!("{}", "-".repeat(80));

                for _ in 0..map_len {
                    let pool_id = decoder.bytes().unwrap_or(&[]);
                    let pool_hex = hex::encode(pool_id);
                    let _ = decoder.array(); // IndividualPoolStake array(2)
                                             // tag(30) rational
                    let _ = decoder.tag();
                    let _ = decoder.array();
                    let num = decoder.u64().unwrap_or(0);
                    let den = decoder.u64().unwrap_or(1);
                    let fraction = num as f64 / den.max(1) as f64;
                    decoder.skip().ok(); // vrf_hash
                    println!("{pool_hex:<60} {fraction:>18.10}");
                }

                println!("\nTotal pools: {map_len}");
                Ok(())
            }
            QuerySubcommand::StakeAddressInfo {
                address,
                socket_path,
                testnet_magic,
            } => {
                // Decode bech32 stake address to extract the 28-byte staking credential hash.
                // A Shelley reward address has the format: [header(1)] [stake_cred(28)]
                // The header byte encodes network and credential type; the credential starts at byte 1.
                let (_, addr_bytes) = bech32::decode(&address)
                    .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;

                // Extract 28-byte credential hash (bytes 1..29 of the decoded address)
                let credential_bytes: &[u8] = if addr_bytes.len() >= 29 {
                    &addr_bytes[1..29]
                } else {
                    &addr_bytes
                };

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Pass the credential to the node so it filters server-side.
                // This avoids fetching the entire stake address table over the socket.
                let raw = client
                    .query_stake_address_info(credential_bytes)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake address info: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(2) [delegations_map, rewards_map]]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }
                // Strip HFC success wrapper: array(1)
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // HFC wrapper consumed
                } else {
                    decoder.set_position(pos);
                }

                // Result: array(2) [delegations_map, rewards_map]
                // delegations_map: Map<Credential, KeyHash StakePool>
                // rewards_map:     Map<Credential, Coin>
                let _ = decoder.array(); // array(2)

                // Parse delegations map: Map<Credential, pool_hash>
                let deleg_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut delegations: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for _ in 0..deleg_len {
                    // Credential: [0|1, hash(28)]
                    let _ = decoder.array();
                    let _ = decoder.u32(); // credential type (0=key, 1=script)
                    let cred = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let pool = hex::encode(decoder.bytes().unwrap_or(&[]));
                    delegations.insert(cred, pool);
                }

                // Parse rewards map: Map<Credential, Coin>
                // Since we filtered server-side the result should contain only the queried address,
                // but we iterate all entries for robustness.
                let rewards_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                // Collect entries so we can emit well-formed JSON (correct trailing comma)
                let mut entries: Vec<(String, u64)> = Vec::new();
                for _ in 0..rewards_len {
                    let _ = decoder.array();
                    let _ = decoder.u32(); // credential type
                    let cred = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let reward_balance = decoder.u64().unwrap_or(0);
                    entries.push((cred, reward_balance));
                }

                // Output JSON array matching cardano-cli format:
                // [{"address": "stake1...", "delegation": "pool1...", "rewardAccountBalance": N}]
                println!("[");
                let last = entries.len().saturating_sub(1);
                for (i, (cred, reward_balance)) in entries.iter().enumerate() {
                    let pool = delegations.get(cred).cloned().unwrap_or_default();
                    let comma = if i < last { "," } else { "" };
                    println!("  {{");
                    println!("    \"address\": \"{address}\",");
                    if pool.is_empty() {
                        println!("    \"delegation\": null,");
                    } else {
                        println!("    \"delegation\": \"{pool}\",");
                    }
                    println!("    \"rewardAccountBalance\": {reward_balance}");
                    println!("  }}{comma}");
                }
                if entries.is_empty() {
                    // Address not registered — cardano-cli returns empty array
                    // with a single entry showing zero balance and no delegation.
                    println!("  {{");
                    println!("    \"address\": \"{address}\",");
                    println!("    \"delegation\": null,");
                    println!("    \"rewardAccountBalance\": 0");
                    println!("  }}");
                }
                println!("]");

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

                // Parse MsgResult [4, [array(7)]] — ConwayGovState with HFC wrapper
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                // Strip HFC success wrapper array(1)
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                let mut proposals = Vec::new();
                let mut committee_count = 0usize;
                let mut constitution_url = String::new();

                // ConwayGovState = array(7)
                if let Ok(Some(7)) = decoder.array() {
                    // [0] Proposals = array(2) [roots, values]
                    if let Ok(Some(2)) = decoder.array() {
                        // roots: array(4) of StrictMaybe
                        decoder.skip().ok(); // skip roots

                        // values: array(n) of GovActionState
                        if let Ok(Some(n)) = decoder.array() {
                            for _ in 0..n {
                                // GovActionState = array(7)
                                if let Ok(Some(7)) = decoder.array() {
                                    // [0] GovActionId = array(2) [tx_hash, index]
                                    let _ = decoder.array();
                                    let tx_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                                    let action_idx = decoder.u32().unwrap_or(0);
                                    // [1] committeeVotes, [2] drepVotes, [3] spoVotes
                                    decoder.skip().ok();
                                    decoder.skip().ok();
                                    decoder.skip().ok();
                                    // [4] ProposalProcedure = array(4)
                                    let mut action_type = String::new();
                                    let mut deposit = 0u64;
                                    if let Ok(Some(4)) = decoder.array() {
                                        deposit = decoder.u64().unwrap_or(0);
                                        decoder.skip().ok(); // return_addr
                                                             // gov_action = sum type
                                        if let Ok(Some(_)) = decoder.array() {
                                            let gov_tag = decoder.u32().unwrap_or(6);
                                            action_type = match gov_tag {
                                                0 => "ParameterChange",
                                                1 => "HardForkInitiation",
                                                2 => "TreasuryWithdrawals",
                                                3 => "NoConfidence",
                                                4 => "UpdateCommittee",
                                                5 => "NewConstitution",
                                                _ => "InfoAction",
                                            }
                                            .to_string();
                                            // Skip remaining gov_action fields based on tag
                                            let skip_count = match gov_tag {
                                                0 => 3, // ParameterChange: prev, params, policy
                                                1 => 2, // HardFork: prev, version
                                                2 => 2, // Treasury: map, policy
                                                3 => 1, // NoConfidence: prev
                                                4 => 4, // UpdateCommittee: prev, remove, add, quorum
                                                5 => 2, // NewConstitution: prev, constitution
                                                _ => 0, // InfoAction: no fields
                                            };
                                            for _ in 0..skip_count {
                                                decoder.skip().ok();
                                            }
                                        }
                                        // anchor = array(2) [url, hash]
                                        decoder.skip().ok();
                                    }
                                    // [5] proposedIn, [6] expiresAfter
                                    let proposed = decoder.u64().unwrap_or(0);
                                    let expires = decoder.u64().unwrap_or(0);
                                    proposals.push((
                                        tx_id,
                                        action_idx,
                                        action_type,
                                        deposit,
                                        proposed,
                                        expires,
                                    ));
                                } else {
                                    decoder.skip().ok();
                                }
                            }
                        }
                    } else {
                        decoder.skip().ok();
                    }

                    // [1] Committee
                    if let Ok(Some(len)) = decoder.array() {
                        if len == 1 {
                            // StrictMaybe Just — array(2) [Map<Cred,Epoch>, threshold]
                            if let Ok(Some(2)) = decoder.array() {
                                let members = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                committee_count = members as usize;
                                for _ in 0..members {
                                    decoder.skip().ok(); // credential
                                    decoder.skip().ok(); // epoch
                                }
                                decoder.skip().ok(); // threshold
                            }
                        }
                        // else: Nothing (0), no committee
                    }

                    // [2] Constitution = array(2) [Anchor, MaybeScriptHash]
                    if let Ok(Some(2)) = decoder.array() {
                        // Anchor = array(2) [url, hash]
                        if let Ok(Some(2)) = decoder.array() {
                            constitution_url = decoder.str().unwrap_or("").to_string();
                            decoder.skip().ok(); // hash
                        }
                        decoder.skip().ok(); // script hash
                    }

                    // [3]-[6] curPParams, prevPParams, FuturePParams, DRepPulsingState
                    // We don't display these, just note they exist
                }

                println!("Governance State (Conway)");
                println!("========================");
                println!("Committee Members: {committee_count}");
                if !constitution_url.is_empty() {
                    println!("Constitution:     {constitution_url}");
                }
                println!("Active Proposals: {}", proposals.len());

                if !proposals.is_empty() {
                    println!("\nProposals:");
                    println!(
                        "{:<20} {:<14} {:>15} {:>8} {:>8}",
                        "Type", "TxId", "Deposit (ADA)", "Proposed", "Expires"
                    );
                    println!("{}", "-".repeat(68));
                    for (tx_id, idx, action_type, deposit, proposed, expires) in &proposals {
                        let short_tx = if tx_id.len() > 8 {
                            format!("{}#{idx}", &tx_id[..8])
                        } else {
                            format!("{tx_id}#{idx}")
                        };
                        println!(
                            "{action_type:<20} {short_tx:<14} {:>15} {:>8} {:>8}",
                            deposit / 1_000_000,
                            proposed,
                            expires
                        );
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

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // Parse Map<Credential, DRepState>
                // Credential: [type, hash(28)], DRepState: array(4) [expiry, maybe_anchor, deposit, delegators]
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                // Each entry: (cip0129_id, hex_hash, deposit, anchor_url, expiry_epoch)
                let mut dreps: Vec<(String, String, u64, String, u64)> = Vec::new();

                for _ in 0..map_len {
                    // Key: Credential array(2) [type, hash]
                    let _ = decoder.array();
                    // Credential type: 0 = key hash, 1 = script hash
                    let cred_type = decoder.u8().unwrap_or(0);
                    let hash_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                    let hex_hash = hex::encode(&hash_bytes);

                    // Encode as CIP-0129 bech32 identifier (drep1 / drep_script1)
                    let cip0129_id =
                        torsten_primitives::encode_drep_from_cbor(cred_type, &hash_bytes)
                            .unwrap_or_else(|_| hex_hash.clone());

                    // Value: DRepState array(4)
                    let _ = decoder.array();
                    let expiry_epoch = decoder.u64().unwrap_or(0);
                    // maybe_anchor: array(0)=None, array(1)=[anchor]
                    let anchor_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut anchor = String::new();
                    if anchor_len == 1 {
                        let _ = decoder.array(); // Anchor [url, hash]
                        anchor = decoder.str().map(|s| s.to_string()).unwrap_or_default();
                        decoder.skip().ok(); // skip hash
                    }
                    let deposit = decoder.u64().unwrap_or(0);
                    decoder.skip().ok(); // skip delegators set

                    dreps.push((cip0129_id, hex_hash, deposit, anchor, expiry_epoch));
                }

                // Filter by key hash if provided — match against either the hex hash or the bech32 id
                let filtered: Vec<_> = if let Some(ref hash) = drep_key_hash {
                    dreps
                        .iter()
                        .filter(|(id, hex, _, _, _)| hex.contains(hash) || id.contains(hash))
                        .collect()
                } else {
                    dreps.iter().collect()
                };

                println!("DRep State (Conway)");
                println!("===================");
                println!("Total DReps: {}", dreps.len());

                if !filtered.is_empty() {
                    // CIP-0129 identifiers are at most ~63 chars; use 66-char column.
                    println!(
                        "\n{:<66} {:>16} {:>8}",
                        "DRep ID (CIP-0129)", "Deposit (ADA)", "Epoch"
                    );
                    println!("{}", "-".repeat(92));
                    for (cip0129_id, _hex, deposit, anchor, epoch) in &filtered {
                        let deposit_ada = *deposit / 1_000_000;
                        println!("{cip0129_id:<66} {deposit_ada:>16} {epoch:>8}");
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

                // Parse MsgResult [4, [array(3) [map_members, maybe_threshold, epoch]]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // CommitteeMembersState array(3)
                let _ = decoder.array();

                // [0] Map<ColdCredential, CommitteeMemberState>
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                // Each entry: (cold_cip0129, hot_cip0129, status_tag)
                let mut members: Vec<(String, String, u32)> = Vec::new();
                let mut resigned_count = 0u64;

                for _ in 0..map_len {
                    // Key: ColdCredential array(2) [type, hash]
                    let _ = decoder.array();
                    let cold_type = decoder.u8().unwrap_or(0);
                    let cold_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                    // Encode with CIP-0129 cc_cold1 / cc_cold_script1
                    let cold = torsten_primitives::encode_cc_cold_from_cbor(cold_type, &cold_bytes)
                        .unwrap_or_else(|_| hex::encode(&cold_bytes));

                    // Value: CommitteeMemberState array(4)
                    let _ = decoder.array();
                    // [0] HotCredAuthStatus
                    let status_arr = decoder.array().unwrap_or(Some(1)).unwrap_or(1);
                    let status_tag = decoder.u32().unwrap_or(1);
                    let mut hot = String::new();
                    if status_tag == 0 && status_arr >= 2 {
                        // MemberAuthorized: [0, hot_credential]
                        let _ = decoder.array(); // [type, hash]
                        let hot_type = decoder.u8().unwrap_or(0);
                        let hot_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                        // Encode with CIP-0129 cc_hot1 / cc_hot_script1
                        hot = torsten_primitives::encode_cc_hot_from_cbor(hot_type, &hot_bytes)
                            .unwrap_or_else(|_| hex::encode(&hot_bytes));
                    } else if status_tag == 2 {
                        resigned_count += 1;
                        // MemberResigned: skip maybe_anchor
                        if status_arr >= 2 {
                            decoder.skip().ok();
                        }
                    }
                    // [1] MemberStatus
                    decoder.skip().ok();
                    // [2] Maybe EpochNo
                    decoder.skip().ok();
                    // [3] NextEpochChange
                    decoder.skip().ok();

                    members.push((cold, hot, status_tag));
                }

                // [1] Maybe threshold
                decoder.skip().ok();
                // [2] Current epoch
                let epoch = decoder.u64().unwrap_or(0);

                println!("Constitutional Committee State (Conway)");
                println!("=======================================");
                println!("Epoch: {epoch}");
                println!(
                    "Active Members: {}",
                    members.iter().filter(|(_, _, s)| *s == 0).count()
                );
                println!("Resigned Members: {resigned_count}");

                let authorized: Vec<_> = members.iter().filter(|(_, _, s)| *s == 0).collect();
                if !authorized.is_empty() {
                    // CIP-0129 identifiers: cc_cold1/cc_cold_script1 and cc_hot1/cc_hot_script1
                    println!(
                        "\n{:<65} {:<65}",
                        "Cold Credential (CIP-0129)", "Hot Credential (CIP-0129)"
                    );
                    println!("{}", "-".repeat(132));
                    for (cold, hot, _) in &authorized {
                        println!("{cold:<65} {hot:<65}");
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

                // Parse MsgResult [4, [Map<pool_hash, [ratio, vrf_hash]>]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }
                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                struct PoolInfo {
                    pool_id: String,
                    stake_num: u64,
                    stake_den: u64,
                }
                let mut pools = Vec::new();
                // Map<pool_hash(28), [tag(30)[num,den], vrf_hash(32)]>
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                for _ in 0..map_len {
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = decoder.array(); // array(2)
                                             // Stake ratio: tag(30)[num, den]
                    let _ = decoder.tag(); // tag(30)
                    let _ = decoder.array(); // [num, den]
                    let stake_num = decoder.u64().unwrap_or(0);
                    let stake_den = decoder.u64().unwrap_or(1);
                    // VRF hash
                    let _vrf = decoder.bytes().unwrap_or(&[]);
                    pools.push(PoolInfo {
                        pool_id,
                        stake_num,
                        stake_den,
                    });
                }

                println!("{:<58} {:>12}", "PoolId", "Stake");
                println!("{}", "-".repeat(72));
                for p in &pools {
                    let stake_frac = if p.stake_den > 0 {
                        p.stake_num as f64 / p.stake_den as f64
                    } else {
                        0.0
                    };
                    println!("{:<58} {:>11.6}%", p.pool_id, stake_frac * 100.0);
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

                // Parse MsgResult [4, [array(4) [pool_map, mark_total, set_total, go_total]]]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }
                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // array(4) [pool_map, mark_total, set_total, go_total]
                let _ = decoder.array(); // array(4)

                // pool_map: Map<pool_hash(28), array(3) [mark, set, go]>
                let pool_count = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut pools: Vec<(String, u64, u64, u64)> = Vec::new();
                for _ in 0..pool_count {
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = decoder.array(); // array(3)
                    let mark = decoder.u64().unwrap_or(0);
                    let set = decoder.u64().unwrap_or(0);
                    let go = decoder.u64().unwrap_or(0);
                    pools.push((pool_id, mark, set, go));
                }

                let total_mark = decoder.u64().unwrap_or(0);
                let total_set = decoder.u64().unwrap_or(0);
                let total_go = decoder.u64().unwrap_or(0);

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

                // Parse MsgResult [4, [Map<pool_hash(28), PoolParams_array(9)>]]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }
                // Strip HFC success wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // Consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

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

                for _ in 0..map_len {
                    // Key: pool hash
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    // Value: array(9) PoolParams
                    let _ = decoder.array(); // consume array(9)
                    let operator = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = operator; // same as pool_id
                    let vrf_keyhash = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let pledge = decoder.u64().unwrap_or(0);
                    let cost = decoder.u64().unwrap_or(0);
                    // margin: tagged rational tag(30)[num, den]
                    let (margin_num, margin_den) = {
                        let _ = decoder.tag(); // tag(30)
                        let _ = decoder.array(); // [num, den]
                        let n = decoder.u64().unwrap_or(0);
                        let d = decoder.u64().unwrap_or(1);
                        (n, d)
                    };
                    let reward_account = hex::encode(decoder.bytes().unwrap_or(&[]));
                    // owners: tag(258) set
                    let _ = decoder.tag(); // tag(258)
                    let owner_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut owners = Vec::new();
                    for _ in 0..owner_len {
                        owners.push(hex::encode(decoder.bytes().unwrap_or(&[])));
                    }
                    // relays: array of relay structures
                    let relay_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut relays = Vec::new();
                    for _ in 0..relay_len {
                        let _ = decoder.array(); // relay array
                        let relay_tag = decoder.u32().unwrap_or(99);
                        match relay_tag {
                            0 => {
                                // SingleHostAddr: port, ipv4, ipv6
                                let port = decoder.u16().ok();
                                if port.is_none() {
                                    decoder.skip().ok();
                                } // skip null
                                let ipv4 = decoder.bytes().ok().map(|b| {
                                    if b.len() == 4 {
                                        format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
                                    } else {
                                        hex::encode(b)
                                    }
                                });
                                if ipv4.is_none() {
                                    decoder.skip().ok();
                                } // skip null
                                decoder.skip().ok(); // skip ipv6
                                let addr = ipv4.unwrap_or_default();
                                relays.push(format!("{}:{}", addr, port.unwrap_or(0)));
                            }
                            1 => {
                                // SingleHostName: port, dns_name
                                let port = decoder.u16().ok();
                                if port.is_none() {
                                    decoder.skip().ok();
                                }
                                let dns = decoder.str().unwrap_or("").to_string();
                                relays.push(format!("{}:{}", dns, port.unwrap_or(0)));
                            }
                            2 => {
                                // MultiHostName: dns_name
                                relays.push(decoder.str().unwrap_or("").to_string());
                            }
                            _ => {
                                // Skip unknown relay type fields
                                decoder.skip().ok();
                            }
                        }
                    }
                    // metadata: nullable [url, hash]
                    let (metadata_url, metadata_hash) = {
                        let pos = decoder.position();
                        if let Ok(Some(2)) = decoder.array() {
                            let url = Some(decoder.str().unwrap_or("").to_string());
                            let hash = decoder.bytes().ok().map(hex::encode);
                            (url, hash)
                        } else {
                            decoder.set_position(pos);
                            decoder.skip().ok(); // skip null
                            (None, None)
                        }
                    };

                    pools.push(PoolInfo {
                        pool_id,
                        vrf_keyhash,
                        pledge,
                        cost,
                        margin_num,
                        margin_den,
                        relays,
                        reward_account,
                        owners,
                        metadata_url,
                        metadata_hash,
                    });
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

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
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
            QuerySubcommand::Constitution {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_constitution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query constitution: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(2) [Anchor, MaybeScriptHash]]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                // Constitution = array(2) [Anchor, MaybeScriptHash]
                let mut url = String::new();
                let mut data_hash = String::new();
                let mut script_hash = String::from("none");

                if let Ok(Some(2)) = decoder.array() {
                    // Anchor = array(2) [url, hash]
                    if let Ok(Some(2)) = decoder.array() {
                        url = decoder.str().unwrap_or("").to_string();
                        data_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                    }
                    // StrictMaybe ScriptHash
                    if let Ok(bytes) = decoder.bytes() {
                        script_hash = hex::encode(bytes);
                    }
                }

                println!("Constitution");
                println!("============");
                println!("URL:         {url}");
                println!("Data Hash:   {data_hash}");
                println!("Script Hash: {script_hash}");

                Ok(())
            }
            QuerySubcommand::RatifyState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_ratify_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query ratify state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, ratify_state]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult(4), got {tag}");
                }

                // Strip HFC wrapper array(1)
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                // RatifyState = array(4) [enacted_seq, expired_seq, delayed_bool, future_pparam_update]
                let mut enacted_count = 0u64;
                let mut expired_count = 0u64;
                let mut delayed = false;
                let mut enacted_ids = Vec::new();
                let mut expired_ids = Vec::new();

                if let Ok(Some(4)) = decoder.array() {
                    // enacted: array(n) of (GovActionState, GovActionId)
                    if let Ok(Some(n)) = decoder.array() {
                        enacted_count = n;
                        for _ in 0..n {
                            // array(2) [GovActionState, GovActionId]
                            if let Ok(Some(2)) = decoder.array() {
                                // Skip GovActionState (complex structure)
                                decoder.skip().ok();
                                // GovActionId = array(2) [tx_hash, index]
                                if let Ok(Some(2)) = decoder.array() {
                                    let tx_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                                    let index = decoder.u32().unwrap_or(0);
                                    enacted_ids.push(format!("{}#{}", tx_hash, index));
                                }
                            }
                        }
                    }

                    // expired: array(n) of GovActionId
                    if let Ok(Some(n)) = decoder.array() {
                        expired_count = n;
                        for _ in 0..n {
                            // GovActionId = array(2) [tx_hash, index]
                            if let Ok(Some(2)) = decoder.array() {
                                let tx_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                                let index = decoder.u32().unwrap_or(0);
                                expired_ids.push(format!("{}#{}", tx_hash, index));
                            }
                        }
                    }

                    // delayed flag
                    delayed = decoder.bool().unwrap_or(false);

                    // future_pparam_update: skip
                    decoder.skip().ok();
                }

                println!("Ratification State");
                println!("==================");
                println!("Enacted proposals: {enacted_count}");
                for id in &enacted_ids {
                    println!("  {id}");
                }
                println!("Expired proposals: {expired_count}");
                for id in &expired_ids {
                    println!("  {id}");
                }
                println!("Delayed:           {delayed}");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso8601_to_unix_epoch() {
        // 1970-01-01T00:00:00Z = Unix epoch
        assert_eq!(parse_iso8601_to_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn test_parse_iso8601_to_unix_known_date() {
        // 2022-11-24T00:00:00Z = Preview testnet genesis (approximately)
        let ts = parse_iso8601_to_unix("2022-11-24T00:00:00Z").unwrap();
        // 2022-11-24 is day 328 of year 2022
        // Expected: 1669248000
        assert_eq!(ts, 1669248000);
    }

    #[test]
    fn test_parse_iso8601_to_unix_with_time() {
        let ts = parse_iso8601_to_unix("2020-06-01T21:44:51Z").unwrap();
        // Manually computed
        assert!(ts > 1590000000 && ts < 1600000000);
        assert_eq!(ts, 1591047891);
    }

    #[test]
    fn test_parse_iso8601_leap_year() {
        // 2024 is a leap year — Feb 29 should be valid
        let feb28 = parse_iso8601_to_unix("2024-02-28T00:00:00Z").unwrap();
        let feb29 = parse_iso8601_to_unix("2024-02-29T00:00:00Z").unwrap();
        assert_eq!(
            feb29 - feb28,
            86400,
            "Feb 29 should be exactly 1 day after Feb 28"
        );
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_to_unix("not a date").is_none());
        assert!(parse_iso8601_to_unix("2022-01-01").is_none()); // no time
        assert!(parse_iso8601_to_unix("").is_none());
    }

    #[test]
    fn test_era_name_all() {
        assert_eq!(era_name(0), "Byron");
        assert_eq!(era_name(1), "Shelley");
        assert_eq!(era_name(2), "Allegra");
        assert_eq!(era_name(3), "Mary");
        assert_eq!(era_name(4), "Alonzo");
        assert_eq!(era_name(5), "Babbage");
        assert_eq!(era_name(6), "Conway");
        assert_eq!(era_name(7), "Unknown");
        assert_eq!(era_name(99), "Unknown");
    }
}
