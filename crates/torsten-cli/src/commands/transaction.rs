use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::transaction::{ExUnits, PlutusData, Redeemer, RedeemerTag};

/// Multi-asset token map: `(policy_id_bytes, asset_name_bytes) → quantity`.
///
/// Used throughout the auto-balance pipeline to accumulate and subtract token
/// bundles from input and output UTxOs.
type MultiAssetMap = HashMap<(Vec<u8>, Vec<u8>), u64>;

#[derive(Args, Debug)]
pub struct TransactionCmd {
    #[command(subcommand)]
    command: TxSubcommand,
}

/// Arguments shared between `transaction build` and `transaction build-raw`.
///
/// Both subcommands accept the exact same flags and produce the same output.
/// `build-raw` exists for cardano-cli naming compatibility — many downstream
/// scripts and tools invoke `cardano-cli transaction build-raw` specifically.
///
/// Auto-balance mode: when `--socket-path` is provided and `--fee` is NOT
/// explicitly set, the command connects to the node, queries UTxO values for
/// the given inputs and the current protocol parameters, computes the fee
/// automatically, derives a change output, and writes a balanced transaction.
/// If `--fee` IS set the old manual-balance behaviour is preserved.
#[derive(Args, Debug)]
struct BuildArgs {
    /// Transaction inputs (format: tx_hash#index)
    #[arg(long, num_args = 1..)]
    tx_in: Vec<String>,
    /// Transaction outputs (format: address+amount)
    #[arg(long, num_args = 1..)]
    tx_out: Vec<String>,
    /// Change address (required for auto-balance mode)
    #[arg(long)]
    change_address: Option<String>,
    /// Fee amount in lovelace.
    ///
    /// When omitted and `--socket-path` is set the fee is computed
    /// automatically from protocol parameters (auto-balance mode).
    /// When omitted without `--socket-path` a default of 200 000 lovelace
    /// is used (offline / build-raw compatible behaviour).
    #[arg(long)]
    fee: Option<u64>,
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
    /// Plutus script file to attach to the most recently specified --tx-in.
    ///
    /// Must be a text-envelope file whose `type` begins with "PlutusScriptV1",
    /// "PlutusScriptV2", or "PlutusScriptV3".  Each occurrence is matched to
    /// the `--tx-in` that immediately precedes it on the command line by
    /// position (both lists are iterated in declaration order by clap, so the
    /// i-th `--tx-in-script-file` is paired with the i-th `--tx-in`).
    #[arg(long)]
    tx_in_script_file: Vec<PathBuf>,
    /// Datum JSON file to attach to the script-bearing input at the same
    /// position.  Uses the cardano-cli JSON PlutusData schema
    /// (`{"int": N}` / `{"bytes": "hex"}` / `{"list": [...]}` /
    ///  `{"map": [...]}` / `{"constructor": N, "fields": [...]}`).
    #[arg(long)]
    tx_in_datum_file: Vec<PathBuf>,
    /// Redeemer JSON file for the script-bearing input at the same position.
    /// Same JSON schema as `--tx-in-datum-file`.
    #[arg(long)]
    tx_in_redeemer_file: Vec<PathBuf>,
    /// Execution units budget for the script-bearing input at the same
    /// position.  Format: `mem,steps`  (e.g. `1000000,500000000`).
    #[arg(long)]
    tx_in_execution_units: Vec<String>,
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
    // ── Node connection args (used for auto-balance) ──────────────────────────
    /// Path to the node's Unix domain socket.
    ///
    /// When provided together with no explicit `--fee`, the command performs
    /// automatic fee computation and change calculation (auto-balance mode).
    #[arg(long)]
    socket_path: Option<PathBuf>,
    /// Use mainnet (network magic 764824073)
    #[arg(long)]
    mainnet: bool,
    /// Testnet network magic (e.g. 2 for preview, 1 for preprod)
    #[arg(long)]
    testnet_magic: Option<u64>,
}

#[derive(Subcommand, Debug)]
enum TxSubcommand {
    /// Build a transaction body (alias: build-raw)
    Build(BuildArgs),
    /// Build a transaction body — cardano-cli compatible alias for `build`
    ///
    /// Accepts the same arguments as `transaction build` and produces identical
    /// output. Provided for compatibility with scripts that call
    /// `cardano-cli transaction build-raw`.
    BuildRaw(BuildArgs),
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
        /// Protocol parameters file (JSON output of `query protocol-parameters`)
        #[arg(long)]
        protocol_params_file: PathBuf,
        /// Number of transaction inputs (cardano-cli compatibility flag; ignored when
        /// the tx body file is present since we measure the actual body size)
        #[arg(long)]
        tx_in_count: Option<u64>,
        /// Number of transaction outputs (cardano-cli compatibility flag; ignored
        /// when the tx body file is present)
        #[arg(long)]
        tx_out_count: Option<u64>,
    },
    /// Calculate the minimum lovelace required for a UTxO output
    ///
    /// Uses the Babbage/Conway formula:
    ///   min_ada = max(1_000_000, coinsPerUTxOByte * (output_cbor_size + 160))
    ///
    /// Reads `coinsPerUTxOByte` (or `utxoCostPerByte`) from the protocol params
    /// JSON and estimates the serialised output size from the --tx-out value spec.
    CalculateMinRequiredUtxo {
        /// Protocol parameters file (JSON output of `query protocol-parameters`)
        #[arg(long)]
        protocol_params_file: PathBuf,
        /// Output value specification in cardano-cli format: `address+lovelace` or
        /// `address+lovelace+"policy.asset qty+..."` for multi-asset outputs.
        #[arg(long = "tx-out")]
        tx_out: String,
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
#[derive(Clone, Debug)]
pub(crate) struct ParsedTxOutput {
    pub(crate) address: String,
    pub(crate) lovelace: u64,
    /// Native tokens: Vec<(policy_hex, asset_name_hex, quantity)>
    pub(crate) tokens: Vec<(String, String, u64)>,
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
    script_data_hash: Option<&Hash32>,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // Transaction body map fields:
    // 0: inputs, 1: outputs, 2: fee, 3: ttl, 4: certificates, 5: withdrawals,
    // 7: auxiliary_data_hash, 9: mint, 11: script_data_hash,
    // 13: collateral, 14: required_signers
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
    if script_data_hash.is_some() {
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

    // Field 11: script_data_hash (optional — present when Plutus scripts are used)
    if let Some(hash) = script_data_hash {
        enc.u32(11)?;
        enc.bytes(hash.as_bytes())?;
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

// ─── Plutus script witness support ─────────────────────────────────────────

/// Plutus language version extracted from a text-envelope `type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlutusVersion {
    V1,
    V2,
    V3,
}

/// All the information needed to build the Plutus witness for one script input.
#[derive(Debug, Clone)]
pub(crate) struct ScriptWitness {
    /// Language version of the script.
    pub(crate) version: PlutusVersion,
    /// Raw flat-encoded script bytes (the `cborHex` payload from the text envelope
    /// is itself CBOR-wrapped bytes; we store the inner bytes here).
    pub(crate) script_bytes: Vec<u8>,
    /// CBOR encoding of the datum `PlutusData` for this input.
    pub(crate) datum_cbor: Vec<u8>,
    /// CBOR encoding of the redeemer `PlutusData` for this input.
    pub(crate) redeemer_data_cbor: Vec<u8>,
    /// Execution units budget.
    pub(crate) ex_units: ExUnits,
}

/// Load a Plutus script from a cardano-cli text-envelope file.
///
/// The envelope `type` field must start with "PlutusScriptV1", "PlutusScriptV2",
/// or "PlutusScriptV3".  The `cborHex` field is the double-CBOR encoding used
/// by cardano-cli: the outer CBOR wraps a byte-string whose content is the
/// flat-encoded script bytes.  We strip the outer CBOR byte-string wrapper and
/// return the inner bytes together with the version.
pub(crate) fn load_plutus_script(path: &PathBuf) -> Result<(PlutusVersion, Vec<u8>)> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read script file '{}': {e}", path.display()))?;
    let env: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid JSON in script file '{}': {e}", path.display()))?;

    let type_str = env["type"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'type' field in '{}'", path.display()))?;

    let version = if type_str.starts_with("PlutusScriptV1") {
        PlutusVersion::V1
    } else if type_str.starts_with("PlutusScriptV2") {
        PlutusVersion::V2
    } else if type_str.starts_with("PlutusScriptV3") {
        PlutusVersion::V3
    } else {
        bail!(
            "Unsupported script type '{}' in '{}'. \
             Expected PlutusScriptV1, PlutusScriptV2, or PlutusScriptV3.",
            type_str,
            path.display()
        );
    };

    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'cborHex' in '{}'", path.display()))?;
    let outer_cbor = hex::decode(cbor_hex)
        .map_err(|e| anyhow::anyhow!("Invalid hex in 'cborHex' of '{}': {e}", path.display()))?;

    // cardano-cli wraps the flat script in a CBOR byte-string (#6.XX(bytes(...))).
    // We may have:
    //   - Just bytes(N): the flat bytes directly
    //   - tag(N) bytes(N): tagged flat bytes (some versions wrap with a short tag)
    // Unwrap one level of CBOR to get the raw script bytes.
    let script_bytes = cbor_unwrap_bytes(&outer_cbor).unwrap_or(outer_cbor);

    Ok((version, script_bytes))
}

/// Strip a CBOR byte-string (or tag + byte-string) wrapper from `data` and
/// return the contained bytes.  Returns `None` if the outermost item is not
/// a byte-string or a single-tag wrapping a byte-string.
fn cbor_unwrap_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = minicbor::Decoder::new(data);
    // Try to consume an optional CBOR tag (major type 6) without advancing the
    // position if the next item is not a tag.  We save the position first so
    // we can reset if `tag()` fails or if the item after the tag is not bytes.
    let pos_before = dec.position();

    let has_tag = matches!(dec.datatype(), Ok(minicbor::data::Type::Tag));
    if has_tag {
        // Consume the tag and fall through to the byte-string read below.
        let _ = dec.tag();
    } else {
        // No tag — reset in case `datatype()` moved the cursor (it shouldn't,
        // but be defensive).
        dec.set_position(pos_before);
    }

    if let Ok(b) = dec.bytes() {
        return Some(b.to_vec());
    }
    None
}

/// Parse a cardano-cli PlutusData JSON value into `PlutusData`.
///
/// Supported schemas (matching cardano-cli / cardano-api):
/// - `{"int": <number>}`  →  `PlutusData::Integer`
/// - `{"bytes": "<hex>"}` →  `PlutusData::Bytes`
/// - `{"list": [...]}`    →  `PlutusData::List`
/// - `{"map": [{"k": ..., "v": ...}, ...]}` → `PlutusData::Map`
/// - `{"constructor": <N>, "fields": [...]}` → `PlutusData::Constr`
pub(crate) fn parse_plutus_data_json(json: &serde_json::Value) -> Result<PlutusData> {
    if let Some(n) = json.get("int") {
        let i = n
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("PlutusData 'int' must be an integer, got: {n}"))?;
        return Ok(PlutusData::Integer(i as i128));
    }

    if let Some(b) = json.get("bytes") {
        let hex_str = b
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("PlutusData 'bytes' must be a hex string"))?;
        let bytes = hex::decode(hex_str)
            .map_err(|e| anyhow::anyhow!("Invalid hex in PlutusData 'bytes': {e}"))?;
        return Ok(PlutusData::Bytes(bytes));
    }

    if let Some(list) = json.get("list") {
        let arr = list
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("PlutusData 'list' must be an array"))?;
        let items = arr
            .iter()
            .map(parse_plutus_data_json)
            .collect::<Result<Vec<_>>>()?;
        return Ok(PlutusData::List(items));
    }

    if let Some(map) = json.get("map") {
        let arr = map
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("PlutusData 'map' must be an array of {{k,v}} pairs"))?;
        let mut entries = Vec::with_capacity(arr.len());
        for entry in arr {
            let k = entry
                .get("k")
                .ok_or_else(|| anyhow::anyhow!("PlutusData map entry missing 'k'"))?;
            let v = entry
                .get("v")
                .ok_or_else(|| anyhow::anyhow!("PlutusData map entry missing 'v'"))?;
            entries.push((parse_plutus_data_json(k)?, parse_plutus_data_json(v)?));
        }
        return Ok(PlutusData::Map(entries));
    }

    if let Some(ctor) = json.get("constructor") {
        let tag = ctor.as_u64().ok_or_else(|| {
            anyhow::anyhow!("PlutusData 'constructor' must be a non-negative integer")
        })?;
        let fields_json = json
            .get("fields")
            .and_then(|f| f.as_array())
            .ok_or_else(|| anyhow::anyhow!("PlutusData constructor missing 'fields' array"))?;
        let fields = fields_json
            .iter()
            .map(parse_plutus_data_json)
            .collect::<Result<Vec<_>>>()?;
        return Ok(PlutusData::Constr(tag, fields));
    }

    bail!(
        "Unrecognised PlutusData JSON schema. Expected one of: \
         {{\"int\": N}}, {{\"bytes\": \"hex\"}}, {{\"list\": [...]}}, \
         {{\"map\": [...]}}, {{\"constructor\": N, \"fields\": [...]}}.  \
         Got: {json}"
    )
}

