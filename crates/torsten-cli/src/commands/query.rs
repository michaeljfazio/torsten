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
    },
    /// Query UTxOs at an address
    Utxo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query protocol parameters
    ProtocolParameters {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
    },
    /// Query stake distribution
    StakeDistribution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query stake address info
    StakeAddressInfo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query governance state (Conway era)
    GovState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query DRep state (Conway era)
    DrepState {
        #[arg(long)]
        drep_key_hash: Option<String>,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
    },
    /// Query committee state (Conway era)
    CommitteeState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
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
async fn connect_and_acquire(
    socket_path: &std::path::Path,
) -> Result<torsten_network::N2CClient> {
    let mut client = torsten_network::N2CClient::connect(socket_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Cannot connect to node socket '{}': {e}\nIs the node running?",
                socket_path.display()
            )
        })?;

    client
        .handshake(764824073)
        .await
        .map_err(|e| anyhow::anyhow!("Handshake failed: {e}"))?;

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
            QuerySubcommand::Tip { socket_path } => {
                let mut client = connect_and_acquire(&socket_path).await?;

                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;

                let epoch = client.query_epoch().await.unwrap_or(0);
                let era = client.query_era().await.unwrap_or(6);

                release_and_done(&mut client).await;

                let hash_hex = hex::encode(&tip.hash);
                let era_str = era_name(era);

                println!("{{");
                println!("    \"slot\": {},", tip.slot);
                println!("    \"hash\": \"{hash_hex}\",");
                println!("    \"block\": {},", tip.block_no);
                println!("    \"epoch\": {epoch},");
                println!("    \"era\": \"{era_str}\",");
                println!("    \"syncProgress\": \"100.00\"");
                println!("}}");
                Ok(())
            }
            QuerySubcommand::Utxo {
                address,
                socket_path: _,
            } => {
                println!("Querying UTxOs for {address}...");
                println!("(UTxO query not yet implemented - requires UTxO by address index)");
                Ok(())
            }
            QuerySubcommand::ProtocolParameters {
                socket_path,
                out_file,
            } => {
                let mut client = connect_and_acquire(&socket_path).await?;

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
            QuerySubcommand::StakeDistribution { socket_path } => {
                let mut client = connect_and_acquire(&socket_path).await?;

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
                socket_path: _,
            } => {
                println!("Querying stake address info for {address}...");
                println!("(Stake address info query not yet implemented)");
                Ok(())
            }
            QuerySubcommand::GovState { socket_path } => {
                let mut client = connect_and_acquire(&socket_path).await?;

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
                            let arr_len =
                                decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                let pmap_len =
                                    decoder.map().unwrap_or(Some(0)).unwrap_or(0);
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
                                            tx_id = hex::encode(
                                                decoder.bytes().unwrap_or(&[]),
                                            )
                                        }
                                        "action_index" => {
                                            action_idx = decoder.u32().unwrap_or(0)
                                        }
                                        "action_type" => {
                                            action_type =
                                                decoder.str().unwrap_or("").to_string()
                                        }
                                        "yes_votes" => yes = decoder.u64().unwrap_or(0),
                                        "no_votes" => no = decoder.u64().unwrap_or(0),
                                        "abstain_votes" => {
                                            abstain = decoder.u64().unwrap_or(0)
                                        }
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                proposals.push((
                                    tx_id,
                                    action_idx,
                                    action_type,
                                    yes,
                                    no,
                                    abstain,
                                ));
                            }
                        }
                        _ => {
                            decoder.skip().ok();
                        }
                    }
                }

                println!("Governance State (Conway)");
                println!("========================");
                println!(
                    "Treasury:         {} ADA",
                    treasury / 1_000_000
                );
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
                        println!(
                            "{action_type:<20} {short_tx:<8} {yes:>6} {no:>6} {abstain:>8}"
                        );
                    }
                }

                Ok(())
            }
            QuerySubcommand::DrepState {
                drep_key_hash,
                socket_path,
            } => {
                let mut client = connect_and_acquire(&socket_path).await?;

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
                            "credential" => {
                                cred = hex::encode(decoder.bytes().unwrap_or(&[]))
                            }
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
            QuerySubcommand::CommitteeState { socket_path } => {
                let mut client = connect_and_acquire(&socket_path).await?;

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
                            let arr_len =
                                decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                let mmap_len =
                                    decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                let mut cold = String::new();
                                let mut hot = String::new();
                                for _ in 0..mmap_len {
                                    let mkey = decoder.str().unwrap_or("");
                                    match mkey {
                                        "cold" => {
                                            cold = hex::encode(
                                                decoder.bytes().unwrap_or(&[]),
                                            )
                                        }
                                        "hot" => {
                                            hot = hex::encode(
                                                decoder.bytes().unwrap_or(&[]),
                                            )
                                        }
                                        _ => {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                members.push((cold, hot));
                            }
                        }
                        "resigned" => {
                            let arr_len =
                                decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..arr_len {
                                resigned.push(hex::encode(
                                    decoder.bytes().unwrap_or(&[]),
                                ));
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
                    println!(
                        "\n{:<66} {:<66}",
                        "Cold Credential", "Hot Credential"
                    );
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
        }
    }
}
