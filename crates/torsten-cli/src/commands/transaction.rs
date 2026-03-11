use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use torsten_primitives::hash::{Hash28, Hash32};

#[derive(Args, Debug)]
pub struct TransactionCmd {
    #[command(subcommand)]
    command: TxSubcommand,
}

#[derive(Subcommand, Debug)]
enum TxSubcommand {
    /// Build a transaction
    Build {
        /// Transaction inputs (format: tx_hash#index)
        #[arg(long, num_args = 1..)]
        tx_in: Vec<String>,
        /// Transaction outputs (format: address+amount)
        #[arg(long, num_args = 1..)]
        tx_out: Vec<String>,
        /// Change address
        #[arg(long)]
        change_address: String,
        /// Fee amount in lovelace
        #[arg(long, default_value = "200000")]
        fee: u64,
        /// Time-to-live (slot number)
        #[arg(long)]
        ttl: Option<u64>,
        /// Certificate files to include
        #[arg(long)]
        certificate_file: Vec<PathBuf>,
        /// Withdrawal (format: stake_address+amount)
        #[arg(long)]
        withdrawal: Vec<String>,
        /// Metadata JSON file
        #[arg(long)]
        metadata_json_file: Option<PathBuf>,
        /// Collateral inputs for Plutus scripts (format: tx_hash#index)
        #[arg(long)]
        tx_in_collateral: Vec<String>,
        /// Required signers (key hash hex)
        #[arg(long)]
        required_signer_hash: Vec<String>,
        /// Mint/burn tokens (format: policy_id.asset_name+quantity or policy_id.asset_name-quantity)
        #[arg(long)]
        mint: Vec<String>,
        /// Output file for the transaction body
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Sign a transaction
    Sign {
        /// Transaction file to sign
        #[arg(long)]
        tx_body_file: PathBuf,
        /// Signing key files
        #[arg(long, num_args = 1..)]
        signing_key_file: Vec<PathBuf>,
        /// Output file for signed transaction
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Submit a transaction
    Submit {
        /// Signed transaction file
        #[arg(long)]
        tx_file: PathBuf,
        /// Node socket path
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        /// Use mainnet (default)
        #[arg(long)]
        mainnet: bool,
        /// Testnet network magic
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Calculate transaction hash
    #[command(name = "txid")]
    Txid {
        /// Transaction file
        #[arg(long)]
        tx_file: PathBuf,
    },
    /// Calculate the minimum fee for a transaction
    CalculateMinFee {
        /// Transaction body file
        #[arg(long)]
        tx_body_file: PathBuf,
        /// Number of signing keys (witnesses)
        #[arg(long, default_value = "1")]
        witness_count: u64,
        /// Protocol parameters file (JSON)
        #[arg(long)]
        protocol_params_file: PathBuf,
    },
    /// View transaction contents
    View {
        /// Transaction file
        #[arg(long)]
        tx_file: PathBuf,
    },
    /// Create a transaction witness
    Witness {
        /// Transaction body file to witness
        #[arg(long)]
        tx_body_file: PathBuf,
        /// Signing key file
        #[arg(long)]
        signing_key_file: PathBuf,
        /// Output witness file
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Assemble a transaction from a body and witnesses
    Assemble {
        /// Transaction body file
        #[arg(long)]
        tx_body_file: PathBuf,
        /// Witness files
        #[arg(long, num_args = 1..)]
        witness_file: Vec<PathBuf>,
        /// Output signed transaction file
        #[arg(long)]
        out_file: PathBuf,
    },
    /// Get the policy ID from a script file
    Policyid {
        /// Script file
        #[arg(long)]
        script_file: PathBuf,
    },
}

/// Parse a tx input string "tx_hash#index" into (hash, index)
fn parse_tx_input(s: &str) -> Result<(Hash32, u32)> {
    let parts: Vec<&str> = s.split('#').collect();
    if parts.len() != 2 {
        bail!("Invalid tx input format: '{s}'. Expected tx_hash#index");
    }
    let hash_bytes = hex::decode(parts[0])?;
    if hash_bytes.len() != 32 {
        bail!(
            "Invalid transaction hash length: {} bytes",
            hash_bytes.len()
        );
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&hash_bytes);
    let hash = Hash32::from_bytes(arr);
    let index: u32 = parts[1].parse()?;
    Ok((hash, index))
}

/// Parsed transaction output with optional multi-asset tokens
struct ParsedTxOutput {
    address: String,
    lovelace: u64,
    /// Native tokens: Vec<(policy_hex, asset_name_hex, quantity)>
    tokens: Vec<(String, String, u64)>,
}

/// Parse a tx output string into address, lovelace, and optional native tokens.
///
/// Supported formats:
/// - `address+amount` (ADA only)
/// - `address+amount+"policy_id.asset_name amount+..."` (with native tokens)
fn parse_tx_output(s: &str) -> Result<ParsedTxOutput> {
    let parts: Vec<&str> = s.splitn(3, '+').collect();
    if parts.len() < 2 {
        bail!("Invalid tx output format: '{s}'. Expected address+amount[+tokens]");
    }
    let address = parts[0].to_string();
    let lovelace: u64 = parts[1].trim().parse()?;

    let mut tokens = Vec::new();
    if parts.len() == 3 {
        // Parse multi-asset tokens: "policy.name qty+policy.name qty+..."
        let token_str = parts[2].trim().trim_matches('"');
        for token_part in token_str.split('+') {
            let token_part = token_part.trim();
            if token_part.is_empty() {
                continue;
            }
            // Format: "policy_id.asset_name qty" or "policy_id qty"
            let token_parts: Vec<&str> = token_part.splitn(2, ' ').collect();
            if token_parts.len() != 2 {
                bail!("Invalid token format: '{token_part}'. Expected 'policy_id.asset_name quantity'");
            }
            let qty: u64 = token_parts[1].trim().parse()?;
            let asset_id = token_parts[0];
            let (policy, asset_name) = if let Some(dot_pos) = asset_id.find('.') {
                (
                    asset_id[..dot_pos].to_string(),
                    asset_id[dot_pos + 1..].to_string(),
                )
            } else {
                (asset_id.to_string(), String::new())
            };
            // Validate policy is valid hex (56 chars = 28 bytes)
            if policy.len() != 56 {
                bail!(
                    "Invalid policy ID length: expected 56 hex chars, got {}",
                    policy.len()
                );
            }
            hex::decode(&policy)?;
            tokens.push((policy, asset_name, qty));
        }
    }

    Ok(ParsedTxOutput {
        address,
        lovelace,
        tokens,
    })
}

/// Build a CBOR transaction body
#[allow(clippy::too_many_arguments)]
fn build_tx_body_cbor(
    inputs: &[(Hash32, u32)],
    outputs: &[ParsedTxOutput],
    fee: u64,
    ttl: Option<u64>,
    certificates: &[Vec<u8>],
    withdrawals: &[(Vec<u8>, u64)],
    auxiliary_data: Option<&[u8]>,
    collateral_inputs: &[(Hash32, u32)],
    required_signers: &[Vec<u8>],
    mint: &[MintEntry],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // Transaction body map fields:
    // 0: inputs, 1: outputs, 2: fee, 3: ttl, 4: certificates, 5: withdrawals,
    // 7: auxiliary_data_hash, 9: mint, 13: collateral, 14: required_signers
    let mut field_count = 3u64; // inputs + outputs + fee
    if ttl.is_some() {
        field_count += 1;
    }
    if !certificates.is_empty() {
        field_count += 1;
    }
    if !withdrawals.is_empty() {
        field_count += 1;
    }
    if auxiliary_data.is_some() {
        field_count += 1;
    }
    if !mint.is_empty() {
        field_count += 1;
    }
    if !collateral_inputs.is_empty() {
        field_count += 1;
    }
    if !required_signers.is_empty() {
        field_count += 1;
    }
    enc.map(field_count)?;

    // Field 0: inputs
    enc.u32(0)?;
    enc.array(inputs.len() as u64)?;
    for (hash, index) in inputs {
        enc.array(2)?;
        enc.bytes(hash.as_bytes())?;
        enc.u32(*index)?;
    }

    // Field 1: outputs (post-Alonzo format)
    enc.u32(1)?;
    enc.array(outputs.len() as u64)?;
    for output in outputs {
        let (_hrp, addr_bytes) = bech32::decode(&output.address)
            .map_err(|e| anyhow::anyhow!("Invalid bech32 address '{}': {e}", output.address))?;

        if output.tokens.is_empty() {
            // Simple output: [address, amount]
            enc.array(2)?;
            enc.bytes(&addr_bytes)?;
            enc.u64(output.lovelace)?;
        } else {
            // Multi-asset output: [address, [lovelace, {policy: {asset: qty}}]]
            enc.array(2)?;
            enc.bytes(&addr_bytes)?;

            // Value: [lovelace, multi_asset_map]
            enc.array(2)?;
            enc.u64(output.lovelace)?;

            // Group tokens by policy
            let mut policy_map: std::collections::BTreeMap<Vec<u8>, Vec<(Vec<u8>, u64)>> =
                std::collections::BTreeMap::new();
            for (policy_hex, asset_name_hex, qty) in &output.tokens {
                let policy_bytes = hex::decode(policy_hex)?;
                let asset_bytes = hex::decode(asset_name_hex).unwrap_or_default();
                policy_map
                    .entry(policy_bytes)
                    .or_default()
                    .push((asset_bytes, *qty));
            }

            enc.map(policy_map.len() as u64)?;
            for (policy_bytes, assets) in &policy_map {
                enc.bytes(policy_bytes)?;
                enc.map(assets.len() as u64)?;
                for (asset_name, qty) in assets {
                    enc.bytes(asset_name)?;
                    enc.u64(*qty)?;
                }
            }
        }
    }

    // Field 2: fee
    enc.u32(2)?;
    enc.u64(fee)?;

    // Field 3: ttl (optional)
    if let Some(ttl_val) = ttl {
        enc.u32(3)?;
        enc.u64(ttl_val)?;
    }

    // Field 4: certificates (optional)
    if !certificates.is_empty() {
        enc.u32(4)?;
        enc.array(certificates.len() as u64)?;
        for cert_cbor in certificates {
            // Re-encode each cert byte-by-byte through the encoder's writer
            // by writing raw CBOR via the underlying writer
            enc.writer_mut().extend_from_slice(cert_cbor);
        }
    }

    // Field 5: withdrawals (optional)
    if !withdrawals.is_empty() {
        enc.u32(5)?;
        enc.map(withdrawals.len() as u64)?;
        for (reward_addr, amount) in withdrawals {
            enc.bytes(reward_addr)?;
            enc.u64(*amount)?;
        }
    }

    // Field 7: auxiliary data hash (optional)
    if let Some(aux_data) = auxiliary_data {
        let aux_hash = torsten_primitives::hash::blake2b_256(aux_data);
        enc.u32(7)?;
        enc.bytes(aux_hash.as_bytes())?;
    }

    // Field 9: mint (optional) — map { policy_id: { asset_name: quantity } }
    if !mint.is_empty() {
        enc.u32(9)?;
        enc.map(mint.len() as u64)?;
        for (policy_bytes, assets) in mint {
            enc.bytes(policy_bytes)?;
            enc.map(assets.len() as u64)?;
            for (asset_name, qty) in assets {
                enc.bytes(asset_name)?;
                // Mint quantities can be negative (burning)
                if *qty >= 0 {
                    enc.u64(*qty as u64)?;
                } else {
                    // CBOR negative integer: encode as -(1+n) → major type 1
                    let neg = (-1 - *qty) as u64;
                    // Manual CBOR major type 1 encoding
                    let neg_bytes = minicbor::to_vec(neg)?;
                    // Set major type to 1 (negative)
                    let mut neg_cbor = neg_bytes;
                    neg_cbor[0] = (neg_cbor[0] & 0x1f) | 0x20;
                    enc.writer_mut().extend_from_slice(&neg_cbor);
                }
            }
        }
    }

    // Field 13: collateral inputs (optional)
    if !collateral_inputs.is_empty() {
        enc.u32(13)?;
        enc.array(collateral_inputs.len() as u64)?;
        for (hash, index) in collateral_inputs {
            enc.array(2)?;
            enc.bytes(hash.as_bytes())?;
            enc.u32(*index)?;
        }
    }

    // Field 14: required signers (optional)
    if !required_signers.is_empty() {
        enc.u32(14)?;
        enc.array(required_signers.len() as u64)?;
        for signer_hash in required_signers {
            enc.bytes(signer_hash)?;
        }
    }

    Ok(buf)
}

/// Mint entry: (policy_id_bytes, Vec<(asset_name_bytes, quantity)>)
type MintEntry = (Vec<u8>, Vec<(Vec<u8>, i64)>);

/// Parse mint arguments into grouped policy -> [(asset_name, quantity)] structure.
/// Format: "policy_id.asset_name_hex+quantity" or "policy_id.asset_name_hex-quantity" (for burn)
fn parse_mint_args(mint_args: &[String]) -> Result<Vec<MintEntry>> {
    use std::collections::BTreeMap;
    let mut policy_map: BTreeMap<Vec<u8>, Vec<(Vec<u8>, i64)>> = BTreeMap::new();

    for arg in mint_args {
        // Split on '+' or find '-' for negative quantities
        let (policy_asset, qty_str, sign) = if let Some(idx) = arg.rfind('+') {
            (&arg[..idx], &arg[idx + 1..], 1i64)
        } else if let Some(idx) = arg.rfind('-') {
            // Make sure '-' is not in the policy_id part (hex chars)
            if idx > 0 {
                (&arg[..idx], &arg[idx + 1..], -1i64)
            } else {
                bail!("Invalid mint format: '{arg}'. Expected policy_id.asset_name+quantity");
            }
        } else {
            bail!("Invalid mint format: '{arg}'. Expected policy_id.asset_name+quantity");
        };

        let qty: i64 = qty_str
            .trim()
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("Invalid mint quantity '{qty_str}': {e}"))?
            * sign;

        // Split policy_asset on '.'
        let (policy_hex, asset_hex) = if let Some(dot_idx) = policy_asset.find('.') {
            (&policy_asset[..dot_idx], &policy_asset[dot_idx + 1..])
        } else {
            (policy_asset, "")
        };

        let policy_bytes = hex::decode(policy_hex)
            .map_err(|e| anyhow::anyhow!("Invalid policy ID hex '{policy_hex}': {e}"))?;
        let asset_bytes = if asset_hex.is_empty() {
            vec![]
        } else {
            hex::decode(asset_hex)
                .map_err(|e| anyhow::anyhow!("Invalid asset name hex '{asset_hex}': {e}"))?
        };

        policy_map
            .entry(policy_bytes)
            .or_default()
            .push((asset_bytes, qty));
    }

    Ok(policy_map.into_iter().collect())
}

/// Parse a cardano-cli JSON native script into our NativeScript type.
///
/// Supported JSON "type" values:
/// - "sig" / "ScriptPubkey": requires a key hash
/// - "all" / "ScriptAll": requires all sub-scripts
/// - "any" / "ScriptAny": requires any sub-script
/// - "atLeast" / "ScriptNOfK": requires N of K sub-scripts
/// - "after" / "InvalidBefore": valid after slot
/// - "before" / "InvalidHereafter": valid before slot
fn parse_json_native_script(
    json: &serde_json::Value,
) -> Result<torsten_primitives::transaction::NativeScript> {
    use torsten_primitives::transaction::NativeScript;

    let script_type = json["type"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Native script missing 'type' field"))?;

    match script_type {
        "sig" | "ScriptPubkey" => {
            let key_hash_hex = json["keyHash"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("sig script missing 'keyHash'"))?;
            let key_hash_bytes = hex::decode(key_hash_hex)
                .map_err(|e| anyhow::anyhow!("Invalid keyHash hex: {e}"))?;
            if key_hash_bytes.len() != 28 {
                bail!("keyHash must be 28 bytes, got {}", key_hash_bytes.len());
            }
            // Pad 28-byte key hash to Hash32 (our internal representation)
            let h28 = Hash28::try_from(key_hash_bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("Failed to convert key hash to Hash28"))?;
            Ok(NativeScript::ScriptPubkey(h28.to_hash32_padded()))
        }
        "all" | "ScriptAll" => {
            let scripts = parse_json_script_list(json)?;
            Ok(NativeScript::ScriptAll(scripts))
        }
        "any" | "ScriptAny" => {
            let scripts = parse_json_script_list(json)?;
            Ok(NativeScript::ScriptAny(scripts))
        }
        "atLeast" | "ScriptNOfK" => {
            let required = json["required"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("atLeast script missing 'required'"))?
                as u32;
            let scripts = parse_json_script_list(json)?;
            Ok(NativeScript::ScriptNOfK(required, scripts))
        }
        "after" | "InvalidBefore" => {
            let slot = json["slot"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("after script missing 'slot'"))?;
            Ok(NativeScript::InvalidBefore(
                torsten_primitives::time::SlotNo(slot),
            ))
        }
        "before" | "InvalidHereafter" => {
            let slot = json["slot"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("before script missing 'slot'"))?;
            Ok(NativeScript::InvalidHereafter(
                torsten_primitives::time::SlotNo(slot),
            ))
        }
        _ => bail!("Unknown native script type: '{script_type}'"),
    }
}

fn parse_json_script_list(
    json: &serde_json::Value,
) -> Result<Vec<torsten_primitives::transaction::NativeScript>> {
    let scripts_arr = json["scripts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Script missing 'scripts' array"))?;
    scripts_arr
        .iter()
        .map(parse_json_native_script)
        .collect::<Result<Vec<_>>>()
}

/// Decode and print a summary of a transaction output from CBOR.
/// Handles both legacy array format and post-Alonzo map format.
fn decode_output_summary(decoder: &mut minicbor::Decoder<'_>) {
    let pos = decoder.position();
    // Try map format first (post-Alonzo: {0: addr, 1: value, ...})
    if let Ok(Some(_map_len)) = decoder.map() {
        let mut addr_hex = String::new();
        let mut lovelace = 0u64;
        let mut has_tokens = false;
        loop {
            let Ok(key) = decoder.u32() else { break };
            match key {
                0 => {
                    addr_hex = decoder.bytes().map(hex::encode).unwrap_or_default();
                }
                1 => {
                    // Value: either uint (pure ADA) or [uint, multiasset_map]
                    if let Ok(coin) = decoder.u64() {
                        lovelace = coin;
                    } else {
                        // Array: [coin, multiasset]
                        decoder.set_position(decoder.position());
                        if decoder.array().is_ok() {
                            lovelace = decoder.u64().unwrap_or(0);
                            has_tokens = true;
                            decoder.skip().ok(); // skip multiasset
                        }
                    }
                }
                _ => {
                    decoder.skip().ok();
                }
            }
        }
        let ada = lovelace as f64 / 1_000_000.0;
        let token_info = if has_tokens { " + tokens" } else { "" };
        println!("{lovelace} lovelace ({ada:.6} ADA){token_info}");
        if !addr_hex.is_empty() {
            println!(
                "      addr: {}",
                &addr_hex[..std::cmp::min(40, addr_hex.len())]
            );
        }
        return;
    }

    // Fallback: try array format [addr, value, ...]
    decoder.set_position(pos);
    if let Ok(Some(_arr_len)) = decoder.array() {
        let addr_hex = decoder.bytes().map(hex::encode).unwrap_or_default();
        let lovelace = decoder.u64().unwrap_or(0);
        let ada = lovelace as f64 / 1_000_000.0;
        println!("{lovelace} lovelace ({ada:.6} ADA)");
        if !addr_hex.is_empty() {
            println!(
                "      addr: {}",
                &addr_hex[..std::cmp::min(40, addr_hex.len())]
            );
        }
        // Skip remaining elements (datum, script_ref)
        for _ in 2.._arr_len {
            decoder.skip().ok();
        }
        return;
    }

    // Can't decode — skip
    decoder.set_position(pos);
    decoder.skip().ok();
    println!("<unable to decode>");
}

/// Load certificate CBOR from a text envelope file
fn load_certificate_cbor(path: &PathBuf) -> Result<Vec<u8>> {
    let content = std::fs::read_to_string(path)?;
    let env: serde_json::Value = serde_json::from_str(&content)?;
    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in {}", path.display()))?;
    Ok(hex::decode(cbor_hex)?)
}

/// Parse a withdrawal string "stake_address+amount"
fn parse_withdrawal(s: &str) -> Result<(Vec<u8>, u64)> {
    let parts: Vec<&str> = s.split('+').collect();
    if parts.len() != 2 {
        bail!("Invalid withdrawal format: '{s}'. Expected stake_address+amount");
    }
    let (_hrp, addr_bytes) = bech32::decode(parts[0])
        .map_err(|e| anyhow::anyhow!("Invalid stake address '{}': {e}", parts[0]))?;
    let amount: u64 = parts[1].trim().parse()?;
    Ok((addr_bytes, amount))
}

/// Build auxiliary data CBOR from a JSON metadata file
fn build_auxiliary_data(metadata_json: &serde_json::Value) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // Auxiliary data is: map { 0: metadata_map }
    // where metadata_map is: { label => value }
    enc.map(1)?;
    enc.u32(0)?;

    if let Some(obj) = metadata_json.as_object() {
        enc.map(obj.len() as u64)?;
        for (key, value) in obj {
            let label: u64 = key
                .parse()
                .map_err(|_| anyhow::anyhow!("Metadata label must be an integer, got '{key}'"))?;
            enc.u64(label)?;
            encode_metadata_value(&mut enc, value)?;
        }
    } else {
        anyhow::bail!("Metadata JSON must be an object with integer keys");
    }

    Ok(buf)
}

/// Recursively encode a JSON metadata value as CBOR
fn encode_metadata_value(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    value: &serde_json::Value,
) -> Result<()> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= 0 {
                    enc.u64(i as u64)?;
                } else {
                    enc.i64(i)?;
                }
            }
        }
        serde_json::Value::String(s) => {
            enc.str(s)?;
        }
        serde_json::Value::Array(arr) => {
            enc.array(arr.len() as u64)?;
            for item in arr {
                encode_metadata_value(enc, item)?;
            }
        }
        serde_json::Value::Object(obj) => {
            enc.map(obj.len() as u64)?;
            for (k, v) in obj {
                enc.str(k)?;
                encode_metadata_value(enc, v)?;
            }
        }
        serde_json::Value::Bool(b) => {
            enc.bool(*b)?;
        }
        serde_json::Value::Null => {
            enc.null()?;
        }
    }
    Ok(())
}