/// Load and parse a PlutusData JSON file.
pub(crate) fn load_plutus_data_file(path: &PathBuf) -> Result<PlutusData> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!("Cannot read datum/redeemer file '{}': {e}", path.display())
    })?;
    let json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Invalid JSON in '{}': {e}", path.display()))?;
    parse_plutus_data_json(&json).map_err(|e| anyhow::anyhow!("In '{}': {e}", path.display()))
}

/// Parse a `--tx-in-execution-units` string like `"1000000,500000000"`.
pub(crate) fn parse_execution_units(s: &str) -> Result<ExUnits> {
    let (mem_str, steps_str) = s
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("Invalid execution units '{s}'. Expected mem,steps"))?;
    let mem = mem_str
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid mem in execution units '{s}': {e}"))?;
    let steps = steps_str
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("Invalid steps in execution units '{s}': {e}"))?;
    Ok(ExUnits { mem, steps })
}

/// Encode a single `Redeemer` to CBOR `[tag, index, data, [mem, steps]]`.
///
/// Self-contained implementation that avoids depending on the `pub(crate)`
/// helper in `torsten-serialization`, keeping the CLI crate independent.
fn encode_redeemer_to_cbor(r: &Redeemer) -> Vec<u8> {
    // Redeemer = [tag, index, data, ex_units]
    // Tag: Spend=0, Mint=1, Cert=2, Reward=3, Vote=4, Propose=5
    let tag_num: u64 = match r.tag {
        RedeemerTag::Spend => 0,
        RedeemerTag::Mint => 1,
        RedeemerTag::Cert => 2,
        RedeemerTag::Reward => 3,
        RedeemerTag::Vote => 4,
        RedeemerTag::Propose => 5,
    };

    // Build header (array(4), tag, index) in a block so the encoder borrow ends
    // before we extend with the data and ex_units bytes.
    let mut header = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut header);
        enc.array(4).unwrap();
        enc.u64(tag_num).unwrap();
        enc.u64(r.index as u64).unwrap();
    }

    // ex_units: [mem, steps]
    let mut ex_units = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut ex_units);
        enc.array(2).unwrap();
        enc.u64(r.ex_units.mem).unwrap();
        enc.u64(r.ex_units.steps).unwrap();
    }

    // Concatenate: header || data_cbor || ex_units
    let mut buf = header;
    buf.extend_from_slice(&encode_plutus_data_to_cbor(&r.data));
    buf.extend_from_slice(&ex_units);
    buf
}

/// Encode a `PlutusData` value to CBOR using the Cardano wire encoding.
///
/// Self-contained implementation that mirrors `torsten_serialization::cbor::encode_plutus_data`
/// without depending on it, keeping the CLI crate independent of internal
/// serialization helpers.
fn encode_plutus_data_to_cbor(data: &PlutusData) -> Vec<u8> {
    match data {
        PlutusData::Integer(n) => {
            let n = *n;
            if n >= 0 {
                // Non-negative: CBOR major type 0 (unsigned)
                if n <= u64::MAX as i128 {
                    let mut buf = Vec::new();
                    minicbor::Encoder::new(&mut buf).u64(n as u64).unwrap();
                    buf
                } else {
                    // Positive bignum: tag(2) + bytes(big-endian)
                    let bytes = (n as u128).to_be_bytes();
                    let start = bytes.iter().position(|&b| b != 0).unwrap_or(15);
                    let mut buf = Vec::new();
                    {
                        let mut enc = minicbor::Encoder::new(&mut buf);
                        enc.tag(minicbor::data::Tag::new(2)).unwrap();
                        enc.bytes(&bytes[start..]).unwrap();
                    }
                    buf
                }
            } else {
                // Negative: CBOR major type 1
                if n >= i64::MIN as i128 {
                    let mut buf = Vec::new();
                    minicbor::Encoder::new(&mut buf).i64(n as i64).unwrap();
                    buf
                } else {
                    // Negative bignum: tag(3) + bytes(big-endian of -(1+n))
                    let abs = (-(1 + n)) as u128;
                    let bytes = abs.to_be_bytes();
                    let start = bytes.iter().position(|&b| b != 0).unwrap_or(15);
                    let mut buf = Vec::new();
                    {
                        let mut enc = minicbor::Encoder::new(&mut buf);
                        enc.tag(minicbor::data::Tag::new(3)).unwrap();
                        enc.bytes(&bytes[start..]).unwrap();
                    }
                    buf
                }
            }
        }
        PlutusData::Bytes(b) => {
            let mut buf = Vec::new();
            minicbor::Encoder::new(&mut buf).bytes(b).unwrap();
            buf
        }
        PlutusData::List(items) => {
            let mut header = Vec::new();
            minicbor::Encoder::new(&mut header)
                .array(items.len() as u64)
                .unwrap();
            let mut buf = header;
            for item in items {
                buf.extend_from_slice(&encode_plutus_data_to_cbor(item));
            }
            buf
        }
        PlutusData::Map(entries) => {
            let mut header = Vec::new();
            minicbor::Encoder::new(&mut header)
                .map(entries.len() as u64)
                .unwrap();
            let mut buf = header;
            for (k, v) in entries {
                buf.extend_from_slice(&encode_plutus_data_to_cbor(k));
                buf.extend_from_slice(&encode_plutus_data_to_cbor(v));
            }
            buf
        }
        PlutusData::Constr(tag, fields) => {
            // Small constructors 0–6: CBOR tag 121+n
            // Constructors 7–127: CBOR tag 1280+(n-7)
            // Anything else: CBOR tag 102 with [constructor, fields]
            let (cbor_tag, wrap_in_pair): (u64, bool) = if *tag < 7 {
                (121 + tag, false)
            } else if *tag < 128 {
                (1280 + (tag - 7), false)
            } else {
                (102, true)
            };

            let mut header = Vec::new();
            {
                let mut enc = minicbor::Encoder::new(&mut header);
                enc.tag(minicbor::data::Tag::new(cbor_tag)).unwrap();
                if wrap_in_pair {
                    // General form: tag(102) [constructor_index, [fields...]]
                    enc.array(2).unwrap();
                    enc.u64(*tag).unwrap();
                }
                enc.array(fields.len() as u64).unwrap();
            }
            let mut buf = header;
            for f in fields {
                buf.extend_from_slice(&encode_plutus_data_to_cbor(f));
            }
            buf
        }
    }
}

/// Compute the `script_data_hash` field (tx body key 11).
///
/// Per the Cardano spec (Alonzo+), this is:
/// ```text
/// script_data_hash = blake2b_256(
///     redeemers_cbor         -- array of redeemer; empty array if no redeemers
///     || datums_cbor          -- array of PlutusData; empty array if no datums
///     || language_views_cbor  -- map of language → cost model; empty map offline
/// )
/// ```
/// We use an empty language views map (`a0`) when building offline.  The node
/// re-validates this hash during submission using the actual cost models.
///
/// Returns `None` when no Plutus scripts are attached to the transaction.
pub(crate) fn compute_script_data_hash_offline(witnesses: &[ScriptWitness]) -> Option<Hash32> {
    if witnesses.is_empty() {
        return None;
    }

    // Redeemers: one per witness, indexed by witness position (= sorted tx-input position)
    let redeemers: Vec<Redeemer> = witnesses
        .iter()
        .enumerate()
        .map(|(idx, w)| {
            let data =
                decode_plutus_data_cbor(&w.redeemer_data_cbor).unwrap_or(PlutusData::Bytes(vec![]));
            Redeemer {
                tag: RedeemerTag::Spend,
                index: idx as u32,
                data,
                ex_units: w.ex_units,
            }
        })
        .collect();

    // Encode redeemers as an array
    let mut redeemer_bytes = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut redeemer_bytes);
        enc.array(redeemers.len() as u64).unwrap();
    }
    for r in &redeemers {
        redeemer_bytes.extend_from_slice(&encode_redeemer_to_cbor(r));
    }

    // Encode datums as an array
    let mut datum_bytes = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut datum_bytes);
        enc.array(witnesses.len() as u64).unwrap();
    }
    for w in witnesses {
        datum_bytes.extend_from_slice(&w.datum_cbor);
    }

    // Empty language views map: `a0`
    let language_views: Vec<u8> = vec![0xa0];

    let mut preimage = Vec::new();
    preimage.extend_from_slice(&redeemer_bytes);
    preimage.extend_from_slice(&datum_bytes);
    preimage.extend_from_slice(&language_views);

    Some(torsten_primitives::hash::blake2b_256(&preimage))
}

/// Build the Plutus witness set CBOR for inclusion in the signed transaction.
///
/// Returns the raw CBOR bytes of a witness-set *map* containing only the
/// Plutus-related keys (3/6/7 for scripts, 4 for datums, 5 for redeemers).
/// The caller merges these with vkey witness key 0 when assembling the
/// final signed transaction.
pub(crate) fn build_plutus_witness_set_cbor(witnesses: &[ScriptWitness]) -> Vec<u8> {
    if witnesses.is_empty() {
        // Empty map — no Plutus witnesses.
        return vec![0xa0];
    }

    // Collect scripts by version (maintaining declaration order for consistent hashing)
    let v1_scripts: Vec<&[u8]> = witnesses
        .iter()
        .filter(|w| w.version == PlutusVersion::V1)
        .map(|w| w.script_bytes.as_slice())
        .collect();
    let v2_scripts: Vec<&[u8]> = witnesses
        .iter()
        .filter(|w| w.version == PlutusVersion::V2)
        .map(|w| w.script_bytes.as_slice())
        .collect();
    let v3_scripts: Vec<&[u8]> = witnesses
        .iter()
        .filter(|w| w.version == PlutusVersion::V3)
        .map(|w| w.script_bytes.as_slice())
        .collect();

    let datum_count = witnesses.len();
    let redeemer_count = witnesses.len();

    // Count witness-set map keys (in wire-format order: 3, 4, 5, 6, 7)
    let mut key_count = 0usize;
    if !v1_scripts.is_empty() {
        key_count += 1;
    }
    if datum_count > 0 {
        key_count += 1;
    }
    if redeemer_count > 0 {
        key_count += 1;
    }
    if !v2_scripts.is_empty() {
        key_count += 1;
    }
    if !v3_scripts.is_empty() {
        key_count += 1;
    }

    let mut buf = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.map(key_count as u64).unwrap();
    }

    // Key 3: PlutusV1 scripts — each script is bytes(flat_encoded_script)
    if !v1_scripts.is_empty() {
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.u32(3).unwrap();
        enc.array(v1_scripts.len() as u64).unwrap();
        for s in &v1_scripts {
            enc.bytes(s).unwrap();
        }
    }

    // Key 4: datums — array of PlutusData; each datum_cbor is a pre-encoded item
    if datum_count > 0 {
        {
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.u32(4).unwrap();
            enc.array(datum_count as u64).unwrap();
        }
        for w in witnesses {
            // datum_cbor is the encoding of one PlutusData item — inject raw bytes.
            buf.extend_from_slice(&w.datum_cbor);
        }
    }

    // Key 5: redeemers — array of [tag, index, data, ex_units]
    if redeemer_count > 0 {
        {
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.u32(5).unwrap();
            enc.array(redeemer_count as u64).unwrap();
        }
        for (idx, w) in witnesses.iter().enumerate() {
            let data =
                decode_plutus_data_cbor(&w.redeemer_data_cbor).unwrap_or(PlutusData::Bytes(vec![]));
            let r = Redeemer {
                tag: RedeemerTag::Spend,
                index: idx as u32,
                data,
                ex_units: w.ex_units,
            };
            buf.extend_from_slice(&encode_redeemer_to_cbor(&r));
        }
    }

    // Key 6: PlutusV2 scripts
    if !v2_scripts.is_empty() {
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.u32(6).unwrap();
        enc.array(v2_scripts.len() as u64).unwrap();
        for s in &v2_scripts {
            enc.bytes(s).unwrap();
        }
    }

    // Key 7: PlutusV3 scripts
    if !v3_scripts.is_empty() {
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.u32(7).unwrap();
        enc.array(v3_scripts.len() as u64).unwrap();
        for s in &v3_scripts {
            enc.bytes(s).unwrap();
        }
    }

    buf
}

/// Decode a single CBOR-encoded PlutusData item back into the `PlutusData` type.
///
/// Used when round-tripping pre-encoded CBOR through the redeemer builder.
/// On any decode failure returns `None`.
fn decode_plutus_data_cbor(cbor: &[u8]) -> Option<PlutusData> {
    // We use our own decode logic via minicbor to reconstruct PlutusData.
    decode_plutus_cbor_inner(&mut minicbor::Decoder::new(cbor))
}

