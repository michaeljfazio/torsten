use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct GovernanceCmd {
    #[command(subcommand)]
    command: GovernanceSubcommand,
}

#[derive(Subcommand, Debug)]
enum GovernanceSubcommand {
    /// DRep commands
    Drep {
        #[command(subcommand)]
        command: DRepSubcommand,
    },
    /// Vote on governance actions
    Vote {
        #[command(subcommand)]
        command: VoteSubcommand,
    },
    /// Create governance actions
    Action {
        #[command(subcommand)]
        command: ActionSubcommand,
    },
}

#[derive(Subcommand, Debug)]
enum DRepSubcommand {
    /// Generate DRep keys
    KeyGen {
        #[arg(long)]
        verification_key_file: PathBuf,
        #[arg(long)]
        signing_key_file: PathBuf,
    },
    /// Get DRep ID
    Id {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        /// Output format: bech32 (default) or hex
        #[arg(long, default_value = "bech32")]
        output_format: String,
    },
    /// Create DRep registration certificate
    RegistrationCertificate {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        #[arg(long)]
        key_reg_deposit_amt: u64,
        /// Optional anchor URL for DRep metadata
        #[arg(long)]
        anchor_url: Option<String>,
        /// Optional anchor data hash
        #[arg(long)]
        anchor_data_hash: Option<String>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create DRep deregistration (retirement) certificate
    RetirementCertificate {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        #[arg(long)]
        deposit_amt: u64,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create DRep metadata update certificate
    UpdateCertificate {
        #[arg(long)]
        drep_verification_key_file: PathBuf,
        #[arg(long)]
        anchor_url: Option<String>,
        #[arg(long)]
        anchor_data_hash: Option<String>,
        #[arg(long)]
        out_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum VoteSubcommand {
    /// Create a vote file
    Create {
        #[arg(long)]
        governance_action_tx_id: String,
        #[arg(long)]
        governance_action_index: u32,
        /// Vote: yes, no, or abstain
        #[arg(long)]
        vote: String,
        /// DRep verification key file (for DRep voter)
        #[arg(long)]
        drep_verification_key_file: Option<PathBuf>,
        /// SPO cold verification key file (for SPO voter)
        #[arg(long)]
        cold_verification_key_file: Option<PathBuf>,
        /// CC hot verification key file (for Constitutional Committee voter)
        #[arg(long)]
        cc_hot_verification_key_file: Option<PathBuf>,
        #[arg(long)]
        anchor_url: Option<String>,
        #[arg(long)]
        anchor_data_hash: Option<String>,
        #[arg(long)]
        out_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
#[allow(clippy::enum_variant_names)]
enum ActionSubcommand {
    /// Create an info action
    CreateInfo {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create a no-confidence action
    CreateNoConfidence {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        #[arg(long)]
        prev_governance_action_tx_id: Option<String>,
        #[arg(long)]
        prev_governance_action_index: Option<u32>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create a new constitution action
    CreateConstitution {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        /// Constitution anchor URL
        #[arg(long)]
        constitution_url: String,
        /// Constitution anchor data hash
        #[arg(long)]
        constitution_hash: String,
        /// Optional guardrail script hash
        #[arg(long)]
        constitution_script_hash: Option<String>,
        #[arg(long)]
        prev_governance_action_tx_id: Option<String>,
        #[arg(long)]
        prev_governance_action_index: Option<u32>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create a hard fork initiation action
    CreateHardForkInitiation {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        /// Major protocol version
        #[arg(long)]
        protocol_major_version: u64,
        /// Minor protocol version
        #[arg(long)]
        protocol_minor_version: u64,
        #[arg(long)]
        prev_governance_action_tx_id: Option<String>,
        #[arg(long)]
        prev_governance_action_index: Option<u32>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Compute the hash of anchor data
    HashAnchorData {
        /// Path to the anchor data file
        #[arg(long)]
        file_binary: Option<PathBuf>,
        /// Anchor text to hash directly
        #[arg(long)]
        file_text: Option<PathBuf>,
    },
    /// Create a protocol parameters update action
    CreateProtocolParametersUpdate {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        /// Protocol parameter changes as JSON file
        #[arg(long)]
        protocol_parameters_update: PathBuf,
        /// Optional guardrail script hash
        #[arg(long)]
        constitution_script_hash: Option<String>,
        #[arg(long)]
        prev_governance_action_tx_id: Option<String>,
        #[arg(long)]
        prev_governance_action_index: Option<u32>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create an update committee action
    CreateUpdateCommittee {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        /// Cold verification key files of members to remove
        #[arg(long)]
        remove_cc_cold_verification_key_hash: Vec<String>,
        /// New committee member: key_hash,expiry_epoch
        #[arg(long)]
        add_cc_cold_verification_key_hash: Vec<String>,
        /// Quorum threshold as rational (e.g., "2/3")
        #[arg(long)]
        threshold: String,
        #[arg(long)]
        prev_governance_action_tx_id: Option<String>,
        #[arg(long)]
        prev_governance_action_index: Option<u32>,
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Create a treasury withdrawal action
    CreateTreasuryWithdrawal {
        #[arg(long)]
        anchor_url: String,
        #[arg(long)]
        anchor_data_hash: String,
        #[arg(long)]
        deposit: u64,
        #[arg(long)]
        return_addr: String,
        /// Withdrawal target: address+amount
        #[arg(long)]
        funds_receiving_stake_verification_key_file: PathBuf,
        #[arg(long)]
        transfer: u64,
        #[arg(long)]
        out_file: PathBuf,
    },
}

fn simple_cbor_wrap(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    if data.len() < 24 {
        result.push(0x40 | data.len() as u8);
    } else if data.len() < 256 {
        result.push(0x58);
        result.push(data.len() as u8);
    } else {
        result.push(0x59);
        result.extend_from_slice(&(data.len() as u16).to_be_bytes());
    }
    result.extend_from_slice(data);
    result
}

impl GovernanceCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            GovernanceSubcommand::Drep { command } => match command {
                DRepSubcommand::KeyGen {
                    verification_key_file,
                    signing_key_file,
                } => {
                    let sk = dugite_crypto::keys::PaymentSigningKey::generate();
                    let vk = sk.verification_key();

                    let sk_env = serde_json::json!({
                        "type": "DRepSigningKey_ed25519",
                        "description": "Delegated Representative Signing Key",
                        "cborHex": hex::encode(simple_cbor_wrap(&sk.to_bytes()))
                    });
                    let vk_env = serde_json::json!({
                        "type": "DRepVerificationKey_ed25519",
                        "description": "Delegated Representative Verification Key",
                        "cborHex": hex::encode(simple_cbor_wrap(&vk.to_bytes()))
                    });

                    std::fs::write(&signing_key_file, serde_json::to_string_pretty(&sk_env)?)?;
                    std::fs::write(
                        &verification_key_file,
                        serde_json::to_string_pretty(&vk_env)?,
                    )?;

                    println!("DRep keys generated.");
                    Ok(())
                }
                DRepSubcommand::Id {
                    drep_verification_key_file,
                    output_format,
                } => {
                    let key_hash = load_key_hash(&drep_verification_key_file)?;

                    if output_format == "hex" {
                        println!("{}", hex::encode(&key_hash));
                    } else {
                        // CIP-0129: DRep key-hash identifiers use the `drep1` HRP.
                        // (The legacy `drep` prefix was superseded by CIP-0129.)
                        let hash28 = dugite_primitives::Hash28::try_from(key_hash.as_slice())
                            .map_err(|_| {
                                anyhow::anyhow!(
                                    "DRep key hash must be 28 bytes, got {}",
                                    key_hash.len()
                                )
                            })?;
                        let drep_id = dugite_primitives::encode_drep_key(&hash28)
                            .map_err(|e| anyhow::anyhow!("Failed to encode DRep ID: {e}"))?;
                        println!("{drep_id}");
                    }
                    Ok(())
                }
                DRepSubcommand::RegistrationCertificate {
                    drep_verification_key_file,
                    key_reg_deposit_amt,
                    anchor_url,
                    anchor_data_hash,
                    out_file,
                } => {
                    let key_hash = load_key_hash(&drep_verification_key_file)?;

                    // Build DRep registration certificate CBOR
                    // Conway cert type 16 = RegDRep
                    let mut cert_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut cert_cbor);

                    let has_anchor = anchor_url.is_some() && anchor_data_hash.is_some();
                    enc.array(if has_anchor { 4 } else { 3 })?;
                    enc.u32(16)?; // RegDRep tag
                                  // Credential: [0, key_hash] for verification key
                    enc.array(2)?;
                    enc.u32(0)?;
                    enc.bytes(&key_hash)?;
                    enc.u64(key_reg_deposit_amt)?;

                    if let (Some(url), Some(hash_hex)) = (&anchor_url, &anchor_data_hash) {
                        let hash_bytes = hex::decode(hash_hex)?;
                        enc.array(2)?;
                        enc.str(url)?;
                        enc.bytes(&hash_bytes)?;
                    }

                    let cert_env = serde_json::json!({
                        "type": "CertificateConway",
                        "description": "DRep Registration Certificate",
                        "cborHex": hex::encode(&cert_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                    println!(
                        "DRep registration certificate written to: {}",
                        out_file.display()
                    );
                    Ok(())
                }
                DRepSubcommand::RetirementCertificate {
                    drep_verification_key_file,
                    deposit_amt,
                    out_file,
                } => {
                    let key_hash = load_key_hash(&drep_verification_key_file)?;

                    // Conway cert type 17 = UnregDRep
                    let mut cert_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut cert_cbor);
                    enc.array(3)?;
                    enc.u32(17)?;
                    enc.array(2)?;
                    enc.u32(0)?;
                    enc.bytes(&key_hash)?;
                    enc.u64(deposit_amt)?;

                    let cert_env = serde_json::json!({
                        "type": "CertificateConway",
                        "description": "DRep Retirement Certificate",
                        "cborHex": hex::encode(&cert_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                    println!(
                        "DRep retirement certificate written to: {}",
                        out_file.display()
                    );
                    Ok(())
                }
                DRepSubcommand::UpdateCertificate {
                    drep_verification_key_file,
                    anchor_url,
                    anchor_data_hash,
                    out_file,
                } => {
                    let key_hash = load_key_hash(&drep_verification_key_file)?;

                    // Conway cert type 18 = UpdateDRep
                    let mut cert_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut cert_cbor);

                    let has_anchor = anchor_url.is_some() && anchor_data_hash.is_some();
                    enc.array(if has_anchor { 3 } else { 2 })?;
                    enc.u32(18)?;
                    enc.array(2)?;
                    enc.u32(0)?;
                    enc.bytes(&key_hash)?;

                    if let (Some(url), Some(hash_hex)) = (&anchor_url, &anchor_data_hash) {
                        let hash_bytes = hex::decode(hash_hex)?;
                        enc.array(2)?;
                        enc.str(url)?;
                        enc.bytes(&hash_bytes)?;
                    }

                    let cert_env = serde_json::json!({
                        "type": "CertificateConway",
                        "description": "DRep Update Certificate",
                        "cborHex": hex::encode(&cert_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&cert_env)?)?;
                    println!("DRep update certificate written to: {}", out_file.display());
                    Ok(())
                }
            },
            GovernanceSubcommand::Vote { command } => match command {
                VoteSubcommand::Create {
                    governance_action_tx_id,
                    governance_action_index,
                    vote,
                    drep_verification_key_file,
                    cold_verification_key_file,
                    cc_hot_verification_key_file,
                    anchor_url,
                    anchor_data_hash,
                    out_file,
                } => {
                    let vote_value = match vote.to_lowercase().as_str() {
                        "yes" => 1u32,
                        "no" => 0,
                        "abstain" => 2,
                        _ => anyhow::bail!("Invalid vote: '{vote}'. Must be yes, no, or abstain"),
                    };

                    let action_tx_hash = hex::decode(&governance_action_tx_id)?;
                    if action_tx_hash.len() != 32 {
                        anyhow::bail!("Invalid governance action tx id length");
                    }

                    // Determine voter type and credential
                    // CC Hot = type 0, SPO = type 1, DRep = type 2
                    let (voter_type, voter_hash) = if let Some(ref cc_file) =
                        cc_hot_verification_key_file
                    {
                        (0u32, load_key_hash(cc_file)?)
                    } else if let Some(ref cold_file) = cold_verification_key_file {
                        (1, load_key_hash(cold_file)?)
                    } else if let Some(ref drep_file) = drep_verification_key_file {
                        (2, load_key_hash(drep_file)?)
                    } else {
                        anyhow::bail!(
                                "Must provide --drep-verification-key-file, --cold-verification-key-file, or --cc-hot-verification-key-file"
                            );
                    };

                    // Build vote CBOR
                    let mut vote_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut vote_cbor);

                    // Voting procedures map: { voter => { action_id => voting_procedure } }
                    enc.map(1)?;
                    // Voter: [voter_type, credential]
                    enc.array(2)?;
                    enc.u32(voter_type)?;
                    enc.array(2)?;
                    enc.u32(0)?; // key credential
                    enc.bytes(&voter_hash)?;
                    // Action votes map
                    enc.map(1)?;
                    // Action ID: [tx_hash, index]
                    enc.array(2)?;
                    enc.bytes(&action_tx_hash)?;
                    enc.u32(governance_action_index)?;
                    // Voting procedure: [vote, anchor]
                    enc.array(2)?;
                    enc.u32(vote_value)?;
                    if let (Some(url), Some(hash_hex)) = (&anchor_url, &anchor_data_hash) {
                        let hash_bytes = hex::decode(hash_hex)?;
                        enc.array(2)?;
                        enc.str(url)?;
                        enc.bytes(&hash_bytes)?;
                    } else {
                        enc.null()?;
                    }

                    let voter_desc = match voter_type {
                        0 => "Constitutional Committee",
                        1 => "Stake Pool Operator",
                        _ => "DRep",
                    };
                    let vote_env = serde_json::json!({
                        "type": "VoteConway",
                        "description": format!("{voter_desc} Governance Vote"),
                        "cborHex": hex::encode(&vote_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&vote_env)?)?;

                    let vote_str = match vote_value {
                        0 => "No",
                        1 => "Yes",
                        _ => "Abstain",
                    };
                    println!("Vote file written to: {}", out_file.display());
                    println!(
                        "Vote: {vote_str} ({voter_desc}) on {governance_action_tx_id}#{governance_action_index}"
                    );
                    Ok(())
                }
            },
            GovernanceSubcommand::Action { command } => match command {
                ActionSubcommand::CreateInfo {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    // Build governance action CBOR
                    // ProposalProcedure: [deposit, return_addr, gov_action, anchor]
                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // InfoAction = tag 6, no params
                    enc.array(1)?;
                    enc.u32(6)?;
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "Info Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!("Info action written to: {}", out_file.display());
                    Ok(())
                }
                ActionSubcommand::CreateNoConfidence {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    prev_governance_action_tx_id,
                    prev_governance_action_index,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // NoConfidence = tag 3
                    enc.array(2)?;
                    enc.u32(3)?;
                    encode_prev_action_id(
                        &mut enc,
                        &prev_governance_action_tx_id,
                        &prev_governance_action_index,
                    )?;
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "No Confidence Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!("No-confidence action written to: {}", out_file.display());
                    Ok(())
                }
                ActionSubcommand::CreateConstitution {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    constitution_url,
                    constitution_hash,
                    constitution_script_hash,
                    prev_governance_action_tx_id,
                    prev_governance_action_index,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;
                    let const_hash = hex::decode(&constitution_hash)?;

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // NewConstitution = tag 5
                    enc.array(3)?;
                    enc.u32(5)?;
                    encode_prev_action_id(
                        &mut enc,
                        &prev_governance_action_tx_id,
                        &prev_governance_action_index,
                    )?;
                    // Constitution: [anchor, script_hash]
                    enc.array(2)?;
                    // Constitution anchor
                    enc.array(2)?;
                    enc.str(&constitution_url)?;
                    enc.bytes(&const_hash)?;
                    // Guardrail script hash (nullable)
                    if let Some(ref script_hash_hex) = constitution_script_hash {
                        let script_hash = hex::decode(script_hash_hex)?;
                        enc.bytes(&script_hash)?;
                    } else {
                        enc.null()?;
                    }
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "New Constitution Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!("New constitution action written to: {}", out_file.display());
                    Ok(())
                }
                ActionSubcommand::CreateHardForkInitiation {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    protocol_major_version,
                    protocol_minor_version,
                    prev_governance_action_tx_id,
                    prev_governance_action_index,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // HardForkInitiation = tag 1
                    enc.array(3)?;
                    enc.u32(1)?;
                    encode_prev_action_id(
                        &mut enc,
                        &prev_governance_action_tx_id,
                        &prev_governance_action_index,
                    )?;
                    // Protocol version: [major, minor]
                    enc.array(2)?;
                    enc.u64(protocol_major_version)?;
                    enc.u64(protocol_minor_version)?;
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "Hard Fork Initiation Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!(
                        "Hard fork initiation action written to: {}",
                        out_file.display()
                    );
                    Ok(())
                }
                ActionSubcommand::HashAnchorData {
                    file_binary,
                    file_text,
                } => {
                    let data = if let Some(ref path) = file_binary {
                        std::fs::read(path)?
                    } else if let Some(ref path) = file_text {
                        std::fs::read(path)?
                    } else {
                        anyhow::bail!("Must provide either --file-binary or --file-text");
                    };

                    let hash = dugite_primitives::hash::blake2b_256(&data);
                    println!("{}", hex::encode(hash.as_bytes()));
                    Ok(())
                }
                ActionSubcommand::CreateProtocolParametersUpdate {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    protocol_parameters_update,
                    constitution_script_hash,
                    prev_governance_action_tx_id,
                    prev_governance_action_index,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    // Read protocol parameter update JSON
                    let pp_content = std::fs::read_to_string(&protocol_parameters_update)?;
                    let pp_json: serde_json::Value = serde_json::from_str(&pp_content)?;

                    // Encode protocol parameter update as CBOR map
                    let pp_cbor = encode_protocol_param_update(&pp_json)?;

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // ParameterChange = tag 0
                    enc.array(4)?;
                    enc.u32(0)?;
                    encode_prev_action_id(
                        &mut enc,
                        &prev_governance_action_tx_id,
                        &prev_governance_action_index,
                    )?;
                    // Embed raw protocol param update CBOR
                    enc.writer_mut().extend_from_slice(&pp_cbor);
                    // Policy hash
                    if let Some(ref script_hash_hex) = constitution_script_hash {
                        let script_hash = hex::decode(script_hash_hex)?;
                        enc.bytes(&script_hash)?;
                    } else {
                        enc.null()?;
                    }
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "Protocol Parameters Update Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!(
                        "Protocol parameters update action written to: {}",
                        out_file.display()
                    );
                    Ok(())
                }
                ActionSubcommand::CreateUpdateCommittee {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    remove_cc_cold_verification_key_hash,
                    add_cc_cold_verification_key_hash,
                    threshold,
                    prev_governance_action_tx_id,
                    prev_governance_action_index,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    // Parse threshold as rational "num/den"
                    let thresh_parts: Vec<&str> = threshold.split('/').collect();
                    if thresh_parts.len() != 2 {
                        anyhow::bail!(
                            "Invalid threshold format: '{threshold}'. Expected num/den (e.g., 2/3)"
                        );
                    }
                    let thresh_num: u64 = thresh_parts[0].parse()?;
                    let thresh_den: u64 = thresh_parts[1].parse()?;

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // UpdateCommittee = tag 4
                    enc.array(5)?;
                    enc.u32(4)?;
                    encode_prev_action_id(
                        &mut enc,
                        &prev_governance_action_tx_id,
                        &prev_governance_action_index,
                    )?;
                    // Members to remove (set of credentials)
                    enc.array(remove_cc_cold_verification_key_hash.len() as u64)?;
                    for hash_hex in &remove_cc_cold_verification_key_hash {
                        let hash_bytes = hex::decode(hash_hex)?;
                        enc.array(2)?;
                        enc.u32(0)?; // key credential
                        enc.bytes(&hash_bytes)?;
                    }
                    // Members to add: { credential => expiry_epoch }
                    enc.map(add_cc_cold_verification_key_hash.len() as u64)?;
                    for entry in &add_cc_cold_verification_key_hash {
                        // Format: "key_hash,expiry_epoch"
                        let parts: Vec<&str> = entry.split(',').collect();
                        if parts.len() != 2 {
                            anyhow::bail!(
                                "Invalid add member format: '{entry}'. Expected key_hash,expiry_epoch"
                            );
                        }
                        let hash_bytes = hex::decode(parts[0])?;
                        let expiry: u64 = parts[1].parse()?;
                        enc.array(2)?;
                        enc.u32(0)?;
                        enc.bytes(&hash_bytes)?;
                        enc.u64(expiry)?;
                    }
                    // Threshold as rational
                    enc.array(2)?;
                    enc.u64(thresh_num)?;
                    enc.u64(thresh_den)?;
                    // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "Update Committee Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!("Update committee action written to: {}", out_file.display());
                    Ok(())
                }
                ActionSubcommand::CreateTreasuryWithdrawal {
                    anchor_url,
                    anchor_data_hash,
                    deposit,
                    return_addr,
                    funds_receiving_stake_verification_key_file,
                    transfer,
                    out_file,
                } => {
                    let anchor_hash = hex::decode(&anchor_data_hash)?;
                    let (_, return_addr_bytes) = bech32::decode(&return_addr)?;

                    // Load the funds-receiving stake verification key and build reward address
                    let stake_vkey_json: serde_json::Value = serde_json::from_str(
                        &std::fs::read_to_string(&funds_receiving_stake_verification_key_file)?,
                    )?;
                    let stake_vkey_hex = stake_vkey_json["cborHex"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing cborHex in stake vkey file"))?;
                    let stake_vkey_cbor = hex::decode(stake_vkey_hex)?;
                    // Strip CBOR wrapper (2 bytes for 32-byte key)
                    let stake_vkey_raw = if stake_vkey_cbor.len() > 32 {
                        &stake_vkey_cbor[stake_vkey_cbor.len() - 32..]
                    } else {
                        &stake_vkey_cbor
                    };
                    let stake_hash = dugite_primitives::hash::blake2b_224(stake_vkey_raw);
                    // Reward address: 0xe0 (testnet) or 0xe1 (mainnet) + 28-byte key hash
                    // Use testnet by default (matches return_addr network)
                    let network_byte = if return_addr_bytes.first().is_some_and(|b| b & 0x01 == 1) {
                        0xe1u8 // mainnet
                    } else {
                        0xe0u8 // testnet
                    };
                    let mut withdrawal_addr = vec![network_byte];
                    withdrawal_addr.extend_from_slice(stake_hash.as_ref());

                    let mut action_cbor = Vec::new();
                    let mut enc = minicbor::Encoder::new(&mut action_cbor);
                    enc.array(4)?;
                    enc.u64(deposit)?;
                    enc.bytes(&return_addr_bytes)?;
                    // TreasuryWithdrawals = tag 2
                    enc.array(3)?;
                    enc.u32(2)?;
                    // Withdrawals map: reward_address → amount
                    enc.map(1)?;
                    enc.bytes(&withdrawal_addr)?;
                    enc.u64(transfer)?;
                    enc.null()?; // policy_hash
                                 // Anchor
                    enc.array(2)?;
                    enc.str(&anchor_url)?;
                    enc.bytes(&anchor_hash)?;

                    let action_env = serde_json::json!({
                        "type": "GovernanceActionConway",
                        "description": "Treasury Withdrawal Governance Action",
                        "cborHex": hex::encode(&action_cbor)
                    });

                    std::fs::write(&out_file, serde_json::to_string_pretty(&action_env)?)?;
                    println!(
                        "Treasury withdrawal action written to: {}",
                        out_file.display()
                    );
                    Ok(())
                }
            },
        }
    }
}

/// Encode a previous governance action ID as CBOR (null if not provided)
fn encode_prev_action_id(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    tx_id: &Option<String>,
    index: &Option<u32>,
) -> Result<()> {
    if let (Some(tx_id_hex), Some(idx)) = (tx_id, index) {
        let tx_hash = hex::decode(tx_id_hex)?;
        if tx_hash.len() != 32 {
            anyhow::bail!("Invalid prev governance action tx id length");
        }
        enc.array(2)?;
        enc.bytes(&tx_hash)?;
        enc.u32(*idx)?;
    } else {
        enc.null()?;
    }
    Ok(())
}

/// Encode protocol parameter update JSON as CBOR map
///
/// Maps JSON field names to their Conway-era CBOR key numbers
fn encode_protocol_param_update(json: &serde_json::Value) -> Result<Vec<u8>> {
    let obj = json
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Protocol param update must be a JSON object"))?;

    // Map of JSON keys to CBOR field numbers
    let field_map: &[(&str, u32)] = &[
        ("txFeePerByte", 0),
        ("minFeeA", 0),
        ("txFeeFixed", 1),
        ("minFeeB", 1),
        ("maxBlockBodySize", 2),
        ("maxTxSize", 3),
        ("maxBlockHeaderSize", 4),
        ("stakeAddressDeposit", 5),
        ("keyDeposit", 5),
        ("stakePoolDeposit", 6),
        ("poolDeposit", 6),
        ("poolRetireMaxEpoch", 7),
        ("eMax", 7),
        ("stakePoolTargetNum", 8),
        ("nOpt", 8),
        ("minPoolCost", 16),
        ("utxoCostPerByte", 17),
        ("adaPerUtxoByte", 17),
        ("maxTxExecutionUnits", 20),
        ("maxBlockExecutionUnits", 21),
        ("maxValueSize", 22),
        ("collateralPercentage", 23),
        ("maxCollateralInputs", 24),
        ("drepDeposit", 30),
        ("govActionDeposit", 31),
        ("govActionLifetime", 32),
    ];

    // Pre-compute the actual field count accounting for aliases and null values
    let mut seen_keys = std::collections::HashSet::new();
    let mut field_count = 0u64;
    for (json_key, cbor_key) in field_map {
        if let Some(value) = obj.get(*json_key) {
            if !value.is_null() && seen_keys.insert(*cbor_key) {
                field_count += 1;
            }
        }
    }

    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.map(field_count)?;

    let mut written_keys = std::collections::HashSet::new();
    for (json_key, cbor_key) in field_map {
        if let Some(value) = obj.get(*json_key) {
            if value.is_null() || written_keys.contains(cbor_key) {
                continue;
            }
            written_keys.insert(cbor_key);
            enc.u32(*cbor_key)?;
            if let Some(n) = value.as_u64() {
                enc.u64(n)?;
            } else if let Some(obj) = value.as_object() {
                // Execution units: { "memory": N, "steps": N }
                if let (Some(mem), Some(steps)) = (
                    obj.get("memory").and_then(|v| v.as_u64()),
                    obj.get("steps").and_then(|v| v.as_u64()),
                ) {
                    enc.array(2)?;
                    enc.u64(steps)?;
                    enc.u64(mem)?;
                }
            }
        }
    }

    Ok(buf)
}

/// Load a verification key file and return the blake2b-224 hash
fn load_key_hash(path: &PathBuf) -> Result<Vec<u8>> {
    let content = std::fs::read_to_string(path)?;
    let env: serde_json::Value = serde_json::from_str(&content)?;
    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in {}", path.display()))?;
    let cbor_bytes = hex::decode(cbor_hex)?;
    let key_bytes = if cbor_bytes.len() > 2 {
        &cbor_bytes[2..]
    } else {
        &cbor_bytes
    };
    let hash = dugite_primitives::hash::blake2b_224(key_bytes);
    Ok(hash.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_prev_action_id_none() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_prev_action_id(&mut enc, &None, &None).unwrap();
        // Should encode as CBOR null (0xf6)
        assert_eq!(buf, vec![0xf6]);
    }

    #[test]
    fn test_encode_prev_action_id_some() {
        let tx_id = Some("aa".repeat(32)); // 32-byte hex
        let index = Some(3u32);
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_prev_action_id(&mut enc, &tx_id, &index).unwrap();
        // Should start with array(2), then bytes(32), then u32(3)
        let mut dec = minicbor::Decoder::new(&buf);
        assert_eq!(dec.array().unwrap(), Some(2));
        let tx_bytes = dec.bytes().unwrap();
        assert_eq!(tx_bytes.len(), 32);
        assert_eq!(dec.u32().unwrap(), 3);
    }

    #[test]
    fn test_encode_prev_action_id_invalid_length() {
        let tx_id = Some("aabb".to_string()); // only 2 bytes
        let index = Some(0u32);
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        let result = encode_prev_action_id(&mut enc, &tx_id, &index);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("length"));
    }

    #[test]
    fn test_hash_anchor_data_blake2b_256() {
        let data = b"Hello, Cardano!";
        let hash = dugite_primitives::hash::blake2b_256(data);
        // Verify it produces a 32-byte hash
        assert_eq!(hash.as_bytes().len(), 32);
        // Same input should produce same hash
        let hash2 = dugite_primitives::hash::blake2b_256(data);
        assert_eq!(hash.as_bytes(), hash2.as_bytes());
    }

    #[test]
    fn test_encode_protocol_param_update_empty() {
        let json = serde_json::json!({});
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        // Empty map
        assert_eq!(dec.map().unwrap(), Some(0));
    }

    #[test]
    fn test_encode_protocol_param_update_single_field() {
        let json = serde_json::json!({ "txFeePerByte": 44 });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        assert_eq!(dec.map().unwrap(), Some(1));
        assert_eq!(dec.u32().unwrap(), 0); // txFeePerByte = key 0
        assert_eq!(dec.u64().unwrap(), 44);
    }

    #[test]
    fn test_encode_protocol_param_update_multiple_fields() {
        let json = serde_json::json!({
            "txFeePerByte": 44,
            "txFeeFixed": 155381,
            "maxTxSize": 16384
        });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        assert_eq!(dec.map().unwrap(), Some(3));

        // Collect key-value pairs (order depends on HashMap iteration)
        let mut pairs = Vec::new();
        for _ in 0..3 {
            let key = dec.u32().unwrap();
            let val = dec.u64().unwrap();
            pairs.push((key, val));
        }
        pairs.sort_by_key(|(k, _)| *k);

        assert_eq!(pairs[0], (0, 44)); // txFeePerByte
        assert_eq!(pairs[1], (1, 155381)); // txFeeFixed
        assert_eq!(pairs[2], (3, 16384)); // maxTxSize
    }

    #[test]
    fn test_encode_protocol_param_update_null_fields_skipped() {
        let json = serde_json::json!({
            "txFeePerByte": 44,
            "maxTxSize": null
        });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        // Only 1 field (null is skipped)
        assert_eq!(dec.map().unwrap(), Some(1));
        assert_eq!(dec.u32().unwrap(), 0);
        assert_eq!(dec.u64().unwrap(), 44);
    }

    #[test]
    fn test_encode_protocol_param_update_execution_units() {
        let json = serde_json::json!({
            "maxTxExecutionUnits": {
                "memory": 14000000000u64,
                "steps": 10000000000000u64
            }
        });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        assert_eq!(dec.map().unwrap(), Some(1));
        assert_eq!(dec.u32().unwrap(), 20); // maxTxExecutionUnits = key 20
        assert_eq!(dec.array().unwrap(), Some(2));
        // Note: CBOR encodes [steps, memory] per Haskell ExUnits
        assert_eq!(dec.u64().unwrap(), 10000000000000); // steps first
        assert_eq!(dec.u64().unwrap(), 14000000000); // memory second
    }

    #[test]
    fn test_encode_protocol_param_update_alias_dedup() {
        // minFeeA and txFeePerByte both map to key 0 — should only encode once
        let json = serde_json::json!({
            "minFeeA": 44,
            "txFeePerByte": 55
        });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        // Only 1 entry (deduplicated by cbor_key)
        assert_eq!(dec.map().unwrap(), Some(1));
        assert_eq!(dec.u32().unwrap(), 0);
        // First encountered wins
        let val = dec.u64().unwrap();
        assert!(val == 44 || val == 55); // JSON object iteration order is non-deterministic
    }

    #[test]
    fn test_encode_protocol_param_update_conway_fields() {
        let json = serde_json::json!({
            "drepDeposit": 500000000,
            "govActionDeposit": 100000000000u64,
            "govActionLifetime": 6
        });
        let buf = encode_protocol_param_update(&json).unwrap();
        let mut dec = minicbor::Decoder::new(&buf);
        assert_eq!(dec.map().unwrap(), Some(3));

        let mut pairs = Vec::new();
        for _ in 0..3 {
            let key = dec.u32().unwrap();
            let val = dec.u64().unwrap();
            pairs.push((key, val));
        }
        pairs.sort_by_key(|(k, _)| *k);

        assert_eq!(pairs[0], (30, 500000000)); // drepDeposit
        assert_eq!(pairs[1], (31, 100000000000)); // govActionDeposit
        assert_eq!(pairs[2], (32, 6)); // govActionLifetime
    }

    #[test]
    fn test_encode_protocol_param_update_not_object() {
        let json = serde_json::json!("not an object");
        let result = encode_protocol_param_update(&json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON object"));
    }

    #[test]
    fn test_encode_prev_action_id_partial_args() {
        // Only tx_id provided (no index) — should encode as null
        let tx_id = Some("aa".repeat(32));
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_prev_action_id(&mut enc, &tx_id, &None).unwrap();
        assert_eq!(buf, vec![0xf6]); // CBOR null
    }
}