impl TransactionCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            TxSubcommand::Build {
                tx_in,
                tx_out,
                change_address: _,
                fee,
                ttl,
                certificate_file,
                withdrawal,
                metadata_json_file,
                tx_in_collateral,
                required_signer_hash,
                mint,
                out_file,
            } => {
                if tx_in.is_empty() {
                    bail!("At least one --tx-in is required");
                }
                if tx_out.is_empty() {
                    bail!("At least one --tx-out is required");
                }

                let inputs: Vec<(Hash32, u32)> = tx_in
                    .iter()
                    .map(|s| parse_tx_input(s))
                    .collect::<Result<_>>()?;
                let outputs: Vec<ParsedTxOutput> = tx_out
                    .iter()
                    .map(|s| parse_tx_output(s))
                    .collect::<Result<_>>()?;

                let collateral_inputs: Vec<(Hash32, u32)> = tx_in_collateral
                    .iter()
                    .map(|s| parse_tx_input(s))
                    .collect::<Result<_>>()?;

                let required_signers: Vec<Vec<u8>> = required_signer_hash
                    .iter()
                    .map(|s| {
                        hex::decode(s).map_err(|e| anyhow::anyhow!("Invalid signer hash: {e}"))
                    })
                    .collect::<Result<_>>()?;

                let parsed_mint = parse_mint_args(&mint)?;

                let certificates: Vec<Vec<u8>> = certificate_file
                    .iter()
                    .map(load_certificate_cbor)
                    .collect::<Result<_>>()?;

                let withdrawals: Vec<(Vec<u8>, u64)> = withdrawal
                    .iter()
                    .map(|s| parse_withdrawal(s))
                    .collect::<Result<_>>()?;

                let auxiliary_data = if let Some(ref meta_file) = metadata_json_file {
                    let meta_content = std::fs::read_to_string(meta_file)?;
                    let meta_json: serde_json::Value = serde_json::from_str(&meta_content)?;
                    Some(build_auxiliary_data(&meta_json)?)
                } else {
                    None
                };

                let tx_body_cbor = build_tx_body_cbor(
                    &inputs,
                    &outputs,
                    fee,
                    ttl,
                    &certificates,
                    &withdrawals,
                    auxiliary_data.as_deref(),
                    &collateral_inputs,
                    &required_signers,
                    &parsed_mint,
                )?;

                // Write as text envelope (cardano-cli compatible format)
                let mut envelope = serde_json::json!({
                    "type": "TxBodyConway",
                    "description": "Transaction Body",
                    "cborHex": hex::encode(&tx_body_cbor)
                });

                // Include auxiliary data if present (for sign/assemble to embed in tx)
                if let Some(ref aux) = auxiliary_data {
                    envelope["auxiliaryDataCborHex"] = serde_json::Value::String(hex::encode(aux));
                }

                std::fs::write(&out_file, serde_json::to_string_pretty(&envelope)?)?;
                println!("Transaction body written to: {}", out_file.display());
                Ok(())
            }
            TxSubcommand::Sign {
                tx_body_file,
                signing_key_file,
                out_file,
            } => {
                // Read the tx body envelope
                let content = std::fs::read_to_string(&tx_body_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let tx_body_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx body file"))?;
                let tx_body_cbor = hex::decode(tx_body_hex)?;

                // Hash the transaction body
                let tx_hash = torsten_crypto::signing::hash_transaction(&tx_body_cbor);

                // Sign with each key
                let mut witnesses = Vec::new();
                for key_file in &signing_key_file {
                    let key_content = std::fs::read_to_string(key_file)?;
                    let key_env: serde_json::Value = serde_json::from_str(&key_content)?;
                    let key_cbor_hex = key_env["cborHex"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in key file"))?;
                    let key_cbor = hex::decode(key_cbor_hex)?;
                    // Skip CBOR wrapper (2 bytes for byte string header)
                    let key_bytes = if key_cbor.len() > 2 {
                        &key_cbor[2..]
                    } else {
                        &key_cbor
                    };

                    let sk = torsten_crypto::keys::PaymentSigningKey::from_bytes(key_bytes)?;
                    let vk = sk.verification_key();
                    let signature = sk.sign(tx_hash.as_bytes());

                    witnesses.push((vk.to_bytes().to_vec(), signature));
                }

                // Build signed transaction CBOR: [tx_body, witnesses, true, null]
                let mut signed_buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut signed_buf);
                enc.array(4)?;

                // Raw tx body CBOR (embed as-is using tag-less bytes)
                // We need to include the raw CBOR, so write it directly
                signed_buf.extend_from_slice(&tx_body_cbor);

                // Re-create encoder after extending
                let mut witness_buf = Vec::new();
                let mut wenc = minicbor::Encoder::new(&mut witness_buf);

                // Witness set: map { 0: [[vkey, sig], ...] }
                wenc.map(1)?;
                wenc.u32(0)?;
                wenc.array(witnesses.len() as u64)?;
                for (vkey, sig) in &witnesses {
                    wenc.array(2)?;
                    wenc.bytes(vkey)?;
                    wenc.bytes(sig)?;
                }

                // Build complete signed tx: [body, witness_set, true, null]
                let mut final_buf = Vec::new();
                let mut fenc = minicbor::Encoder::new(&mut final_buf);
                fenc.array(4)?;
                // Embed raw body CBOR
                final_buf.extend_from_slice(&tx_body_cbor);
                // Embed witness set
                final_buf.extend_from_slice(&witness_buf);
                // is_valid = true
                let mut tail = Vec::new();
                let mut tenc = minicbor::Encoder::new(&mut tail);
                tenc.bool(true)?;
                final_buf.extend_from_slice(&tail);

                // auxiliary_data: include if present in tx body envelope, otherwise null
                if let Some(aux_hex) = envelope
                    .get("auxiliaryDataCborHex")
                    .and_then(|v| v.as_str())
                {
                    let aux_cbor = hex::decode(aux_hex)?;
                    final_buf.extend_from_slice(&aux_cbor);
                } else {
                    let mut null_buf = Vec::new();
                    let mut nenc = minicbor::Encoder::new(&mut null_buf);
                    nenc.null()?;
                    final_buf.extend_from_slice(&null_buf);
                }

                let signed_envelope = serde_json::json!({
                    "type": "Tx ConwayEra",
                    "description": "Signed Transaction",
                    "cborHex": hex::encode(&final_buf)
                });

                std::fs::write(&out_file, serde_json::to_string_pretty(&signed_envelope)?)?;
                println!("Signed transaction written to: {}", out_file.display());
                println!("Transaction hash: {tx_hash}");
                Ok(())
            }
            TxSubcommand::Submit {
                tx_file,
                socket_path,
                testnet_magic,
                ..
            } => {
                // Read signed transaction
                let content = std::fs::read_to_string(&tx_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx file"))?;
                let tx_cbor = hex::decode(cbor_hex)?;

                // Compute the transaction ID for display
                let body_cbor = extract_tx_body(&tx_cbor)?;
                let tx_hash = torsten_crypto::signing::hash_transaction(&body_cbor);

                // Submit via N2C socket
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(async {
                    let mut client = torsten_network::N2CClient::connect(&socket_path)
                        .await
                        .map_err(|e| anyhow::anyhow!("Cannot connect to node socket: {e}"))?;

                    let magic = testnet_magic.unwrap_or(764824073);
                    client
                        .handshake(magic)
                        .await
                        .map_err(|e| anyhow::anyhow!("Handshake failed: {e}"))?;

                    // Submit the transaction
                    client
                        .submit_tx(&tx_cbor)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;

                    println!("Transaction successfully submitted.");
                    println!("Transaction ID: {tx_hash}");
                    Ok::<(), anyhow::Error>(())
                })?;

                Ok(())
            }
            TxSubcommand::Txid { tx_file } => {
                let content = std::fs::read_to_string(&tx_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx file"))?;
                let cbor_bytes = hex::decode(cbor_hex)?;

                // If it's a signed tx [body, witnesses, valid, aux], we need just the body
                // For a tx body file, the whole thing is the body
                let tx_type = envelope["type"].as_str().unwrap_or("");
                let body_cbor = if tx_type.contains("Tx ") || tx_type.contains("Signed") {
                    // Signed tx - extract body from the array
                    // For simplicity, hash the whole body portion
                    // The body is the first element of the CBOR array
                    extract_tx_body(&cbor_bytes)?
                } else {
                    cbor_bytes
                };

                let hash = torsten_crypto::signing::hash_transaction(&body_cbor);
                println!("{hash}");
                Ok(())
            }
            TxSubcommand::CalculateMinFee {
                tx_body_file,
                witness_count,
                protocol_params_file,
            } => {
                // Read protocol parameters
                let pp_content = std::fs::read_to_string(&protocol_params_file)?;
                let pp: serde_json::Value = serde_json::from_str(&pp_content)?;

                let min_fee_a = pp["txFeePerByte"]
                    .as_u64()
                    .or_else(|| pp["minFeeA"].as_u64())
                    .unwrap_or(44);
                let min_fee_b = pp["txFeeFixed"]
                    .as_u64()
                    .or_else(|| pp["minFeeB"].as_u64())
                    .unwrap_or(155381);

                // Read tx body to get its size
                let content = std::fs::read_to_string(&tx_body_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx body file"))?;
                let tx_body_cbor = hex::decode(cbor_hex)?;

                // Estimate full signed tx size:
                // tx body + witness overhead per key (vkey 32 + sig 64 + CBOR wrapping ~10)
                let witness_overhead = witness_count * 106;
                // Signed tx envelope: array(4) + body + witness_set + bool + null (~4 bytes)
                let estimated_size = tx_body_cbor.len() as u64 + witness_overhead + 11;

                // fee = min_fee_a * tx_size + min_fee_b
                let fee = min_fee_a * estimated_size + min_fee_b;
                println!("{fee} Lovelace");
                Ok(())
            }
            TxSubcommand::View { tx_file } => {
                let content = std::fs::read_to_string(&tx_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let tx_type = envelope["type"].as_str().unwrap_or("unknown");
                let cbor_hex = envelope["cborHex"].as_str().unwrap_or("");

                println!("Type: {tx_type}");
                println!("CBOR size: {} bytes", cbor_hex.len() / 2);

                let cbor_bytes = hex::decode(cbor_hex)?;
                let body_cbor = if tx_type.contains("Tx ") || tx_type.contains("Signed") {
                    extract_tx_body(&cbor_bytes)?
                } else {
                    cbor_bytes.clone()
                };

                let hash = torsten_crypto::signing::hash_transaction(&body_cbor);
                println!("Transaction hash: {hash}");

                // Decode and display transaction body fields
                let mut decoder = minicbor::Decoder::new(&body_cbor);
                if let Ok(Some(map_len)) = decoder.map() {
                    for _ in 0..map_len {
                        if let Ok(key) = decoder.u32() {
                            match key {
                                0 => {
                                    // Inputs: array of [tx_hash, index]
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Inputs ({arr_len}):");
                                        for _ in 0..arr_len {
                                            if decoder.array().is_ok() {
                                                let tx_hash = decoder
                                                    .bytes()
                                                    .map(hex::encode)
                                                    .unwrap_or_default();
                                                let idx = decoder.u32().unwrap_or(0);
                                                println!("  {tx_hash}#{idx}");
                                            }
                                        }
                                    }
                                }
                                1 => {
                                    // Outputs
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Outputs ({arr_len}):");
                                        for i in 0..arr_len {
                                            print!("  [{i}] ");
                                            decode_output_summary(&mut decoder);
                                        }
                                    }
                                }
                                2 => {
                                    if let Ok(fee) = decoder.u64() {
                                        let ada = fee as f64 / 1_000_000.0;
                                        println!("Fee: {fee} lovelace ({ada:.6} ADA)");
                                    }
                                }
                                3 => {
                                    if let Ok(ttl) = decoder.u64() {
                                        println!("TTL: slot {ttl}");
                                    }
                                }
                                4 => {
                                    // Certificates
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Certificates: {arr_len}");
                                        for _ in 0..arr_len {
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                5 => {
                                    // Withdrawals
                                    if let Ok(Some(map_len)) = decoder.map() {
                                        println!("Withdrawals: {map_len}");
                                        for _ in 0..map_len {
                                            decoder.skip().ok();
                                            decoder.skip().ok();
                                        }
                                    }
                                }
                                7 => {
                                    // Auxiliary data hash
                                    if let Ok(h) = decoder.bytes() {
                                        println!("Auxiliary data hash: {}", hex::encode(h));
                                    }
                                }
                                8 => {
                                    // Validity interval start
                                    if let Ok(slot) = decoder.u64() {
                                        println!("Valid from: slot {slot}");
                                    }
                                }
                                9 => {
                                    // Mint
                                    if let Ok(Some(policy_count)) = decoder.map() {
                                        println!("Mint ({policy_count} policies):");
                                        for _ in 0..policy_count {
                                            let pid = decoder
                                                .bytes()
                                                .map(hex::encode)
                                                .unwrap_or_default();
                                            if let Ok(Some(asset_count)) = decoder.map() {
                                                for _ in 0..asset_count {
                                                    let name = decoder
                                                        .bytes()
                                                        .map(hex::encode)
                                                        .unwrap_or_default();
                                                    // Could be positive or negative
                                                    let qty = decoder.i64().unwrap_or(0);
                                                    let sign = if qty >= 0 { "+" } else { "" };
                                                    println!("  {pid}.{name} {sign}{qty}");
                                                }
                                            }
                                        }
                                    }
                                }
                                11 => {
                                    // Script data hash
                                    if let Ok(h) = decoder.bytes() {
                                        println!("Script data hash: {}", hex::encode(h));
                                    }
                                }
                                13 => {
                                    // Collateral inputs
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Collateral inputs ({arr_len}):");
                                        for _ in 0..arr_len {
                                            if decoder.array().is_ok() {
                                                let tx_hash = decoder
                                                    .bytes()
                                                    .map(hex::encode)
                                                    .unwrap_or_default();
                                                let idx = decoder.u32().unwrap_or(0);
                                                println!("  {tx_hash}#{idx}");
                                            }
                                        }
                                    }
                                }
                                14 => {
                                    // Required signers
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Required signers ({arr_len}):");
                                        for _ in 0..arr_len {
                                            if let Ok(h) = decoder.bytes() {
                                                println!("  {}", hex::encode(h));
                                            }
                                        }
                                    }
                                }
                                16 => {
                                    // Collateral return
                                    print!("Collateral return: ");
                                    decode_output_summary(&mut decoder);
                                }
                                17 => {
                                    // Total collateral
                                    if let Ok(c) = decoder.u64() {
                                        println!("Total collateral: {c} lovelace");
                                    }
                                }
                                18 => {
                                    // Reference inputs
                                    if let Ok(Some(arr_len)) = decoder.array() {
                                        println!("Reference inputs ({arr_len}):");
                                        for _ in 0..arr_len {
                                            if decoder.array().is_ok() {
                                                let tx_hash = decoder
                                                    .bytes()
                                                    .map(hex::encode)
                                                    .unwrap_or_default();
                                                let idx = decoder.u32().unwrap_or(0);
                                                println!("  {tx_hash}#{idx}");
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    println!("Field {key}: <present>");
                                    decoder.skip().ok();
                                }
                            }
                        }
                    }
                }

                Ok(())
            }
            TxSubcommand::Witness {
                tx_body_file,
                signing_key_file,
                out_file,
            } => {
                let content = std::fs::read_to_string(&tx_body_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex"))?;
                let body_cbor = hex::decode(cbor_hex)?;

                let hash = torsten_crypto::signing::hash_transaction(&body_cbor);
                let hash_bytes = hash.to_vec();

                let key_content = std::fs::read_to_string(&signing_key_file)?;
                let key_env: serde_json::Value = serde_json::from_str(&key_content)?;
                let key_cbor_hex = key_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in signing key"))?;
                let key_cbor = hex::decode(key_cbor_hex)?;
                let key_bytes = if key_cbor.len() > 2 {
                    &key_cbor[2..]
                } else {
                    &key_cbor
                };

                let sk = torsten_crypto::keys::PaymentSigningKey::from_bytes(key_bytes)?;
                let vk = sk.verification_key();
                let signature = sk.sign(&hash_bytes);

                // Witness CBOR: [vkey, signature]
                let mut witness_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut witness_cbor);
                enc.array(2)?;
                enc.bytes(&vk.to_bytes())?;
                enc.bytes(&signature)?;

                let witness_env = serde_json::json!({
                    "type": "TxWitness ShelleyEra",
                    "description": "",
                    "cborHex": hex::encode(&witness_cbor)
                });
                std::fs::write(&out_file, serde_json::to_string_pretty(&witness_env)?)?;
                println!("Witness written to: {}", out_file.display());
                Ok(())
            }
            TxSubcommand::Assemble {
                tx_body_file,
                witness_file,
                out_file,
            } => {
                let body_content = std::fs::read_to_string(&tx_body_file)?;
                let body_env: serde_json::Value = serde_json::from_str(&body_content)?;
                let body_cbor_hex = body_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx body"))?;
                let body_cbor = hex::decode(body_cbor_hex)?;

                // Collect all witnesses
                let mut vkey_witnesses: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                for wf in &witness_file {
                    let w_content = std::fs::read_to_string(wf)?;
                    let w_env: serde_json::Value = serde_json::from_str(&w_content)?;
                    let w_cbor_hex = w_env["cborHex"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in witness"))?;
                    let w_cbor = hex::decode(w_cbor_hex)?;

                    let mut decoder = minicbor::Decoder::new(&w_cbor);
                    let _ = decoder.array()?;
                    let vkey = decoder.bytes()?.to_vec();
                    let sig = decoder.bytes()?.to_vec();
                    vkey_witnesses.push((vkey, sig));
                }

                // Build signed tx: [body, witness_set, true, null]
                let mut tx_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut tx_cbor);
                enc.array(4)?;
                // Write body as raw CBOR
                tx_cbor.extend_from_slice(&body_cbor);
                // Witness set: {0: [[vkey, sig], ...]}
                let mut ws_buf = Vec::new();
                let mut ws_enc = minicbor::Encoder::new(&mut ws_buf);
                ws_enc.map(1)?;
                ws_enc.u32(0)?; // vkey witnesses
                ws_enc.array(vkey_witnesses.len() as u64)?;
                for (vkey, sig) in &vkey_witnesses {
                    ws_enc.array(2)?;
                    ws_enc.bytes(vkey)?;
                    ws_enc.bytes(sig)?;
                }
                tx_cbor.extend_from_slice(&ws_buf);
                // is_valid: true
                let mut valid_buf = Vec::new();
                minicbor::Encoder::new(&mut valid_buf).bool(true)?;
                tx_cbor.extend_from_slice(&valid_buf);
                // auxiliary_data: include if present in tx body envelope, otherwise null
                if let Some(aux_hex) = body_env
                    .get("auxiliaryDataCborHex")
                    .and_then(|v| v.as_str())
                {
                    let aux_cbor = hex::decode(aux_hex)?;
                    tx_cbor.extend_from_slice(&aux_cbor);
                } else {
                    let mut null_buf = Vec::new();
                    minicbor::Encoder::new(&mut null_buf).null()?;
                    tx_cbor.extend_from_slice(&null_buf);
                }

                let tx_env = serde_json::json!({
                    "type": "Witnessed Tx ConwayEra",
                    "description": "",
                    "cborHex": hex::encode(&tx_cbor)
                });
                std::fs::write(&out_file, serde_json::to_string_pretty(&tx_env)?)?;

                let hash = torsten_crypto::signing::hash_transaction(&body_cbor);
                println!("Signed transaction assembled.");
                println!("Transaction ID: {hash}");
                println!("Output: {}", out_file.display());
                Ok(())
            }
            TxSubcommand::Policyid { script_file } => {
                let content = std::fs::read_to_string(&script_file)?;
                let json: serde_json::Value = serde_json::from_str(&content)?;

                let native_script = parse_json_native_script(&json)?;
                let script_cbor =
                    torsten_serialization::encode::encode_native_script(&native_script);
                // Script hash = blake2b_224(prefix_tag || cbor_bytes)
                // Native script prefix tag = 0x00 (PlutusV1=0x01, V2=0x02, V3=0x03)
                let mut hash_input = vec![0x00];
                hash_input.extend_from_slice(&script_cbor);
                let hash = torsten_primitives::hash::blake2b_224(&hash_input);
                println!("{}", hash.to_hex());
                Ok(())
            }
        }
    }
}

/// Extract the transaction body CBOR from a signed transaction
/// Signed tx is: [body, witnesses, valid, aux] - we need the body element
fn extract_tx_body(cbor: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = minicbor::Decoder::new(cbor);
    let _ = decoder
        .array()
        .map_err(|e| anyhow::anyhow!("Invalid signed tx CBOR: {e}"))?;

    // Record position before the body
    let body_start = decoder.position();
    decoder
        .skip()
        .map_err(|e| anyhow::anyhow!("Cannot skip tx body: {e}"))?;
    let body_end = decoder.position();

    Ok(cbor[body_start..body_end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tx_input_valid() {
        let hash_hex = "a".repeat(64);
        let input = format!("{hash_hex}#0");
        let (hash, index) = parse_tx_input(&input).unwrap();
        assert_eq!(hash, Hash32::from_bytes([0xaa; 32]));
        assert_eq!(index, 0);
    }

    #[test]
    fn test_parse_tx_input_invalid_format() {
        assert!(parse_tx_input("invalid").is_err());
    }

    #[test]
    fn test_parse_tx_output_valid() {
        let output = parse_tx_output("addr_test1abc+5000000").unwrap();
        assert_eq!(output.address, "addr_test1abc");
        assert_eq!(output.lovelace, 5000000);
        assert!(output.tokens.is_empty());
    }

    #[test]
    fn test_parse_tx_output_with_tokens() {
        let policy = "a".repeat(56);
        let s = format!("addr_test1abc+5000000+\"{policy}.deadbeef 100\"");
        let output = parse_tx_output(&s).unwrap();
        assert_eq!(output.address, "addr_test1abc");
        assert_eq!(output.lovelace, 5000000);
        assert_eq!(output.tokens.len(), 1);
        assert_eq!(output.tokens[0].0, policy);
        assert_eq!(output.tokens[0].1, "deadbeef");
        assert_eq!(output.tokens[0].2, 100);
    }

    #[test]
    fn test_parse_tx_output_invalid() {
        assert!(parse_tx_output("no_plus_sign").is_err());
    }

    #[test]
    fn test_build_tx_body_cbor() {
        let inputs = vec![(Hash32::from_bytes([0xab; 32]), 0)];
        let outputs = vec![];
        let result = build_tx_body_cbor(
            &inputs,
            &outputs,
            200000,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_tx_body_with_multi_asset() {
        let inputs = vec![(Hash32::from_bytes([0xab; 32]), 0)];
        let policy = "a".repeat(56);
        let outputs = vec![ParsedTxOutput {
            address: bech32::encode::<bech32::Bech32>(
                bech32::Hrp::parse("addr_test").unwrap(),
                &[0x00; 57],
            )
            .unwrap(),
            lovelace: 2_000_000,
            tokens: vec![(policy, "deadbeef".to_string(), 100)],
        }];
        let result = build_tx_body_cbor(
            &inputs,
            &outputs,
            200000,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_tx_body_with_certificates() {
        let inputs = vec![(Hash32::from_bytes([0xab; 32]), 0)];
        let outputs = vec![];
        // A simple cert CBOR: [0, [0, bytes(28)]]
        let cert = vec![0x82, 0x00, 0x82, 0x00, 0x58, 0x1c];
        let certs = vec![cert];
        let result = build_tx_body_cbor(
            &inputs,
            &outputs,
            200000,
            None,
            &certs,
            &[],
            None,
            &[],
            &[],
            &[],
        );
        assert!(result.is_ok());
        let cbor = result.unwrap();
        // Should have 4 fields: inputs, outputs, fee, certificates
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.map().unwrap(), Some(4));
    }

    #[test]
    fn test_parse_withdrawal() {
        // Build a valid stake address for testing
        let key_hash = [0xab; 28];
        let mut addr_bytes = vec![0xe0u8]; // testnet reward addr
        addr_bytes.extend_from_slice(&key_hash);
        let addr = bech32::encode::<bech32::Bech32>(
            bech32::Hrp::parse("stake_test").unwrap(),
            &addr_bytes,
        )
        .unwrap();

        let input = format!("{addr}+1000000");
        let result = parse_withdrawal(&input);
        assert!(result.is_ok());
        let (decoded_addr, amount) = result.unwrap();
        assert_eq!(decoded_addr, addr_bytes);
        assert_eq!(amount, 1000000);
    }

    #[test]
    fn test_parse_withdrawal_invalid() {
        assert!(parse_withdrawal("no_plus").is_err());
    }

    #[test]
    fn test_encode_metadata_value() {
        let json: serde_json::Value = serde_json::json!({
            "674": { "msg": ["Hello, Cardano!"] }
        });
        let result = build_auxiliary_data(&json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_tx_body() {
        // Build a simple array: [map{}, map{}, true, null]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.map(0).unwrap(); // body
        enc.map(0).unwrap(); // witnesses
        enc.bool(true).unwrap();
        enc.null().unwrap();

        let body = extract_tx_body(&buf).unwrap();
        // Body should be the CBOR for map(0) = 0xa0
        assert_eq!(body, vec![0xa0]);
    }

    #[test]
    fn test_parse_mint_args_single_mint() {
        let policy = "a".repeat(56); // 28-byte policy ID
        let args = vec![format!("{policy}.deadbeef+100")];
        let result = parse_mint_args(&args).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, hex::decode(&policy).unwrap());
        assert_eq!(result[0].1.len(), 1);
        assert_eq!(result[0].1[0].0, hex::decode("deadbeef").unwrap());
        assert_eq!(result[0].1[0].1, 100);
    }

    #[test]
    fn test_parse_mint_args_burn() {
        let policy = "b".repeat(56);
        let args = vec![format!("{policy}.cafe-50")];
        let result = parse_mint_args(&args).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1[0].1, -50);
    }

    #[test]
    fn test_parse_mint_args_multiple_assets_same_policy() {
        let policy = "c".repeat(56);
        let args = vec![format!("{policy}.aabb+200"), format!("{policy}.ccdd+300")];
        let result = parse_mint_args(&args).unwrap();
        assert_eq!(result.len(), 1); // grouped under same policy
        assert_eq!(result[0].1.len(), 2);
        assert_eq!(result[0].1[0].1, 200);
        assert_eq!(result[0].1[1].1, 300);
    }

    #[test]
    fn test_parse_mint_args_no_asset_name() {
        let policy = "d".repeat(56);
        let args = vec![format!("{policy}+1000")];
        let result = parse_mint_args(&args).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].1[0].0.is_empty()); // empty asset name
        assert_eq!(result[0].1[0].1, 1000);
    }

    #[test]
    fn test_parse_mint_args_invalid_format() {
        let args = vec!["noplusorminussign".to_string()];
        assert!(parse_mint_args(&args).is_err());
    }

    #[test]
    fn test_parse_mint_args_invalid_hex() {
        let args = vec!["gggg.aabb+100".to_string()];
        assert!(parse_mint_args(&args).is_err());
    }

    #[test]
    fn test_parse_json_native_script_sig() {
        let json = serde_json::json!({
            "type": "sig",
            "keyHash": "e09d36c79dec9bd1b3d9e152247701cd0bb860b5ebfd1de8ece6f3d3"
        });
        let script = parse_json_native_script(&json).unwrap();
        match script {
            torsten_primitives::transaction::NativeScript::ScriptPubkey(hash) => {
                let expected =
                    hex::decode("e09d36c79dec9bd1b3d9e152247701cd0bb860b5ebfd1de8ece6f3d3")
                        .unwrap();
                assert_eq!(&hash.as_ref()[..28], &expected[..]);
            }
            _ => panic!("Expected ScriptPubkey"),
        }
    }

    #[test]
    fn test_parse_json_native_script_all() {
        let json = serde_json::json!({
            "type": "all",
            "scripts": [
                { "type": "sig", "keyHash": "a".repeat(56) },
                { "type": "after", "slot": 1000 }
            ]
        });
        let script = parse_json_native_script(&json).unwrap();
        match script {
            torsten_primitives::transaction::NativeScript::ScriptAll(scripts) => {
                assert_eq!(scripts.len(), 2);
            }
            _ => panic!("Expected ScriptAll"),
        }
    }

    #[test]
    fn test_parse_json_native_script_time_locks() {
        let json = serde_json::json!({ "type": "after", "slot": 500 });
        let script = parse_json_native_script(&json).unwrap();
        assert_eq!(
            script,
            torsten_primitives::transaction::NativeScript::InvalidBefore(
                torsten_primitives::time::SlotNo(500)
            )
        );

        let json = serde_json::json!({ "type": "before", "slot": 999 });
        let script = parse_json_native_script(&json).unwrap();
        assert_eq!(
            script,
            torsten_primitives::transaction::NativeScript::InvalidHereafter(
                torsten_primitives::time::SlotNo(999)
            )
        );
    }

    #[test]
    fn test_parse_json_native_script_at_least() {
        let json = serde_json::json!({
            "type": "atLeast",
            "required": 2,
            "scripts": [
                { "type": "sig", "keyHash": "a".repeat(56) },
                { "type": "sig", "keyHash": "b".repeat(56) },
                { "type": "sig", "keyHash": "c".repeat(56) }
            ]
        });
        let script = parse_json_native_script(&json).unwrap();
        match script {
            torsten_primitives::transaction::NativeScript::ScriptNOfK(n, scripts) => {
                assert_eq!(n, 2);
                assert_eq!(scripts.len(), 3);
            }
            _ => panic!("Expected ScriptNOfK"),
        }
    }

    #[test]
    fn test_parse_json_native_script_invalid() {
        let json = serde_json::json!({ "type": "unknown" });
        assert!(parse_json_native_script(&json).is_err());

        let json = serde_json::json!({ "no_type": true });
        assert!(parse_json_native_script(&json).is_err());
    }

    #[test]
    fn test_policyid_cbor_hash() {
        // A simple sig script — verify it matches cardano-cli output
        let json = serde_json::json!({
            "type": "sig",
            "keyHash": "e09d36c79dec9bd1b3d9e152247701cd0bb860b5ebfd1de8ece6f3d3"
        });
        let script = parse_json_native_script(&json).unwrap();
        let cbor = torsten_serialization::encode::encode_native_script(&script);

        // The CBOR should be: [0, h'e09d36c79dec9bd1b3d9e152247701cd0bb860b5ebfd1de8ece6f3d3']
        assert_eq!(cbor[0], 0x82); // array(2)
        assert_eq!(cbor[1], 0x00); // uint(0)
        assert_eq!(cbor[2], 0x58); // bytes(28) - 1-byte length prefix
        assert_eq!(cbor[3], 0x1c); // 28
        assert_eq!(cbor.len(), 4 + 28); // header + 28-byte hash

        // Script hash = blake2b_224(0x00 || cbor) — must include native script prefix tag
        let mut hash_input = vec![0x00];
        hash_input.extend_from_slice(&cbor);
        let hash = torsten_primitives::hash::blake2b_224(&hash_input);

        // Verified against cardano-cli 10.15.0:
        // cardano-cli hash script --script-file <(echo '{"type":"sig","keyHash":"e09d..."}')
        assert_eq!(
            hash.to_hex(),
            "9574ec47eece19ba26900b524f9945d69a28df4fb386522365bf342d"
        );
    }
}