/// Recursive CBOR PlutusData decoder (mirrors `encode_plutus_data`).
fn decode_plutus_cbor_inner(dec: &mut minicbor::Decoder<'_>) -> Option<PlutusData> {
    use minicbor::data::Type;
    match dec.datatype().ok()? {
        Type::Tag => {
            // Constr: tag 121+n (constructor 0–6) or 1280+n (7–127) or 102 general
            let tag = dec.tag().ok()?.as_u64();
            if tag == 102 {
                // General constructor: [index, fields]
                let _ = dec.array().ok()?;
                let index = dec.u64().ok()?;
                let arr_len = dec.array().ok()??;
                let mut fields = Vec::new();
                for _ in 0..arr_len {
                    fields.push(decode_plutus_cbor_inner(dec)?);
                }
                Some(PlutusData::Constr(index, fields))
            } else if (121..=127).contains(&tag) {
                let constructor = tag - 121;
                let arr_len = dec.array().ok()??;
                let mut fields = Vec::new();
                for _ in 0..arr_len {
                    fields.push(decode_plutus_cbor_inner(dec)?);
                }
                Some(PlutusData::Constr(constructor, fields))
            } else if tag >= 1280 {
                let constructor = tag - 1280 + 7;
                let arr_len = dec.array().ok()??;
                let mut fields = Vec::new();
                for _ in 0..arr_len {
                    fields.push(decode_plutus_cbor_inner(dec)?);
                }
                Some(PlutusData::Constr(constructor, fields))
            } else {
                None
            }
        }
        Type::Map => {
            let len = dec.map().ok()?? as usize;
            let mut entries = Vec::new();
            for _ in 0..len {
                let k = decode_plutus_cbor_inner(dec)?;
                let v = decode_plutus_cbor_inner(dec)?;
                entries.push((k, v));
            }
            Some(PlutusData::Map(entries))
        }
        Type::Array => {
            let len = dec.array().ok()?? as usize;
            let mut items = Vec::new();
            for _ in 0..len {
                items.push(decode_plutus_cbor_inner(dec)?);
            }
            Some(PlutusData::List(items))
        }
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
            let n = dec.u64().ok()?;
            Some(PlutusData::Integer(n as i128))
        }
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => {
            let n = dec.i64().ok()?;
            Some(PlutusData::Integer(n as i128))
        }
        Type::Bytes => {
            let b = dec.bytes().ok()?.to_vec();
            Some(PlutusData::Bytes(b))
        }
        _ => None,
    }
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

// ─── Fee estimation ────────────────────────────────────────────────────────

/// Estimate the fee for a transaction using the linear fee formula.
///
/// The Cardano fee formula is:
/// ```text
/// fee = min_fee_a * tx_size_bytes + min_fee_b
/// ```
/// where `tx_size_bytes` is the size of the *fully signed* transaction.
///
/// We estimate the signed tx size as:
/// - The serialised tx body CBOR length
/// - Plus per-witness overhead: each vkey witness is 2 + 32 (vkey) + 64 (sig)
///   bytes, plus CBOR array framing (~10 bytes) = 108 bytes total per witness.
/// - Plus the signed-tx envelope framing (array(4) header + bool + null ≈ 11 bytes)
/// - Plus the witness set map header (~3 bytes)
///
/// This matches the estimate used by `calculate-min-fee`.
pub(crate) fn estimate_fee(
    tx_body_cbor: &[u8],
    witness_count: u64,
    min_fee_a: u64,
    min_fee_b: u64,
) -> u64 {
    // Each vkey witness: array(2)[bytes(32), bytes(64)] ≈ 2+1+32+1+64 = 100 bytes + array2 header = 106
    let witness_overhead = witness_count * 106;
    // Signed tx envelope: array(4) = 1 byte; bool(true) = 1 byte; null = 1 byte; witness set map(1) = ~3 bytes
    let envelope_overhead: u64 = 11;
    let estimated_size = tx_body_cbor.len() as u64 + witness_overhead + envelope_overhead;
    min_fee_a * estimated_size + min_fee_b
}

/// Compute a change output to balance the transaction.
///
/// Compute the minimum lovelace required for a UTxO output that carries the
/// given native tokens.
///
/// The Babbage/Conway formula is:
/// ```text
/// min_ada = max(1_000_000, coins_per_utxo_byte * (output_size_bytes + 160))
/// ```
///
/// `output_size_bytes` is a conservative estimate of the serialised output
/// size derived from the token bundle contents:
/// - 8 bytes overhead for the CBOR array wrapper and coin field
/// - 28 bytes per policy ID
/// - 1 byte CBOR map entry overhead per policy
/// - length of asset name bytes per asset
/// - 8 bytes per asset for the quantity
/// - 1 byte CBOR overhead per asset entry
///
/// 160 is the constant overhead used by the Haskell implementation to account
/// for the key/value UTxO entry overhead in the ledger state.
pub(crate) fn min_ada_for_output(
    tokens: &[(String, String, u64)],
    coins_per_utxo_byte: u64,
) -> u64 {
    // Estimate the serialised byte size of the value field.
    //
    // When there are no tokens the output is a plain coin output; the minimum
    // is then the flat 1 ADA floor (no formula needed, just the floor).
    let output_size_bytes: u64 = if tokens.is_empty() {
        0
    } else {
        // Base: CBOR array(2) header (1 byte) + coin u64 (up to 9 bytes)
        let mut size: u64 = 10;
        // Group tokens by policy to mirror the actual CBOR grouping
        let mut policy_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for (policy, asset_name, _) in tokens {
            policy_map
                .entry(policy.as_str())
                .or_default()
                .push(asset_name.as_str());
        }
        for (policy_hex, assets) in &policy_map {
            // 28-byte policy ID bytes + 1-byte CBOR header
            size += 29;
            // Policy-ID hex length is always 56 chars → 28 bytes; ignore hex_len / 2
            let _ = policy_hex;
            // Per-asset: name bytes + 1-byte header + 8-byte quantity + 1 overhead
            for asset_name_hex in assets {
                size += (asset_name_hex.len() as u64) / 2 + 10;
            }
        }
        size
    };

    const LEDGER_KEY_OVERHEAD: u64 = 160;
    const FLAT_MIN: u64 = 1_000_000;
    let formula = coins_per_utxo_byte.saturating_mul(output_size_bytes + LEDGER_KEY_OVERHEAD);
    formula.max(FLAT_MIN)
}

/// Build the change output for auto-balance mode.
///
/// Computes:
/// - lovelace change = `total_inputs - total_outputs - fee`
/// - token change   = `input_tokens + positive_mints - burns - output_tokens`
///
/// The Cardano ledger requires exact token conservation for every
/// (policy, asset) pair:
/// ```text
/// Σ(input_tokens[asset]) + mint[asset] == Σ(output_tokens[asset])
/// ```
/// where `mint[asset]` is signed (positive = new tokens, negative = burn).
///
/// In auto-balance mode the change output is the implicit final output, so:
/// ```text
/// change[asset] = Σ(input_tokens[asset]) + mint[asset] - Σ(explicit_output_tokens[asset])
/// ```
///
/// A positive remainder must be placed in the change output.  A negative
/// remainder means the user is trying to output more tokens than are available
/// (from inputs plus mints), which is an error.
///
/// The minimum lovelace required to carry the token change bundle is checked
/// with [`min_ada_for_output`].  If there is insufficient lovelace, an error
/// is returned with a clear description of the shortfall.
///
/// Returns `Some(output)` when there is spendable change, or `None` when the
/// lovelace change is zero and there are no tokens.
///
/// Returns an error when:
/// - `total_inputs < total_outputs + fee` (insufficient funds),
/// - a token overdraft is detected (`output > input + mint`), or
/// - the token change requires more lovelace than is available as change.
#[allow(clippy::too_many_arguments)]
pub(crate) fn calculate_change(
    total_inputs: u64,
    total_outputs: u64,
    fee: u64,
    change_address: &str,
    input_tokens: &MultiAssetMap,
    output_tokens: &MultiAssetMap,
    mint: &[MintEntry],
    coins_per_utxo_byte: u64,
) -> Result<Option<ParsedTxOutput>> {
    let total_spent = total_outputs
        .checked_add(fee)
        .ok_or_else(|| anyhow::anyhow!("Arithmetic overflow computing total spent"))?;

    if total_inputs < total_spent {
        let shortfall = total_spent - total_inputs;
        bail!(
            "Insufficient funds: inputs provide {total_inputs} lovelace but \
             outputs + fee require {total_spent} lovelace (shortfall: {shortfall} lovelace)"
        );
    }

    let lovelace_change = total_inputs - total_spent;

    // ── Build the signed token balance map ─────────────────────────────────
    //
    // For every (policy, asset) that appears in inputs OR in the mint field,
    // compute:
    //   balance[asset] = input_qty + mint_qty (signed)
    //
    // `mint_qty` is i64 — positive for new tokens minted, negative for burns.
    // We use i128 internally to avoid overflow when combining u64 + i64.
    let mut balance: HashMap<(Vec<u8>, Vec<u8>), i128> = HashMap::new();

    // Start with input quantities (all non-negative).
    for ((policy, asset), &qty) in input_tokens {
        *balance.entry((policy.clone(), asset.clone())).or_insert(0) += qty as i128;
    }

    // Apply mint/burn quantities from the transaction mint field.
    for (policy_bytes, assets) in mint {
        for (asset_bytes, mint_qty) in assets {
            *balance
                .entry((policy_bytes.clone(), asset_bytes.clone()))
                .or_insert(0) += *mint_qty as i128;
        }
    }

    // ── Compute token change ────────────────────────────────────────────────
    //
    // For every (policy, asset) in either the balance map or the explicit
    // outputs, compute:
    //   change[asset] = balance[asset] - explicit_output_qty[asset]
    //
    // Positive remainder → include in change output.
    // Negative → overdraft error (user sends more than available).
    // Zero    → nothing needed.
    //
    // Collect the full key universe from both maps.
    let all_keys: std::collections::HashSet<(Vec<u8>, Vec<u8>)> = balance
        .keys()
        .cloned()
        .chain(output_tokens.keys().cloned())
        .collect();

    let mut token_change: Vec<(String, String, u64)> = Vec::new();
    for key in &all_keys {
        let available: i128 = balance.get(key).copied().unwrap_or(0);
        let out_qty: i128 = output_tokens.get(key).copied().unwrap_or(0) as i128;
        let remainder = available - out_qty;

        if remainder < 0 {
            // More tokens sent to outputs than available from inputs + mints.
            bail!(
                "Token overdraft: policy {} asset {} — outputs consume {} tokens but \
                 inputs + mints only provide {}",
                hex::encode(&key.0),
                hex::encode(&key.1),
                out_qty,
                available,
            );
        } else if remainder > 0 {
            token_change.push((hex::encode(&key.0), hex::encode(&key.1), remainder as u64));
        }
        // remainder == 0: this asset is exactly balanced, nothing to do.
    }
    // Sort for deterministic output ordering (policy asc, then asset name asc).
    token_change.sort();

    // ── Minimum-ADA check ──────────────────────────────────────────────────
    let min_ada = min_ada_for_output(&token_change, coins_per_utxo_byte);

    if lovelace_change < min_ada && !token_change.is_empty() {
        bail!(
            "Insufficient ADA for token-carrying change output: need at least {min_ada} lovelace \
             to cover the token bundle, but only {lovelace_change} lovelace is available as change. \
             Add more ADA to your inputs or reduce the number of native tokens."
        );
    }

    // When there are no tokens, apply the plain 1 ADA dust floor.
    if token_change.is_empty() {
        const MIN_UTXO_LOVELACE: u64 = 1_000_000;
        if lovelace_change < MIN_UTXO_LOVELACE {
            // Change is dust — leave it as an implicit fee contribution.
            return Ok(None);
        }
    }

    // No change at all (zero lovelace, no tokens) — no output needed.
    if lovelace_change == 0 && token_change.is_empty() {
        return Ok(None);
    }

    Ok(Some(ParsedTxOutput {
        address: change_address.to_string(),
        lovelace: lovelace_change,
        tokens: token_change,
    }))
}

/// Decode the raw UTxO MsgResult payload from a `GetUTxOByTxIn` response and
/// return the total lovelace **and** all native tokens contained in the matched
/// outputs.
///
/// The wire format is:
/// ```text
/// [4, [Map<[tx_hash, index], {0: addr, 1: value, ...}>]]
/// ```
/// where `value` is either a plain uint (ADA-only) or `[uint, multiasset_map]`.
///
/// The multi-asset map has the wire encoding:
/// ```text
/// {bstr(policy_id): {bstr(asset_name): uint(quantity)}}
/// ```
///
/// Returns `(total_lovelace, token_map)` where `token_map` is keyed by
/// `(policy_id_bytes, asset_name_bytes)`.
fn sum_utxo_value(raw: &[u8]) -> Result<(u64, MultiAssetMap)> {
    let mut decoder = minicbor::Decoder::new(raw);

    // Outer: MsgResult = [4, result]
    let _ = decoder.array();
    let tag = decoder
        .u32()
        .map_err(|e| anyhow::anyhow!("Expected MsgResult tag: {e}"))?;
    if tag != 4 {
        bail!("Expected MsgResult(4), got {tag}");
    }

    // Strip HFC success wrapper: array(1) around the actual map
    let pos = decoder.position();
    if let Ok(Some(1)) = decoder.array() {
        // HFC wrapper consumed — nothing more to do here
    } else {
        decoder.set_position(pos);
    }

    // UTxO result: Map<[tx_hash, index], TransactionOutput>
    let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
    if map_len == 0 {
        return Ok((0, HashMap::new()));
    }

    let mut total_lovelace: u64 = 0;
    let mut tokens: MultiAssetMap = HashMap::new();

    for _ in 0..map_len {
        // Key: [tx_hash_bytes, output_index]
        let _ = decoder.array();
        let _ = decoder.bytes(); // tx hash
        let _ = decoder.u32(); // output index

        // Value: PostAlonzo TransactionOutput — map {0: addr, 1: value, ...}
        let output_map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
        for _ in 0..output_map_len {
            let key = decoder.u32().unwrap_or(999);
            match key {
                0 => {
                    // address — skip
                    decoder
                        .skip()
                        .map_err(|e| anyhow::anyhow!("Cannot skip address: {e}"))?;
                }
                1 => {
                    // value: uint (ADA-only) or [uint, multiasset_map]
                    let val_pos = decoder.position();
                    if let Ok(coin) = decoder.u64() {
                        // ADA-only value — no native tokens
                        total_lovelace = total_lovelace.checked_add(coin).ok_or_else(|| {
                            anyhow::anyhow!("Lovelace overflow summing UTxO values")
                        })?;
                    } else {
                        // Multi-asset value: [coin, {policy: {asset: qty}}]
                        decoder.set_position(val_pos);
                        let _ = decoder.array();
                        let coin = decoder
                            .u64()
                            .map_err(|e| anyhow::anyhow!("Cannot read multi-asset coin: {e}"))?;
                        total_lovelace = total_lovelace.checked_add(coin).ok_or_else(|| {
                            anyhow::anyhow!("Lovelace overflow summing UTxO values")
                        })?;

                        // Decode multiasset map: {policy_id_bytes: {asset_name_bytes: qty}}
                        let policy_count = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                        for _ in 0..policy_count {
                            let policy_id = decoder
                                .bytes()
                                .map_err(|e| anyhow::anyhow!("Cannot read policy ID bytes: {e}"))?
                                .to_vec();
                            let asset_count = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                            for _ in 0..asset_count {
                                let asset_name = decoder
                                    .bytes()
                                    .map_err(|e| {
                                        anyhow::anyhow!("Cannot read asset name bytes: {e}")
                                    })?
                                    .to_vec();
                                let qty = decoder.u64().map_err(|e| {
                                    anyhow::anyhow!("Cannot read token quantity: {e}")
                                })?;
                                let entry =
                                    tokens.entry((policy_id.clone(), asset_name)).or_insert(0);
                                *entry = entry
                                    .checked_add(qty)
                                    .ok_or_else(|| anyhow::anyhow!("Token quantity overflow"))?;
                            }
                        }
                    }
                }
                _ => {
                    // datum, script_ref, etc. — skip
                    decoder
                        .skip()
                        .map_err(|e| anyhow::anyhow!("Cannot skip output field {key}: {e}"))?;
                }
            }
        }
    }

    Ok((total_lovelace, tokens))
}

/// Extract fee coefficients and the UTxO cost-per-byte from a protocol
/// parameters JSON string returned by `query_protocol_params`.
///
/// Returns `(min_fee_a, min_fee_b, coins_per_utxo_byte)`:
/// - `min_fee_a` — `txFeePerByte` (aka `minFeeA`) in lovelace per byte
/// - `min_fee_b` — `txFeeFixed`   (aka `minFeeB`) in lovelace (constant term)
/// - `coins_per_utxo_byte` — `utxoCostPerByte` (aka `coinsPerUtxoByte`) used
///   for minimum-ADA computation on token-carrying outputs.  Defaults to
///   4_310 lovelace/byte (current mainnet value) when absent.
///
/// Supports both current cardano-cli field names and legacy aliases.
fn extract_fee_params(params_json: &str) -> Result<(u64, u64, u64)> {
    let pp: serde_json::Value = serde_json::from_str(params_json)
        .map_err(|e| anyhow::anyhow!("Cannot parse protocol params JSON: {e}"))?;

    let min_fee_a = pp["txFeePerByte"]
        .as_u64()
        .or_else(|| pp["minFeeA"].as_u64())
        .ok_or_else(|| anyhow::anyhow!("Protocol params missing txFeePerByte / minFeeA"))?;

    let min_fee_b = pp["txFeeFixed"]
        .as_u64()
        .or_else(|| pp["minFeeB"].as_u64())
        .ok_or_else(|| anyhow::anyhow!("Protocol params missing txFeeFixed / minFeeB"))?;

    // utxoCostPerByte is the Babbage/Conway name; coinsPerUtxoByte is a
    // widely-used alias.  Fall back to 4_310 if neither field is present
    // (offline / old-format protocol params).
    let coins_per_utxo_byte = pp["utxoCostPerByte"]
        .as_u64()
        .or_else(|| pp["coinsPerUtxoByte"].as_u64())
        .unwrap_or(4_310);

    Ok((min_fee_a, min_fee_b, coins_per_utxo_byte))
}

// ─── Build command implementation ──────────────────────────────────────────

/// Build a transaction body and write it as a text envelope to `args.out_file`.
///
/// Shared implementation for both `transaction build` and `transaction build-raw`.
///
/// When `args.socket_path` is `Some` and `args.fee` is `None` the function
/// operates in **auto-balance mode**:
///   1. Connects to the node via N2C.
///   2. Queries UTxO values for every `--tx-in` input.
///   3. Queries the current protocol parameters.
///   4. Builds a preliminary tx body (no change output) and estimates the fee.
///   5. Computes a change output from `(total_inputs - total_outputs - fee)`.
///   6. Re-estimates the fee with the change output included and iterates once
///      for stability — one extra iteration is sufficient because the change
///      output size is constant (a plain ADA output).
///   7. Writes the final balanced body.
async fn cmd_build(args: BuildArgs) -> Result<()> {
    let BuildArgs {
        tx_in,
        tx_out,
        change_address,
        fee: explicit_fee,
        ttl,
        certificate_file,
        withdrawal,
        metadata_json_file,
        tx_in_script_file,
        tx_in_datum_file,
        tx_in_redeemer_file,
        tx_in_execution_units,
        tx_in_collateral,
        required_signer_hash,
        mint,
        out_file,
        socket_path,
        mainnet,
        testnet_magic,
    } = args;

    if tx_in.is_empty() {
        bail!("At least one --tx-in is required");
    }
    // In auto-balance mode (no explicit fee, socket-path given) the user may
    // omit --tx-out entirely when all funds are being swept to --change-address.
    // In manual / offline mode at least one output is required.
    let auto_balance_mode = explicit_fee.is_none() && socket_path.is_some();
    if tx_out.is_empty() && !auto_balance_mode {
        bail!("At least one --tx-out is required");
    }

    let inputs: Vec<(Hash32, u32)> = tx_in
        .iter()
        .map(|s| parse_tx_input(s))
        .collect::<Result<_>>()?;
    let mut outputs: Vec<ParsedTxOutput> = tx_out
        .iter()
        .map(|s| parse_tx_output(s))
        .collect::<Result<_>>()?;

    let collateral_inputs: Vec<(Hash32, u32)> = tx_in_collateral
        .iter()
        .map(|s| parse_tx_input(s))
        .collect::<Result<_>>()?;

    let required_signers: Vec<Vec<u8>> = required_signer_hash
        .iter()
        .map(|s| hex::decode(s).map_err(|e| anyhow::anyhow!("Invalid signer hash: {e}")))
        .collect::<Result<_>>()?;

    let parsed_mint = parse_mint_args(&mint)?;

    // ── Parse Plutus script witnesses ───────────────────────────────────────
    //
    // The four `--tx-in-script-file`, `--tx-in-datum-file`,
    // `--tx-in-redeemer-file`, and `--tx-in-execution-units` arguments are
    // matched by position: the i-th occurrence of each corresponds to the i-th
    // script-bearing `--tx-in`.  cardano-cli uses the same positional pairing
    // convention.
    //
    // Validation: if any script file is given, a matching datum, redeemer, and
    // execution units argument must also be present at the same index.
    let script_witness_count = tx_in_script_file.len();
    if !tx_in_datum_file.is_empty() && tx_in_datum_file.len() != script_witness_count {
        bail!(
            "--tx-in-datum-file count ({}) must match --tx-in-script-file count ({})",
            tx_in_datum_file.len(),
            script_witness_count
        );
    }
    if !tx_in_redeemer_file.is_empty() && tx_in_redeemer_file.len() != script_witness_count {
        bail!(
            "--tx-in-redeemer-file count ({}) must match --tx-in-script-file count ({})",
            tx_in_redeemer_file.len(),
            script_witness_count
        );
    }
    if !tx_in_execution_units.is_empty() && tx_in_execution_units.len() != script_witness_count {
        bail!(
            "--tx-in-execution-units count ({}) must match --tx-in-script-file count ({})",
            tx_in_execution_units.len(),
            script_witness_count
        );
    }

    let mut script_witnesses: Vec<ScriptWitness> = Vec::with_capacity(script_witness_count);
    for i in 0..script_witness_count {
        let (version, script_bytes) = load_plutus_script(&tx_in_script_file[i])?;

        let datum_data = if i < tx_in_datum_file.len() {
            load_plutus_data_file(&tx_in_datum_file[i])?
        } else {
            bail!(
                "--tx-in-datum-file not provided for script witness at position {i}. \
                 Each --tx-in-script-file requires a matching --tx-in-datum-file."
            );
        };

        let redeemer_data = if i < tx_in_redeemer_file.len() {
            load_plutus_data_file(&tx_in_redeemer_file[i])?
        } else {
            bail!(
                "--tx-in-redeemer-file not provided for script witness at position {i}. \
                 Each --tx-in-script-file requires a matching --tx-in-redeemer-file."
            );
        };

        let ex_units = if i < tx_in_execution_units.len() {
            parse_execution_units(&tx_in_execution_units[i])?
        } else {
            bail!(
                "--tx-in-execution-units not provided for script witness at position {i}. \
                 Each --tx-in-script-file requires a matching --tx-in-execution-units."
            );
        };

        // Pre-encode datum and redeemer to CBOR — this avoids round-trip
        // encoding loss when injecting into the witness set and computing the
        // script_data_hash.
        let datum_cbor = encode_plutus_data_to_cbor(&datum_data);
        let redeemer_data_cbor = encode_plutus_data_to_cbor(&redeemer_data);

        script_witnesses.push(ScriptWitness {
            version,
            script_bytes,
            datum_cbor,
            redeemer_data_cbor,
            ex_units,
        });
    }

    // Compute the script_data_hash (tx body field 11) for all Plutus witnesses.
    // This is None when no script witnesses are present.
    let script_data_hash = compute_script_data_hash_offline(&script_witnesses);

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

    // ── Determine the effective fee ─────────────────────────────────────────

    let fee = if let Some(f) = explicit_fee {
        // Manual mode: use the caller-supplied fee verbatim.
        f
    } else if let Some(ref sock) = socket_path {
        // Auto-balance mode: connect to the node and compute everything.

        let change_addr = change_address.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "--change-address is required for auto-balance mode (when --fee is not set)"
            )
        })?;

        let magic = if mainnet {
            764824073u64
        } else {
            testnet_magic.unwrap_or(764824073)
        };

        // Connect to the node.
        let mut client = torsten_network::N2CClient::connect(sock)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Cannot connect to node socket '{}': {e}", sock.display())
            })?;
        client
            .handshake(magic)
            .await
            .map_err(|e| anyhow::anyhow!("Handshake failed: {e}"))?;
        client
            .acquire()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire ledger state: {e}"))?;

        // Query UTxO values for all inputs.
        let utxo_inputs: Vec<(Vec<u8>, u32)> = inputs
            .iter()
            .map(|(h, idx)| (h.as_bytes().to_vec(), *idx))
            .collect();
        let utxo_raw = client
            .query_utxo_by_txin(&utxo_inputs)
            .await
            .map_err(|e| anyhow::anyhow!("UTxO query failed: {e}"))?;
        // Decode both the total lovelace and the full native-token bundle from
        // the UTxO query response.  Tokens present in the inputs but absent
        // from the explicit outputs must appear in the change output.
        let (total_inputs, input_tokens) = sum_utxo_value(&utxo_raw)
            .map_err(|e| anyhow::anyhow!("Failed to decode UTxO response: {e}"))?;

        if total_inputs == 0 {
            bail!(
                "No UTxOs found for the specified inputs. \
                 Ensure the inputs exist on-chain and the node is fully synced."
            );
        }

        // Query protocol parameters.
        let params_json = client
            .query_protocol_params()
            .await
            .map_err(|e| anyhow::anyhow!("Protocol params query failed: {e}"))?;

        client.release().await.ok();
        client.done().await.ok();

        let (min_fee_a, min_fee_b, coins_per_utxo_byte) = extract_fee_params(&params_json)?;

        // Sum the explicit outputs (lovelace only — for fee arithmetic).
        let total_explicit_outputs: u64 = outputs.iter().map(|o| o.lovelace).sum();

        // Build the output token map: aggregate tokens across all explicit
        // outputs so we can compute token change = input_tokens - output_tokens.
        let mut output_tokens: MultiAssetMap = HashMap::new();
        for output in &outputs {
            for (policy_hex, asset_name_hex, qty) in &output.tokens {
                let policy_bytes = hex::decode(policy_hex)?;
                let asset_bytes = hex::decode(asset_name_hex).unwrap_or_default();
                let entry = output_tokens
                    .entry((policy_bytes, asset_bytes))
                    .or_insert(0);
                *entry = entry
                    .checked_add(*qty)
                    .ok_or_else(|| anyhow::anyhow!("Token quantity overflow in outputs"))?;
            }
        }

        // ── Iteration 1: estimate fee without change output ─────────────────
        let body_no_change = build_tx_body_cbor(
            &inputs,
            &outputs,
            0, // placeholder fee — size only matters, not the value
            ttl,
            &certificates,
            &withdrawals,
            auxiliary_data.as_deref(),
            &collateral_inputs,
            &required_signers,
            &parsed_mint,
            script_data_hash.as_ref(),
        )?;
        // Assume 1 witness (the payment key). SPO tools can override with --fee.
        let fee_estimate_1 = estimate_fee(&body_no_change, 1, min_fee_a, min_fee_b);

        // Compute tentative change — including any native tokens and minted/burned tokens.
        let change_output_1 = calculate_change(
            total_inputs,
            total_explicit_outputs,
            fee_estimate_1,
            change_addr,
            &input_tokens,
            &output_tokens,
            &parsed_mint,
            coins_per_utxo_byte,
        )?;

        // ── Iteration 2: re-estimate with the change output included ────────
        //
        // Adding the change output changes the tx body size, which changes
        // the fee, which changes the change amount. One more pass is
        // sufficient for convergence.  Token-carrying change outputs are
        // larger than plain ADA outputs, so including the tokens in this
        // pass gives an accurate size estimate.
        let mut outputs_with_change = outputs.clone();
        if let Some(ref co) = change_output_1 {
            outputs_with_change.push(co.clone());
        }

        let body_with_change = build_tx_body_cbor(
            &inputs,
            &outputs_with_change,
            0,
            ttl,
            &certificates,
            &withdrawals,
            auxiliary_data.as_deref(),
            &collateral_inputs,
            &required_signers,
            &parsed_mint,
            script_data_hash.as_ref(),
        )?;
        let fee_final = estimate_fee(&body_with_change, 1, min_fee_a, min_fee_b);

        // Recompute change with the final fee.
        let change_output_final = calculate_change(
            total_inputs,
            total_explicit_outputs,
            fee_final,
            change_addr,
            &input_tokens,
            &output_tokens,
            &parsed_mint,
            coins_per_utxo_byte,
        )?;

        // Commit: replace `outputs` with the final set (explicit + change).
        if let Some(co) = change_output_final {
            outputs.push(co);
        }

        eprintln!(
            "Auto-balance: total inputs = {total_inputs} lovelace, \
             fee = {fee_final} lovelace, \
             change = {} lovelace",
            total_inputs
                .saturating_sub(total_explicit_outputs)
                .saturating_sub(fee_final)
        );

        fee_final
    } else {
        // Offline / build-raw fallback: use the cardano-cli default of 200 000.
        200_000u64
    };

    // ── Build the final tx body ─────────────────────────────────────────────

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
        script_data_hash.as_ref(),
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

    // Include Plutus witness set CBOR when script witnesses are present.
    // This non-standard field lets `transaction sign` and `transaction assemble`
    // embed the Plutus witnesses alongside the vkey witnesses in the signed tx,
    // without requiring the user to re-supply the script files at sign time.
    if !script_witnesses.is_empty() {
        let plutus_ws_cbor = build_plutus_witness_set_cbor(&script_witnesses);
        envelope["plutusWitnessesCborHex"] =
            serde_json::Value::String(hex::encode(&plutus_ws_cbor));
    }

    std::fs::write(&out_file, serde_json::to_string_pretty(&envelope)?)?;
    println!("Transaction body written to: {}", out_file.display());
    Ok(())
}

impl TransactionCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            // `build` and `build-raw` are identical — both delegate to cmd_build().
            // cmd_build is async (it may connect to the node for auto-balance),
            // so we drive it with a single-threaded tokio runtime here.
            TxSubcommand::Build(args) | TxSubcommand::BuildRaw(args) => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(cmd_build(args))
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

                // Build the witness set map.
                //
                // The witness set is a CBOR map with these optional keys:
                //   0 → vkey witnesses    (always present after signing)
                //   3 → PlutusV1 scripts  \
                //   4 → datums             > from plutusWitnessesCborHex in the
                //   5 → redeemers          > tx body envelope (if any)
                //   6 → PlutusV2 scripts  /
                //   7 → PlutusV3 scripts  /
                //
                // When the tx body envelope contains a `plutusWitnessesCborHex`
                // field we decode the embedded map and merge its keys into the
                // witness set produced here.
                let plutus_ws_hex = envelope
                    .get("plutusWitnessesCborHex")
                    .and_then(|v| v.as_str());

                // Collect Plutus witness-set keys from the pre-built map (if any)
                // so we can count how many witness-set keys we need in total.
                let plutus_entries = collect_plutus_witness_entries(plutus_ws_hex)?;

                // Witness set map: key 0 = vkey witnesses + any Plutus keys
                let ws_key_count = 1 + plutus_entries.len();
                let mut witness_buf = Vec::new();
                {
                    let mut wenc = minicbor::Encoder::new(&mut witness_buf);
                    wenc.map(ws_key_count as u64)?;
                    // Key 0: vkey witnesses
                    wenc.u32(0)?;
                    wenc.array(witnesses.len() as u64)?;
                    for (vkey, sig) in &witnesses {
                        wenc.array(2)?;
                        wenc.bytes(vkey)?;
                        wenc.bytes(sig)?;
                    }
                }
                // Append raw Plutus witness entries (each is pre-encoded key+value CBOR)
                for (_, entry_cbor) in &plutus_entries {
                    witness_buf.extend_from_slice(entry_cbor);
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
                // --tx-in-count and --tx-out-count are accepted for cardano-cli
                // compatibility but are not used: we measure the actual serialised
                // tx body size rather than estimating it from counts.
                tx_in_count: _,
                tx_out_count: _,
            } => {
                // Read protocol parameters from the JSON file produced by
                // `query protocol-parameters`.  We accept both the cardano-cli
                // 10.x names (txFeePerByte / txFeeFixed) and the older names
                // (minFeeA / minFeeB) so that params files from either version
                // of the tool work without modification.
                let pp_content = std::fs::read_to_string(&protocol_params_file)?;
                let pp: serde_json::Value = serde_json::from_str(&pp_content)?;

                let min_fee_a = pp["txFeePerByte"]
                    .as_u64()
                    .or_else(|| pp["minFeeA"].as_u64())
                    .unwrap_or(44);
                let min_fee_b = pp["txFeeFixed"]
                    .as_u64()
                    .or_else(|| pp["minFeeB"].as_u64())
                    .unwrap_or(155_381);

                // Read the tx body text envelope and decode the CBOR payload.
                let content = std::fs::read_to_string(&tx_body_file)?;
                let envelope: serde_json::Value = serde_json::from_str(&content)?;
                let cbor_hex = envelope["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in tx body file"))?;
                let tx_body_cbor = hex::decode(cbor_hex)?;

                // Delegate to the shared fee estimator so the formula is
                // consistent with auto-balance mode.
                let fee = estimate_fee(&tx_body_cbor, witness_count, min_fee_a, min_fee_b);
                // cardano-cli output format: "<fee> Lovelace"
                println!("{fee} Lovelace");
                Ok(())
            }
            TxSubcommand::CalculateMinRequiredUtxo {
                protocol_params_file,
                tx_out,
            } => {
                // Parse the protocol params JSON to extract coinsPerUTxOByte.
                // cardano-cli 10.x uses "coinsPerUTxOByte"; older versions used
                // "utxoCostPerByte" or "minUTxOValue".
                let pp_content = std::fs::read_to_string(&protocol_params_file)?;
                let pp: serde_json::Value = serde_json::from_str(&pp_content)?;

                let coins_per_utxo_byte = pp["coinsPerUTxOByte"]
                    .as_u64()
                    .or_else(|| pp["utxoCostPerByte"].as_u64())
                    .or_else(|| pp["minUTxOValue"].as_u64())
                    .unwrap_or(4_310); // current mainnet/preview default

                // Parse the --tx-out value spec to extract any native tokens.
                // We reuse parse_tx_output() which handles both ADA-only and
                // multi-asset formats.
                let parsed = parse_tx_output(&tx_out)?;

                // Compute minimum ADA using the Babbage/Conway formula via the
                // shared helper function.
                let min_ada = min_ada_for_output(&parsed.tokens, coins_per_utxo_byte);

                // cardano-cli output format: a JSON object with a "lovelace" key
                // matching `cardano-cli transaction calculate-min-required-utxo`.
                let out = serde_json::json!({ "lovelace": min_ada });
                println!("{}", serde_json::to_string_pretty(&out)?);
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

                // Collect Plutus witness entries stored in the body envelope
                let plutus_ws_hex = body_env
                    .get("plutusWitnessesCborHex")
                    .and_then(|v| v.as_str());
                let plutus_entries = collect_plutus_witness_entries(plutus_ws_hex)?;

                // Build signed tx: [body, witness_set, true, null]
                let mut tx_cbor = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut tx_cbor);
                enc.array(4)?;
                // Write body as raw CBOR
                tx_cbor.extend_from_slice(&body_cbor);
                // Witness set: {0: [[vkey, sig], ...], <plutus keys...>}
                let ws_key_count = 1 + plutus_entries.len();
                let mut ws_buf = Vec::new();
                {
                    let mut ws_enc = minicbor::Encoder::new(&mut ws_buf);
                    ws_enc.map(ws_key_count as u64)?;
                    ws_enc.u32(0)?; // vkey witnesses
                    ws_enc.array(vkey_witnesses.len() as u64)?;
                    for (vkey, sig) in &vkey_witnesses {
                        ws_enc.array(2)?;
                        ws_enc.bytes(vkey)?;
                        ws_enc.bytes(sig)?;
                    }
                }
                // Append raw Plutus witness entries
                for (_, entry_cbor) in &plutus_entries {
                    ws_buf.extend_from_slice(entry_cbor);
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

/// Decode a Plutus witness-set map from a hex-encoded CBOR string and return
/// the individual map entries as `(key, raw_cbor_key_and_value)` pairs.
///
/// This is used by `transaction sign` and `transaction assemble` to merge
/// Plutus witness-set keys (3–7) from the `plutusWitnessesCborHex` field in
/// the tx body envelope into the final signed transaction's witness set.
///
/// Returning raw CBOR bytes for each entry (key + value as a contiguous slice)
/// lets us inject them verbatim into the new map without re-encoding, which
/// preserves the byte-exact encoding needed for the script data hash to match.
fn collect_plutus_witness_entries(plutus_ws_hex: Option<&str>) -> Result<Vec<(u32, Vec<u8>)>> {
    let hex = match plutus_ws_hex {
        None => return Ok(Vec::new()),
        Some("") => return Ok(Vec::new()),
        Some(h) => h,
    };

    let cbor = hex::decode(hex)
        .map_err(|e| anyhow::anyhow!("Invalid hex in plutusWitnessesCborHex: {e}"))?;

    let mut dec = minicbor::Decoder::new(&cbor);
    let map_len = dec
        .map()
        .map_err(|e| anyhow::anyhow!("Plutus witness set is not a CBOR map: {e}"))?
        .unwrap_or(0) as usize;

    let mut entries = Vec::with_capacity(map_len);
    for _ in 0..map_len {
        // Record the byte range that covers the key+value pair so we can
        // inject both as a single raw slice into the merged witness set.
        let entry_start = dec.position();
        let key = dec
            .u32()
            .map_err(|e| anyhow::anyhow!("Plutus witness map key is not a uint: {e}"))?;
        dec.skip()
            .map_err(|e| anyhow::anyhow!("Cannot skip Plutus witness map value: {e}"))?;
        let entry_end = dec.position();
        entries.push((key, cbor[entry_start..entry_end].to_vec()));
    }

    Ok(entries)
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
            None,
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
            None,
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
            None,
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

    // ── Auto-balance: estimate_fee tests ─────────────────────────────────────

    #[test]
    fn test_estimate_fee_basic() {
        // A realistic Conway-era mainnet parameter set (preview/mainnet 2024):
        //   minFeeA (txFeePerByte) = 44
        //   minFeeB (txFeeFixed)   = 155381
        let min_fee_a = 44u64;
        let min_fee_b = 155_381u64;

        // Build a minimal tx body: just inputs+outputs+fee fields so we get
        // a deterministic size to reason about.
        let body = build_tx_body_cbor(
            &[(Hash32::from_bytes([0xab; 32]), 0)],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();

        let fee = estimate_fee(&body, 1, min_fee_a, min_fee_b);

        // fee = 44 * (body_len + 106 + 11) + 155381
        let expected_size = body.len() as u64 + 106 + 11;
        let expected_fee = 44 * expected_size + 155_381;
        assert_eq!(fee, expected_fee);
    }

    #[test]
    fn test_estimate_fee_multiple_witnesses() {
        // With 2 witnesses (e.g. payment + stake key), the fee increases by
        // exactly one witness worth of overhead.
        let body = build_tx_body_cbor(
            &[(Hash32::from_bytes([0xcd; 32]), 1)],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();

        let fee_1 = estimate_fee(&body, 1, 44, 155_381);
        let fee_2 = estimate_fee(&body, 2, 44, 155_381);
        // Each extra witness adds 106 bytes → 44 * 106 = 4664 lovelace
        assert_eq!(fee_2 - fee_1, 44 * 106);
    }

    #[test]
    fn test_estimate_fee_grows_with_body_size() {
        // A larger tx body (e.g. two inputs) should yield a higher fee.
        let small_body = build_tx_body_cbor(
            &[(Hash32::from_bytes([0x01; 32]), 0)],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();

        let large_body = build_tx_body_cbor(
            &[
                (Hash32::from_bytes([0x01; 32]), 0),
                (Hash32::from_bytes([0x02; 32]), 1),
            ],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();

        let fee_small = estimate_fee(&small_body, 1, 44, 155_381);
        let fee_large = estimate_fee(&large_body, 1, 44, 155_381);
        assert!(
            fee_large > fee_small,
            "larger tx body must produce higher fee"
        );
    }

    // ── Auto-balance: calculate_change tests ─────────────────────────────────

    /// Convenience helper: empty token maps and mainnet coins_per_utxo_byte.
    fn no_tokens() -> (MultiAssetMap, MultiAssetMap) {
        (HashMap::new(), HashMap::new())
    }

    #[test]
    fn test_calculate_change_exact_fit() {
        // When inputs exactly cover outputs + fee, change is zero → None.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            10_000_000, // total inputs
            9_800_000,  // total outputs
            200_000,    // fee
            "addr_test1abc",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(
            result.is_none(),
            "zero change should produce None (not an empty output)"
        );
    }

    #[test]
    fn test_calculate_change_dust() {
        // Change below 1 ADA (min-UTxO) is considered dust → None.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            10_000_000, // total inputs
            9_700_000,  // total outputs
            299_999,    // fee  → change = 1 lovelace (dust)
            "addr_test1abc",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(result.is_none(), "dust change must be dropped");
    }

    #[test]
    fn test_calculate_change_spendable() {
        // Change of exactly 1 ADA should produce a change output.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            11_000_000, // total inputs
            9_800_000,  // total outputs
            200_000,    // fee  → change = 1_000_000 = 1 ADA
            "addr_test1abc",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(result.is_some());
        let output = result.unwrap();
        assert_eq!(output.lovelace, 1_000_000);
        assert_eq!(output.address, "addr_test1abc");
        assert!(output.tokens.is_empty());
    }

    #[test]
    fn test_calculate_change_large_change() {
        // SPO scenario: large wallet, small payment, majority goes to change.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            100_000_000_000, // 100k ADA input
            1_000_000,       // 1 ADA payment
            170_000,         // fee
            "addr_test1change",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(result.is_some());
        let output = result.unwrap();
        assert_eq!(output.lovelace, 100_000_000_000 - 1_000_000 - 170_000);
    }

    #[test]
    fn test_calculate_change_insufficient_funds() {
        // Inputs do not cover outputs + fee → error with shortfall description.
        let (inp, out) = no_tokens();
        let err = calculate_change(
            5_000_000, // 5 ADA input
            5_000_000, // 5 ADA output (no room for fee)
            200_000,   // fee
            "addr_test1abc",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Insufficient funds"),
            "error message must mention 'Insufficient funds', got: {msg}"
        );
        assert!(
            msg.contains("200000"),
            "error message must include the shortfall amount"
        );
    }

    #[test]
    fn test_calculate_change_exactly_at_minimum() {
        // Change of exactly MIN_UTXO_LOVELACE (1 ADA) should be included.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            11_200_000, // 11.2 ADA
            10_000_000, // 10 ADA output
            200_000,    // fee
            "addr_test1x",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().lovelace, 1_000_000);
    }

    #[test]
    fn test_calculate_change_just_below_minimum() {
        // Change of MIN_UTXO_LOVELACE - 1 (999_999 lovelace) is dust → None.
        let (inp, out) = no_tokens();
        let result = calculate_change(
            11_199_999, // 11.199999 ADA
            10_000_000, // 10 ADA output
            200_000,    // fee → change = 999_999
            "addr_test1x",
            &inp,
            &out,
            &[], // no mint
            4_310,
        )
        .unwrap();
        assert!(result.is_none());
    }

    // ── Native-token change tests ─────────────────────────────────────────────

    /// Build a policy-keyed token entry for `MultiAssetMap`.
    ///
    /// `policy_hex` must be 56 hex chars (28 bytes).
    fn token_entry(policy_hex: &str, asset_name_hex: &str, qty: u64) -> ((Vec<u8>, Vec<u8>), u64) {
        (
            (
                hex::decode(policy_hex).unwrap(),
                hex::decode(asset_name_hex).unwrap(),
            ),
            qty,
        )
    }

    #[test]
    fn test_change_with_native_tokens() {
        // Input UTxO: 10 ADA + 500 HOSKY tokens.
        // Explicit output: 5 ADA (ADA-only, no tokens).
        // Expected change: 5 ADA - fee, plus all 500 HOSKY tokens.

        // 28-byte policy ID (56 hex chars), 5-byte asset name ("HOSKY")
        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59"; // "HOSKY" in hex

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 500);
        input_tokens.insert((p, a), qty);

        let output_tokens: MultiAssetMap = HashMap::new(); // no tokens in explicit outputs

        // fee = 170_000 → change lovelace = 10_000_000 - 5_000_000 - 170_000 = 4_830_000
        let result = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &[], // no mint
            4_310,
        )
        .unwrap();

        let change = result.expect("should produce a change output with tokens");
        assert_eq!(change.lovelace, 4_830_000, "lovelace change is wrong");
        assert_eq!(
            change.tokens.len(),
            1,
            "should carry exactly one token type"
        );
        let (cpol, casset, cqty) = &change.tokens[0];
        assert_eq!(cpol, policy_hex, "policy ID must round-trip correctly");
        assert_eq!(casset, asset_hex, "asset name must round-trip correctly");
        assert_eq!(*cqty, 500, "token quantity must be preserved in full");
    }

    #[test]
    fn test_change_min_ada_with_tokens() {
        // Verify min_ada_for_output returns at least 1 ADA for token bundles
        // and scales with the number of tokens.

        // ADA-only output → always 1 ADA floor.
        assert_eq!(min_ada_for_output(&[], 4_310), 1_000_000);

        // Single-token output: formula should exceed 1 ADA.
        let tokens_one: Vec<(String, String, u64)> = vec![(
            "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef7".to_string(),
            "484f534b59".to_string(),
            100,
        )];
        let min_one = min_ada_for_output(&tokens_one, 4_310);
        // size = 10 (base) + 29 (policy) + (5/2=2 + 10) = 51 bytes → formula = 4310*(51+160) = 909_410 → below 1M floor → 1_000_000
        assert!(
            min_one >= 1_000_000,
            "min ADA for token output must be at least 1 ADA, got {min_one}"
        );

        // Many tokens: the formula result should grow and eventually exceed the 1 ADA floor.
        let mut many_tokens: Vec<(String, String, u64)> = Vec::new();
        for i in 0..10u8 {
            many_tokens.push((
                format!("{:056x}", i), // 28-byte policy
                format!("{:016x}", i), // 8-byte asset name
                u64::from(i) + 1,
            ));
        }
        let min_many = min_ada_for_output(&many_tokens, 4_310);
        assert!(
            min_many >= 1_000_000,
            "multi-token min ADA must be at least 1 ADA, got {min_many}"
        );
        // With 10 tokens across 10 policies the formula should clearly beat 1 ADA.
        // 10 policies × (29 + 8/2 + 10) bytes each ≈ 430 bytes → 4310*(430+160) ≈ 2.5M
        assert!(
            min_many > 1_000_000,
            "with many tokens min ADA should exceed the flat 1 ADA floor, got {min_many}"
        );
    }

    #[test]
    fn test_insufficient_ada_for_token_change() {
        // Input UTxO: 2 ADA + tokens.
        // Explicit output consumes 1.8 ADA → only 0.2 ADA available as change.
        // But the token change output requires at least 1 ADA → must error.

        // 28-byte policy ID (56 hex chars), 5-byte asset name ("HOSKY")
        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 1_000);
        input_tokens.insert((p, a), qty);

        let output_tokens: MultiAssetMap = HashMap::new();

        // 2_000_000 - 1_800_000 - 100_000 = 100_000 lovelace change
        // min_ada for a token output is at least 1_000_000 → error
        let err = calculate_change(
            2_000_000,
            1_800_000,
            100_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &[], // no mint
            4_310,
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("Insufficient ADA for token-carrying change output"),
            "expected token-change error, got: {msg}"
        );
        assert!(
            msg.contains("100000"),
            "error should state available lovelace (100000), got: {msg}"
        );
    }

    // ── Minting / burning: calculate_change with mint field ──────────────────

    /// Build a `MintEntry` from hex-encoded policy + asset name + signed qty.
    fn mint_entry(policy_hex: &str, asset_name_hex: &str, qty: i64) -> MintEntry {
        (
            hex::decode(policy_hex).unwrap(),
            vec![(hex::decode(asset_name_hex).unwrap(), qty)],
        )
    }

    #[test]
    fn test_change_minted_tokens_all_to_change() {
        // Transaction mints 1000 HOSKY tokens and sends no tokens to explicit
        // outputs.  All minted tokens should appear in the change output.
        //
        // Input UTxO: 10 ADA (no tokens).
        // Explicit output: 5 ADA.
        // Mint: +1000 HOSKY.
        // Expected change: ~4.83 ADA + 1000 HOSKY.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59"; // "HOSKY"

        let input_tokens: MultiAssetMap = HashMap::new(); // no tokens in inputs
        let output_tokens: MultiAssetMap = HashMap::new(); // no tokens in explicit outputs
        let mint = vec![mint_entry(policy_hex, asset_hex, 1_000)];

        let result = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("minted tokens with no output must produce a change output");
        assert_eq!(
            change.lovelace, 4_830_000,
            "lovelace change must be correct"
        );
        assert_eq!(
            change.tokens.len(),
            1,
            "minted tokens must appear in change"
        );
        let (cpol, casset, cqty) = &change.tokens[0];
        assert_eq!(cpol, policy_hex);
        assert_eq!(casset, asset_hex);
        assert_eq!(*cqty, 1_000, "all minted tokens must go to change");
    }

    #[test]
    fn test_change_minted_tokens_partial_to_explicit_output() {
        // Transaction mints 1000 HOSKY tokens; 400 are sent to an explicit
        // output; the remaining 600 must appear in the change output.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let input_tokens: MultiAssetMap = HashMap::new();
        let mut output_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), _) = token_entry(policy_hex, asset_hex, 400);
        output_tokens.insert((p, a), 400);
        let mint = vec![mint_entry(policy_hex, asset_hex, 1_000)];

        let result = calculate_change(
            10_000_000,
            7_000_000, // explicit outputs include 2 ADA for token UTxO + 5 ADA payment
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("partial mint to explicit output must yield token change");
        assert_eq!(change.tokens.len(), 1, "one token type in change");
        let (_, _, cqty) = &change.tokens[0];
        assert_eq!(*cqty, 600, "remaining 600 minted tokens must go to change");
    }

    #[test]
    fn test_change_multiple_policies_minted() {
        // Mint two different policies; neither sent to explicit outputs.
        // Both should appear in the consolidated change output.

        let policy_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let policy_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let asset_hex = "4100"; // "A"

        let input_tokens: MultiAssetMap = HashMap::new();
        let output_tokens: MultiAssetMap = HashMap::new();
        let mint = vec![
            mint_entry(policy_a, asset_hex, 500),
            mint_entry(policy_b, asset_hex, 300),
        ];

        let result = calculate_change(
            10_000_000,
            2_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("minted tokens from multiple policies must produce change");
        assert_eq!(
            change.tokens.len(),
            2,
            "both minted token types must appear in change"
        );
        // Tokens are sorted: policy_a < policy_b lexicographically.
        let qtys: Vec<u64> = change.tokens.iter().map(|(_, _, q)| *q).collect();
        assert!(qtys.contains(&500), "500 of policy_a must be in change");
        assert!(qtys.contains(&300), "300 of policy_b must be in change");
    }

    #[test]
    fn test_change_burn_reduces_input_tokens() {
        // Input UTxO: 10 ADA + 1000 HOSKY.
        // Burn 400 HOSKY.
        // No explicit token outputs.
        // Change should contain 1000 - 400 = 600 HOSKY.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 1_000);
        input_tokens.insert((p, a), qty);

        let output_tokens: MultiAssetMap = HashMap::new();
        // Burn 400 (negative mint quantity).
        let mint = vec![mint_entry(policy_hex, asset_hex, -400)];

        let result = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("burn leaves remaining tokens in change");
        assert_eq!(change.tokens.len(), 1);
        let (_, _, cqty) = &change.tokens[0];
        assert_eq!(*cqty, 600, "1000 input - 400 burned = 600 in change");
    }

    #[test]
    fn test_change_full_burn_no_token_change() {
        // Input UTxO: 10 ADA + 500 HOSKY.
        // Burn all 500 HOSKY.
        // Change should be ADA-only with no tokens.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 500);
        input_tokens.insert((p, a), qty);

        let output_tokens: MultiAssetMap = HashMap::new();
        // Burn all 500 tokens.
        let mint = vec![mint_entry(policy_hex, asset_hex, -500)];

        let result = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("ADA change still expected after full burn");
        assert!(
            change.tokens.is_empty(),
            "after burning all tokens, change must be ADA-only"
        );
        assert_eq!(change.lovelace, 4_830_000);
    }

    #[test]
    fn test_change_token_overdraft_error() {
        // Explicit output tries to send 1200 HOSKY but inputs only have 1000
        // and no mint is present → overdraft error.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 1_000);
        input_tokens.insert((p.clone(), a.clone()), qty);

        let mut output_tokens: MultiAssetMap = HashMap::new();
        output_tokens.insert((p, a), 1_200); // 200 more than available

        let err = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &[], // no mint
            4_310,
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("overdraft") || msg.contains("Token overdraft"),
            "expected overdraft error, got: {msg}"
        );
    }

    #[test]
    fn test_change_input_and_minted_tokens_combined() {
        // Input UTxO: 10 ADA + 300 HOSKY.
        // Mint: +200 HOSKY.
        // Explicit output: 100 HOSKY.
        // Expected change: 300 + 200 - 100 = 400 HOSKY.

        let policy_hex = "a0028f350aaabe0545fdcb56b039bfb08e4bb4d8c4d7c3c7d481ef70";
        let asset_hex = "484f534b59";

        let mut input_tokens: MultiAssetMap = HashMap::new();
        let ((p, a), qty) = token_entry(policy_hex, asset_hex, 300);
        input_tokens.insert((p.clone(), a.clone()), qty);

        let mut output_tokens: MultiAssetMap = HashMap::new();
        output_tokens.insert((p, a), 100);

        let mint = vec![mint_entry(policy_hex, asset_hex, 200)];

        let result = calculate_change(
            10_000_000,
            5_000_000,
            170_000,
            "addr_test1change",
            &input_tokens,
            &output_tokens,
            &mint,
            4_310,
        )
        .unwrap();

        let change = result.expect("combined input+mint should produce token change");
        assert_eq!(change.tokens.len(), 1);
        let (_, _, cqty) = &change.tokens[0];
        assert_eq!(
            *cqty, 400,
            "300 input + 200 minted - 100 output = 400 change"
        );
    }

    // ── Auto-balance: extract_fee_params tests ───────────────────────────────

    #[test]
    fn test_extract_fee_params_current_names() {
        // Current cardano-cli JSON field names (post-Alonzo).
        let json = r#"{"txFeePerByte": 44, "txFeeFixed": 155381, "utxoCostPerByte": 4310}"#;
        let (a, b, c) = extract_fee_params(json).unwrap();
        assert_eq!(a, 44);
        assert_eq!(b, 155_381);
        assert_eq!(c, 4_310);
    }

    #[test]
    fn test_extract_fee_params_legacy_names() {
        // Legacy aliases used by older cardano-cli versions.
        let json = r#"{"minFeeA": 44, "minFeeB": 155381, "coinsPerUtxoByte": 4310}"#;
        let (a, b, c) = extract_fee_params(json).unwrap();
        assert_eq!(a, 44);
        assert_eq!(b, 155_381);
        assert_eq!(c, 4_310);
    }

    #[test]
    fn test_extract_fee_params_default_coins_per_utxo_byte() {
        // When utxoCostPerByte is absent, the default (4_310) is used.
        let json = r#"{"txFeePerByte": 44, "txFeeFixed": 155381}"#;
        let (a, b, c) = extract_fee_params(json).unwrap();
        assert_eq!(a, 44);
        assert_eq!(b, 155_381);
        assert_eq!(c, 4_310, "default coins_per_utxo_byte must be 4310");
    }

    #[test]
    fn test_extract_fee_params_missing_field() {
        let json = r#"{"txFeePerByte": 44}"#;
        assert!(extract_fee_params(json).is_err());
    }

    // ── Auto-balance: sum_utxo_value tests ───────────────────────────────────

    /// Build a synthetic UTxO MsgResult payload for testing (ADA-only outputs).
    ///
    /// Format: [4, [Map<[tx_hash, index], {1: lovelace}>]]
    fn build_utxo_msg_result(entries: &[([u8; 32], u32, u64)]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);

        // [4, [map]]
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag

        // HFC success wrapper: array(1)
        enc.array(1).unwrap();

        // UTxO map
        enc.map(entries.len() as u64).unwrap();
        for (hash, index, lovelace) in entries {
            // Key: [tx_hash, index]
            enc.array(2).unwrap();
            enc.bytes(hash).unwrap();
            enc.u32(*index).unwrap();

            // Value: {1: lovelace}  (simplified output — only the value field)
            enc.map(1).unwrap();
            enc.u32(1).unwrap();
            enc.u64(*lovelace).unwrap();
        }

        buf
    }

    /// Build a synthetic UTxO MsgResult payload for testing (multi-asset outputs).
    ///
    /// Each entry: (tx_hash, output_index, lovelace, tokens)
    /// where tokens is a slice of (policy_bytes, asset_name_bytes, quantity).
    #[allow(clippy::type_complexity)]
    fn build_utxo_msg_result_with_tokens(
        entries: &[(&[u8; 32], u32, u64, &[(&[u8], &[u8], u64)])],
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);

        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag
        enc.array(1).unwrap(); // HFC wrapper

        enc.map(entries.len() as u64).unwrap();
        for (hash, index, lovelace, tokens) in entries {
            // Key
            enc.array(2).unwrap();
            enc.bytes(*hash).unwrap();
            enc.u32(*index).unwrap();

            // Value: {1: [lovelace, {policy: {asset: qty}}]}
            enc.map(1).unwrap();
            enc.u32(1).unwrap();

            if tokens.is_empty() {
                enc.u64(*lovelace).unwrap();
            } else {
                // Group by policy for correct wire encoding
                use std::collections::BTreeMap;
                let mut policy_map: BTreeMap<&[u8], Vec<(&[u8], u64)>> = BTreeMap::new();
                for &(policy, asset, qty) in *tokens {
                    policy_map.entry(policy).or_default().push((asset, qty));
                }

                enc.array(2).unwrap();
                enc.u64(*lovelace).unwrap();
                enc.map(policy_map.len() as u64).unwrap();
                for (policy, assets) in &policy_map {
                    enc.bytes(policy).unwrap();
                    enc.map(assets.len() as u64).unwrap();
                    for (asset_name, qty) in assets {
                        enc.bytes(asset_name).unwrap();
                        enc.u64(*qty).unwrap();
                    }
                }
            }
        }

        buf
    }

    #[test]
    fn test_sum_utxo_value_single_entry() {
        let raw = build_utxo_msg_result(&[([0xab; 32], 0, 5_000_000)]);
        let (total, tokens) = sum_utxo_value(&raw).unwrap();
        assert_eq!(total, 5_000_000);
        assert!(tokens.is_empty(), "ADA-only UTxO should have no tokens");
    }

    #[test]
    fn test_sum_utxo_value_multiple_entries() {
        let raw = build_utxo_msg_result(&[
            ([0x01; 32], 0, 10_000_000),
            ([0x02; 32], 1, 20_000_000),
            ([0x03; 32], 0, 5_000_000),
        ]);
        let (total, tokens) = sum_utxo_value(&raw).unwrap();
        assert_eq!(total, 35_000_000);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_sum_utxo_value_empty_map() {
        let raw = build_utxo_msg_result(&[]);
        let (total, tokens) = sum_utxo_value(&raw).unwrap();
        assert_eq!(total, 0);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_sum_utxo_value_with_native_tokens() {
        // A single UTxO output carrying 5 ADA and 500 HOSKY tokens.
        let policy: [u8; 28] = [0xaa; 28];
        let asset_name: [u8; 5] = [0x48, 0x4f, 0x53, 0x4b, 0x59]; // "HOSKY"

        let raw = build_utxo_msg_result_with_tokens(&[(
            &[0xab; 32],
            0,
            5_000_000,
            &[(&policy, &asset_name, 500)],
        )]);

        let (total, token_map) = sum_utxo_value(&raw).unwrap();
        assert_eq!(total, 5_000_000, "lovelace must be summed correctly");
        assert_eq!(token_map.len(), 1, "should decode exactly one token type");

        let qty = token_map
            .get(&(policy.to_vec(), asset_name.to_vec()))
            .copied()
            .unwrap_or(0);
        assert_eq!(qty, 500, "token quantity must match");
    }

    #[test]
    fn test_sum_utxo_value_tokens_aggregated_across_utxos() {
        // Two UTxOs both carrying the same policy/asset — quantities must be summed.
        let policy: [u8; 28] = [0xbb; 28];
        let asset_name: [u8; 4] = [0x54, 0x45, 0x53, 0x54]; // "TEST"

        let raw = build_utxo_msg_result_with_tokens(&[
            (&[0x01; 32], 0, 3_000_000, &[(&policy, &asset_name, 200)]),
            (&[0x02; 32], 0, 2_000_000, &[(&policy, &asset_name, 300)]),
        ]);

        let (total_lovelace, token_map) = sum_utxo_value(&raw).unwrap();
        assert_eq!(total_lovelace, 5_000_000);
        let qty = token_map
            .get(&(policy.to_vec(), asset_name.to_vec()))
            .copied()
            .unwrap_or(0);
        assert_eq!(qty, 500, "token quantities across UTxOs must be summed");
    }

    // ── Plutus data JSON parsing tests ───────────────────────────────────────

    #[test]
    fn test_parse_plutus_data_int() {
        let json = serde_json::json!({"int": 42});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Integer(42));
    }

    #[test]
    fn test_parse_plutus_data_negative_int() {
        let json = serde_json::json!({"int": -100});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Integer(-100));
    }

    #[test]
    fn test_parse_plutus_data_bytes() {
        let json = serde_json::json!({"bytes": "deadbeef"});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn test_parse_plutus_data_empty_bytes() {
        let json = serde_json::json!({"bytes": ""});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Bytes(vec![]));
    }

    #[test]
    fn test_parse_plutus_data_list() {
        let json = serde_json::json!({"list": [{"int": 1}, {"int": 2}]});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(
            data,
            PlutusData::List(vec![PlutusData::Integer(1), PlutusData::Integer(2)])
        );
    }

    #[test]
    fn test_parse_plutus_data_empty_list() {
        let json = serde_json::json!({"list": []});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::List(vec![]));
    }

    #[test]
    fn test_parse_plutus_data_map() {
        let json = serde_json::json!({
            "map": [
                {"k": {"int": 1}, "v": {"bytes": "aabb"}}
            ]
        });
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(
            data,
            PlutusData::Map(vec![(
                PlutusData::Integer(1),
                PlutusData::Bytes(vec![0xaa, 0xbb])
            )])
        );
    }

    #[test]
    fn test_parse_plutus_data_constructor_small() {
        // Constructor 0 with one field
        let json = serde_json::json!({"constructor": 0, "fields": [{"int": 42}]});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Constr(0, vec![PlutusData::Integer(42)]));
    }

    #[test]
    fn test_parse_plutus_data_constructor_empty_fields() {
        let json = serde_json::json!({"constructor": 1, "fields": []});
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(data, PlutusData::Constr(1, vec![]));
    }

    #[test]
    fn test_parse_plutus_data_nested() {
        // Nested constructor with list inside
        let json = serde_json::json!({
            "constructor": 0,
            "fields": [
                {"list": [{"int": 10}, {"bytes": "ff"}]}
            ]
        });
        let data = parse_plutus_data_json(&json).unwrap();
        assert_eq!(
            data,
            PlutusData::Constr(
                0,
                vec![PlutusData::List(vec![
                    PlutusData::Integer(10),
                    PlutusData::Bytes(vec![0xff])
                ])]
            )
        );
    }

    #[test]
    fn test_parse_plutus_data_invalid_schema() {
        let json = serde_json::json!({"unknown": "value"});
        assert!(parse_plutus_data_json(&json).is_err());
    }

    #[test]
    fn test_parse_plutus_data_invalid_bytes_hex() {
        let json = serde_json::json!({"bytes": "zzzz"});
        assert!(parse_plutus_data_json(&json).is_err());
    }

    // ── Plutus data CBOR encoding tests ─────────────────────────────────────

    #[test]
    fn test_encode_plutus_integer_zero() {
        let cbor = encode_plutus_data_to_cbor(&PlutusData::Integer(0));
        // CBOR uint(0) = 0x00
        assert_eq!(cbor, vec![0x00]);
    }

    #[test]
    fn test_encode_plutus_integer_positive() {
        let cbor = encode_plutus_data_to_cbor(&PlutusData::Integer(42));
        // CBOR uint(42) = 0x18 0x2a
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.u64().unwrap(), 42);
    }

    #[test]
    fn test_encode_plutus_integer_negative() {
        let cbor = encode_plutus_data_to_cbor(&PlutusData::Integer(-1));
        // CBOR negative int -1 = 0x20
        assert_eq!(cbor, vec![0x20]);
    }

    #[test]
    fn test_encode_plutus_bytes() {
        let cbor = encode_plutus_data_to_cbor(&PlutusData::Bytes(vec![0xde, 0xad]));
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.bytes().unwrap(), &[0xde, 0xad]);
    }

    #[test]
    fn test_encode_plutus_list() {
        let cbor = encode_plutus_data_to_cbor(&PlutusData::List(vec![
            PlutusData::Integer(1),
            PlutusData::Integer(2),
        ]));
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.u64().unwrap(), 1);
        assert_eq!(dec.u64().unwrap(), 2);
    }

    #[test]
    fn test_encode_plutus_constr_small() {
        // Constructor 0 → CBOR tag 121
        let cbor =
            encode_plutus_data_to_cbor(&PlutusData::Constr(0, vec![PlutusData::Integer(42)]));
        let mut dec = minicbor::Decoder::new(&cbor);
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 121); // 121 + 0
        assert_eq!(dec.array().unwrap(), Some(1));
        assert_eq!(dec.u64().unwrap(), 42);
    }

    #[test]
    fn test_encode_plutus_constr_large() {
        // Constructor 7 → CBOR tag 1280
        let cbor = encode_plutus_data_to_cbor(&PlutusData::Constr(7, vec![]));
        let mut dec = minicbor::Decoder::new(&cbor);
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 1280); // 1280 + (7-7)
        assert_eq!(dec.array().unwrap(), Some(0));
    }

    // ── Execution unit parsing tests ─────────────────────────────────────────

    #[test]
    fn test_parse_execution_units_valid() {
        let ex = parse_execution_units("1000000,500000000").unwrap();
        assert_eq!(ex.mem, 1_000_000);
        assert_eq!(ex.steps, 500_000_000);
    }

    #[test]
    fn test_parse_execution_units_with_spaces() {
        let ex = parse_execution_units("  200000 , 100000000 ").unwrap();
        assert_eq!(ex.mem, 200_000);
        assert_eq!(ex.steps, 100_000_000);
    }

    #[test]
    fn test_parse_execution_units_missing_comma() {
        assert!(parse_execution_units("1000000").is_err());
    }

    #[test]
    fn test_parse_execution_units_non_numeric() {
        assert!(parse_execution_units("abc,def").is_err());
    }

    // ── CBOR byte unwrapping tests ────────────────────────────────────────────

    #[test]
    fn test_cbor_unwrap_bytes_plain() {
        // Encode bytes(b"hello") and check we get "hello" back
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).bytes(b"hello").unwrap();
        let result = cbor_unwrap_bytes(&buf).unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_cbor_unwrap_bytes_not_bytes() {
        // A CBOR uint is not a byte string
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).u32(42).unwrap();
        // Should fall through to None because it's not bytes
        assert!(cbor_unwrap_bytes(&buf).is_none());
    }

    // ── Script data hash computation tests ──────────────────────────────────

    #[test]
    fn test_script_data_hash_none_for_empty_witnesses() {
        let hash = compute_script_data_hash_offline(&[]);
        assert!(hash.is_none(), "No witnesses → no script data hash");
    }

    #[test]
    fn test_script_data_hash_present_for_witness() {
        // A witness with an integer datum/redeemer must produce a hash
        let datum = PlutusData::Integer(42);
        let redeemer = PlutusData::Integer(0);
        let w = ScriptWitness {
            version: PlutusVersion::V2,
            script_bytes: vec![0x01, 0x02, 0x03],
            datum_cbor: encode_plutus_data_to_cbor(&datum),
            redeemer_data_cbor: encode_plutus_data_to_cbor(&redeemer),
            ex_units: ExUnits {
                mem: 1_000_000,
                steps: 500_000_000,
            },
        };
        let hash = compute_script_data_hash_offline(&[w]);
        assert!(
            hash.is_some(),
            "One witness → script data hash must be Some"
        );
        // The hash must be exactly 32 bytes
        assert_eq!(hash.unwrap().as_bytes().len(), 32);
    }

    #[test]
    fn test_script_data_hash_is_deterministic() {
        let datum = PlutusData::Bytes(vec![0xca, 0xfe]);
        let redeemer = PlutusData::Constr(0, vec![]);
        let w = ScriptWitness {
            version: PlutusVersion::V1,
            script_bytes: vec![0xde, 0xad],
            datum_cbor: encode_plutus_data_to_cbor(&datum),
            redeemer_data_cbor: encode_plutus_data_to_cbor(&redeemer),
            ex_units: ExUnits {
                mem: 100,
                steps: 200,
            },
        };
        let h1 = compute_script_data_hash_offline(std::slice::from_ref(&w));
        let h2 = compute_script_data_hash_offline(std::slice::from_ref(&w));
        assert_eq!(h1, h2, "Hash must be deterministic");
    }

    // ── Plutus witness set CBOR tests ────────────────────────────────────────

    #[test]
    fn test_build_plutus_witness_set_empty() {
        let cbor = build_plutus_witness_set_cbor(&[]);
        // Must be empty CBOR map: 0xa0
        assert_eq!(cbor, vec![0xa0]);
    }

    #[test]
    fn test_build_plutus_witness_set_v2_single() {
        let datum = PlutusData::Integer(1);
        let redeemer = PlutusData::Integer(0);
        let w = ScriptWitness {
            version: PlutusVersion::V2,
            script_bytes: vec![0x01],
            datum_cbor: encode_plutus_data_to_cbor(&datum),
            redeemer_data_cbor: encode_plutus_data_to_cbor(&redeemer),
            ex_units: ExUnits {
                mem: 1000,
                steps: 2000,
            },
        };
        let cbor = build_plutus_witness_set_cbor(&[w]);

        // Must be a valid CBOR map with at least key 4 (datums), 5 (redeemers), 6 (v2 scripts)
        let mut dec = minicbor::Decoder::new(&cbor);
        let map_len = dec.map().unwrap().unwrap() as usize;
        assert!(
            map_len >= 3,
            "Witness set must have at least 3 keys (datums, redeemers, v2 scripts)"
        );
    }

    #[test]
    fn test_collect_plutus_witness_entries_none() {
        let entries = collect_plutus_witness_entries(None).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_collect_plutus_witness_entries_empty_string() {
        let entries = collect_plutus_witness_entries(Some("")).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_tx_body_has_script_data_hash_field_when_script_present() {
        // Build a tx body with a script_data_hash and verify field 11 is present
        let hash = torsten_primitives::hash::Hash32::from_bytes([0xab; 32]);
        let inputs = vec![(Hash32::from_bytes([0x01; 32]), 0)];
        let outputs = vec![];
        let cbor = build_tx_body_cbor(
            &inputs,
            &outputs,
            200_000,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            Some(&hash),
        )
        .unwrap();

        // Decode the map and look for key 11
        let mut dec = minicbor::Decoder::new(&cbor);
        let map_len = dec.map().unwrap().unwrap() as usize;
        let mut found_key_11 = false;
        for _ in 0..map_len {
            let key = dec.u32().unwrap();
            if key == 11 {
                found_key_11 = true;
                // Value must be bytes(32)
                let hash_bytes = dec.bytes().unwrap();
                assert_eq!(hash_bytes.len(), 32);
                assert_eq!(hash_bytes, &[0xab; 32]);
            } else {
                dec.skip().unwrap();
            }
        }
        assert!(found_key_11, "Field 11 (script_data_hash) must be present");
    }

    // ── calculate-min-fee: --tx-in-count / --tx-out-count compat flags ────────

    #[test]
    fn test_estimate_fee_uses_body_size_not_counts() {
        // Both a small body and a large body should produce different fees even
        // if the caller passes the same tx-in-count and tx-out-count.  The flags
        // are cosmetic compat stubs; the fee is derived from the actual CBOR body.
        let small_body = build_tx_body_cbor(
            &[(Hash32::from_bytes([0x01; 32]), 0)],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();
        let large_body = build_tx_body_cbor(
            &[
                (Hash32::from_bytes([0x01; 32]), 0),
                (Hash32::from_bytes([0x02; 32]), 1),
                (Hash32::from_bytes([0x03; 32]), 2),
            ],
            &[],
            0,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();
        let fee_small = estimate_fee(&small_body, 1, 44, 155_381);
        let fee_large = estimate_fee(&large_body, 1, 44, 155_381);
        assert!(
            fee_large > fee_small,
            "more inputs → larger body → higher fee"
        );
    }

    // ── calculate-min-required-utxo: min_ada_for_output ──────────────────────

    #[test]
    fn test_min_ada_for_output_no_tokens() {
        // ADA-only output: formula is max(1_000_000, coins_per_utxo_byte * (0 + 160)).
        // With the current mainnet/preview default of 4_310:
        //   4_310 * 160 = 689_600 < 1_000_000 → floor at 1 ADA.
        let result = min_ada_for_output(&[], 4_310);
        assert_eq!(result, 1_000_000, "ADA-only output must floor at 1 ADA");
    }

    #[test]
    fn test_min_ada_for_output_single_native_token() {
        // One native token raises the required ADA above the 1 ADA floor.
        // The exact value depends on the size estimate; just verify it exceeds 1 ADA.
        let tokens = vec![(
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
            "".to_string(),
            1_000u64,
        )];
        let result = min_ada_for_output(&tokens, 4_310);
        assert!(
            result >= 1_000_000,
            "min ADA with tokens must be at least 1 ADA, got {result}"
        );
    }

    #[test]
    fn test_min_ada_for_output_coins_per_utxo_byte_zero() {
        // If coinsPerUTxOByte is 0 (pathological), the formula is 0 but the
        // floor at 1 ADA still applies.
        let result = min_ada_for_output(&[], 0);
        assert_eq!(
            result, 1_000_000,
            "floor must hold even for zero cost param"
        );
    }

    #[test]
    fn test_min_ada_for_output_multiple_policies() {
        // Multiple policies should cost more than one policy.
        let one_policy = vec![(
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
            "token1".to_string(),
            1u64,
        )];
        let two_policies = vec![
            (
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
                "token1".to_string(),
                1u64,
            ),
            (
                "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5".to_string(),
                "token2".to_string(),
                1u64,
            ),
        ];
        let single = min_ada_for_output(&one_policy, 4_310);
        let multi = min_ada_for_output(&two_policies, 4_310);
        assert!(
            multi >= single,
            "two policies must require at least as much ADA as one"
        );
    }
}
