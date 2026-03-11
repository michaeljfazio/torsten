use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use torsten_ledger::SlotConfig;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::Rational;
use torsten_primitives::value::Lovelace;
use tracing::info;

// ──────────────────────────────────────────────────────────────────────────
// Byron genesis
// ──────────────────────────────────────────────────────────────────────────

/// Byron genesis configuration (compatible with cardano-node byron-genesis.json)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ByronGenesis {
    /// AVVM (Ada Voucher Vending Machine) distribution: base64 pubkey → lovelace
    #[serde(default)]
    pub avvm_distr: HashMap<String, String>,
    /// Non-AVVM initial balances: base58 Byron address → lovelace
    #[serde(default)]
    pub non_avvm_balances: HashMap<String, String>,
    /// Bootstrap stakeholders: stakeholder ID → weight
    #[serde(default)]
    pub boot_stakeholders: HashMap<String, serde_json::Value>,
    /// Heavy delegation certificates
    #[serde(default)]
    pub heavy_delegation: HashMap<String, serde_json::Value>,
    /// System start time (POSIX timestamp)
    pub start_time: u64,
    /// Block version data (fee policy, slot duration, etc.)
    #[serde(default)]
    pub block_version_data: ByronBlockVersionData,
    /// Protocol constants (k, protocol magic)
    #[serde(default)]
    pub protocol_consts: ByronProtocolConsts,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ByronBlockVersionData {
    #[serde(default)]
    pub slot_duration: String,
    #[serde(default)]
    pub max_block_size: String,
    #[serde(default)]
    pub max_tx_size: String,
    #[serde(default)]
    pub tx_fee_policy: ByronTxFeePolicy,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ByronTxFeePolicy {
    /// Fee = summand + multiplier * tx_size (both values are ×1e12)
    #[serde(default)]
    pub summand: String,
    #[serde(default)]
    pub multiplier: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ByronProtocolConsts {
    pub k: u64,
    pub protocol_magic: u64,
}

/// A genesis UTxO entry (address bytes + lovelace amount)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GenesisUtxoEntry {
    pub address: Vec<u8>,
    pub lovelace: u64,
}

impl ByronGenesis {
    #[allow(dead_code)]
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Byron genesis: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Byron genesis: {}", path.display()))
    }

    /// Load the Byron genesis and compute its Blake2b-256 hash.
    ///
    /// The hash is computed over the raw file content (canonical JSON), matching
    /// the Cardano reference implementation.
    pub fn load_with_hash(path: &Path) -> Result<(Self, torsten_primitives::hash::Hash32)> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Byron genesis: {}", path.display()))?;
        let genesis: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Byron genesis: {}", path.display()))?;
        let hash = torsten_primitives::hash::blake2b_256(content.as_bytes());
        info!(
            genesis_hash = %hash.to_hex(),
            "Byron genesis hash computed"
        );
        Ok((genesis, hash))
    }

    /// Get the protocol magic from the genesis config
    pub fn protocol_magic(&self) -> u64 {
        self.protocol_consts.protocol_magic
    }

    /// Get the security parameter k
    pub fn security_param(&self) -> u64 {
        self.protocol_consts.k
    }

    /// Extract the initial UTxO set from nonAvvmBalances.
    ///
    /// Returns decoded address bytes and lovelace amounts for all non-zero balances.
    pub fn initial_utxos(&self) -> Vec<GenesisUtxoEntry> {
        let mut entries = Vec::new();

        for (addr_str, lovelace_str) in &self.non_avvm_balances {
            let lovelace: u64 = match lovelace_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if lovelace == 0 {
                continue;
            }

            // Decode base58 Byron address
            match bs58::decode(addr_str).into_vec() {
                Ok(addr_bytes) => {
                    entries.push(GenesisUtxoEntry {
                        address: addr_bytes,
                        lovelace,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to decode Byron genesis address: {}: {}",
                        &addr_str[..40.min(addr_str.len())],
                        e
                    );
                }
            }
        }

        info!(
            count = entries.len(),
            total_lovelace = entries.iter().map(|e| e.lovelace).sum::<u64>(),
            "Byron genesis: extracted initial UTxOs"
        );

        entries
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Shelley genesis
// ──────────────────────────────────────────────────────────────────────────

/// Shelley genesis configuration (compatible with cardano-node shelley-genesis.json)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ShelleyGenesis {
    pub network_magic: u64,
    pub network_id: String,
    pub system_start: String,
    pub active_slots_coeff: f64,
    pub security_param: u64,
    pub epoch_length: u64,
    pub slot_length: u64,
    pub max_lovelace_supply: u64,
    pub max_k_e_s_evolutions: u64,
    pub slots_per_k_e_s_period: u64,
    pub update_quorum: u64,
    pub protocol_params: ShelleyGenesisProtocolParams,
}

/// Protocol parameters as specified in Shelley genesis
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ShelleyGenesisProtocolParams {
    pub min_fee_a: u64,
    pub min_fee_b: u64,
    pub max_block_body_size: u64,
    pub max_tx_size: u64,
    pub max_block_header_size: u64,
    pub key_deposit: u64,
    pub pool_deposit: u64,
    pub e_max: u64,
    #[serde(alias = "nOpt")]
    pub n_opt: u64,
    pub a0: f64,
    pub rho: f64,
    pub tau: f64,
    pub min_pool_cost: u64,
    #[serde(default)]
    pub min_u_tx_o_value: u64,
    pub protocol_version: ProtocolVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolVersion {
    pub major: u64,
    pub minor: u64,
}

impl ShelleyGenesis {
    /// Load the Shelley genesis and compute its Blake2b-256 hash.
    ///
    /// The hash is computed over the raw file content (canonical JSON), matching
    /// the Cardano reference implementation. This hash is used as the initial
    /// value for the rolling nonce (eta_v) in consensus.
    pub fn load_with_hash(path: &Path) -> Result<(Self, torsten_primitives::hash::Hash32)> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Shelley genesis: {}", path.display()))?;
        let genesis: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Shelley genesis: {}", path.display()))?;
        let hash = torsten_primitives::hash::blake2b_256(content.as_bytes());
        info!(
            genesis_hash = %hash.to_hex(),
            "Shelley genesis hash computed"
        );
        Ok((genesis, hash))
    }

    /// Apply genesis parameters to protocol parameters, keeping Conway-era
    /// defaults for fields not present in Shelley genesis.
    pub fn apply_to_protocol_params(&self, params: &mut ProtocolParameters) {
        let gp = &self.protocol_params;
        params.min_fee_a = gp.min_fee_a;
        params.min_fee_b = gp.min_fee_b;
        params.max_block_body_size = gp.max_block_body_size;
        params.max_tx_size = gp.max_tx_size;
        params.max_block_header_size = gp.max_block_header_size;
        params.key_deposit = Lovelace(gp.key_deposit);
        params.pool_deposit = Lovelace(gp.pool_deposit);
        params.e_max = gp.e_max;
        params.n_opt = gp.n_opt;
        params.a0 = float_to_rational(gp.a0);
        params.rho = float_to_rational(gp.rho);
        params.tau = float_to_rational(gp.tau);
        params.min_pool_cost = Lovelace(gp.min_pool_cost);
        params.protocol_version_major = gp.protocol_version.major;
        params.protocol_version_minor = gp.protocol_version.minor;
        params.active_slots_coeff = self.active_slots_coeff;
    }

    /// Derive the SlotConfig for Plutus time conversion from Shelley genesis.
    ///
    /// system_start is an ISO-8601 timestamp (e.g. "2022-10-25T00:00:00Z").
    /// On mainnet, Shelley started at a later slot; for testnets zero_slot is typically 0.
    pub fn slot_config(&self) -> SlotConfig {
        let zero_time = chrono::DateTime::parse_from_rfc3339(&self.system_start)
            .map(|dt| dt.timestamp_millis() as u64)
            .unwrap_or(0);
        // slot_length in genesis is in seconds; SlotConfig needs milliseconds
        let slot_length_ms = (self.slot_length * 1000) as u32;
        SlotConfig {
            zero_time,
            zero_slot: 0,
            slot_length: slot_length_ms,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Alonzo genesis
// ──────────────────────────────────────────────────────────────────────────

/// Alonzo genesis configuration (compatible with cardano-node alonzo-genesis.json)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AlonzoGenesis {
    pub lovelace_per_u_tx_o_word: Option<u64>,
    pub execution_prices: AlonzoExPrices,
    pub max_tx_ex_units: AlonzoExUnits,
    pub max_block_ex_units: AlonzoExUnits,
    pub max_value_size: u64,
    pub collateral_percentage: u64,
    pub max_collateral_inputs: u64,
    #[serde(default)]
    pub cost_models: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlonzoExPrices {
    pub pr_steps: AlonzoRational,
    pub pr_mem: AlonzoRational,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AlonzoRational {
    Struct { numerator: u64, denominator: u64 },
    Float(f64),
}

impl AlonzoRational {
    pub fn to_rational(&self) -> Rational {
        match self {
            AlonzoRational::Struct {
                numerator,
                denominator,
            } => Rational {
                numerator: *numerator,
                denominator: *denominator,
            },
            AlonzoRational::Float(f) => float_to_rational(*f),
        }
    }

    pub fn numerator(&self) -> u64 {
        self.to_rational().numerator
    }

    pub fn denominator(&self) -> u64 {
        self.to_rational().denominator
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlonzoExUnits {
    pub ex_units_mem: u64,
    pub ex_units_steps: u64,
}

impl AlonzoGenesis {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Alonzo genesis: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Alonzo genesis: {}", path.display()))
    }

    /// Apply Alonzo genesis parameters to protocol parameters
    pub fn apply_to_protocol_params(&self, params: &mut ProtocolParameters) {
        info!(
            max_tx_ex_mem = self.max_tx_ex_units.ex_units_mem,
            max_tx_ex_steps = self.max_tx_ex_units.ex_units_steps,
            max_val_size = self.max_value_size,
            collateral_pct = self.collateral_percentage,
            "Applying Alonzo genesis params"
        );

        // Execution unit prices
        params.execution_costs.step_price = Rational {
            numerator: self.execution_prices.pr_steps.numerator(),
            denominator: self.execution_prices.pr_steps.denominator(),
        };
        params.execution_costs.mem_price = Rational {
            numerator: self.execution_prices.pr_mem.numerator(),
            denominator: self.execution_prices.pr_mem.denominator(),
        };

        // Execution unit limits
        params.max_tx_ex_units.mem = self.max_tx_ex_units.ex_units_mem;
        params.max_tx_ex_units.steps = self.max_tx_ex_units.ex_units_steps;
        params.max_block_ex_units.mem = self.max_block_ex_units.ex_units_mem;
        params.max_block_ex_units.steps = self.max_block_ex_units.ex_units_steps;

        // Size and collateral
        params.max_val_size = self.max_value_size;
        params.collateral_percentage = self.collateral_percentage;
        params.max_collateral_inputs = self.max_collateral_inputs;

        // UTxO cost
        if let Some(lovelace_per_word) = self.lovelace_per_u_tx_o_word {
            // Convert lovelacePerUTxOWord to adaPerUTxOByte
            // 1 word = 8 bytes, so per-byte cost = per-word / 8
            // But Babbage uses adaPerUTxOByte directly; for Alonzo era we approximate
            params.ada_per_utxo_byte = Lovelace(lovelace_per_word / 8);
        }

        // Cost models
        if let Some(v1_value) = self.cost_models.get("PlutusV1") {
            if let Some(costs) = parse_cost_model(v1_value) {
                info!(count = costs.len(), "Loaded PlutusV1 cost model");
                params.cost_models.plutus_v1 = Some(costs);
            }
        }
        if let Some(v2_value) = self.cost_models.get("PlutusV2") {
            if let Some(costs) = parse_cost_model(v2_value) {
                info!(count = costs.len(), "Loaded PlutusV2 cost model");
                params.cost_models.plutus_v2 = Some(costs);
            }
        }
        // PlutusV3 may also appear in Alonzo genesis on newer testnets
        if let Some(v3_value) = self.cost_models.get("PlutusV3") {
            if let Some(costs) = parse_cost_model(v3_value) {
                info!(
                    count = costs.len(),
                    "Loaded PlutusV3 cost model from Alonzo genesis"
                );
                params.cost_models.plutus_v3 = Some(costs);
            }
        }
    }
}

/// Parse a cost model from JSON.
///
/// Cost models come in several formats:
/// - Array of integers: `[val1, val2, ...]`
/// - Indexed map: `{"key-0": val, "key-1": val, ...}` (Conway genesis)
/// - Named map: `{"paramName": val, ...}` (Alonzo genesis) — sorted alphabetically
fn parse_cost_model(value: &serde_json::Value) -> Option<Vec<i64>> {
    match value {
        serde_json::Value::Array(arr) => {
            let costs: Vec<i64> = arr.iter().filter_map(|v| v.as_i64()).collect();
            if costs.len() == arr.len() {
                Some(costs)
            } else {
                None
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return None;
            }
            // Check if keys are "key-N" format (indexed)
            // Safety: map.is_empty() is checked above, so .next() always returns Some
            let first_key = map.keys().next().expect("map is non-empty (checked above)");
            if first_key.starts_with("key-") {
                let mut indexed: Vec<(usize, i64)> = Vec::new();
                for (k, v) in map {
                    if let Some(idx) = k.strip_prefix("key-").and_then(|s| s.parse::<usize>().ok())
                    {
                        if let Some(val) = v.as_i64() {
                            indexed.push((idx, val));
                        }
                    }
                }
                indexed.sort_by_key(|(idx, _)| *idx);
                Some(indexed.into_iter().map(|(_, v)| v).collect())
            } else {
                // Named parameters (Alonzo genesis format) — sort alphabetically
                let mut named: Vec<(&String, i64)> = map
                    .iter()
                    .filter_map(|(k, v)| v.as_i64().map(|val| (k, val)))
                    .collect();
                named.sort_by_key(|(k, _)| k.to_owned());
                Some(named.into_iter().map(|(_, v)| v).collect())
            }
        }
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Conway genesis
// ──────────────────────────────────────────────────────────────────────────

/// Conway genesis configuration (compatible with cardano-node conway-genesis.json)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ConwayGenesis {
    pub pool_voting_thresholds: PoolVotingThresholds,
    #[serde(alias = "dRepVotingThresholds")]
    pub d_rep_voting_thresholds: DRepVotingThresholds,
    pub committee_min_size: u64,
    pub committee_max_term_length: u64,
    pub gov_action_lifetime: u64,
    pub gov_action_deposit: u64,
    #[serde(alias = "dRepDeposit")]
    pub d_rep_deposit: u64,
    #[serde(alias = "dRepActivity")]
    pub d_rep_activity: u64,
    #[serde(default)]
    pub min_fee_ref_script_cost_per_byte: Option<u64>,
    #[serde(default)]
    pub plutus_v3_cost_model: Option<Vec<i64>>,
    #[serde(default)]
    pub constitution: Option<serde_json::Value>,
    #[serde(default)]
    pub committee: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct PoolVotingThresholds {
    pub committee_normal: f64,
    pub committee_no_confidence: f64,
    pub hard_fork_initiation: f64,
    pub motion_no_confidence: f64,
    #[serde(default)]
    pub pp_security_group: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DRepVotingThresholds {
    pub motion_no_confidence: f64,
    pub committee_normal: f64,
    pub committee_no_confidence: f64,
    pub update_to_constitution: f64,
    pub hard_fork_initiation: f64,
    #[serde(default)]
    pub pp_network_group: f64,
    #[serde(default)]
    pub pp_economic_group: f64,
    #[serde(default)]
    pub pp_technical_group: f64,
    #[serde(default)]
    pub pp_gov_group: f64,
    pub treasury_withdrawal: f64,
}

impl ConwayGenesis {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Conway genesis: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Conway genesis: {}", path.display()))
    }

    /// Apply Conway genesis parameters to protocol parameters
    pub fn apply_to_protocol_params(&self, params: &mut ProtocolParameters) {
        info!(
            drep_deposit = self.d_rep_deposit,
            drep_activity = self.d_rep_activity,
            gov_action_deposit = self.gov_action_deposit,
            gov_action_lifetime = self.gov_action_lifetime,
            committee_min_size = self.committee_min_size,
            "Applying Conway genesis params"
        );

        // Governance parameters
        params.drep_deposit = Lovelace(self.d_rep_deposit);
        params.drep_activity = self.d_rep_activity;
        params.gov_action_deposit = Lovelace(self.gov_action_deposit);
        params.gov_action_lifetime = self.gov_action_lifetime;
        params.committee_min_size = self.committee_min_size;
        params.committee_max_term_length = self.committee_max_term_length;

        // DRep voting thresholds
        let dvt = &self.d_rep_voting_thresholds;
        params.dvt_no_confidence = float_to_rational(dvt.motion_no_confidence);
        params.dvt_committee_normal = float_to_rational(dvt.committee_normal);
        params.dvt_committee_no_confidence = float_to_rational(dvt.committee_no_confidence);
        params.dvt_constitution = float_to_rational(dvt.update_to_constitution);
        params.dvt_hard_fork = float_to_rational(dvt.hard_fork_initiation);
        params.dvt_treasury_withdrawal = float_to_rational(dvt.treasury_withdrawal);
        params.dvt_pp_network_group = float_to_rational(dvt.pp_network_group);
        params.dvt_pp_economic_group = float_to_rational(dvt.pp_economic_group);
        params.dvt_pp_technical_group = float_to_rational(dvt.pp_technical_group);
        params.dvt_pp_gov_group = float_to_rational(dvt.pp_gov_group);

        if let Some(cost) = self.min_fee_ref_script_cost_per_byte {
            params.min_fee_ref_script_cost_per_byte = cost;
        }

        // PlutusV3 cost model from Conway genesis
        if let Some(v3) = &self.plutus_v3_cost_model {
            info!(
                count = v3.len(),
                "Loaded PlutusV3 cost model from Conway genesis"
            );
            params.cost_models.plutus_v3 = Some(v3.clone());
        }

        // Pool voting thresholds
        let pvt = &self.pool_voting_thresholds;
        params.pvt_motion_no_confidence = float_to_rational(pvt.motion_no_confidence);
        params.pvt_committee_normal = float_to_rational(pvt.committee_normal);
        params.pvt_committee_no_confidence = float_to_rational(pvt.committee_no_confidence);
        params.pvt_hard_fork = float_to_rational(pvt.hard_fork_initiation);
        params.pvt_pp_security_group = float_to_rational(pvt.pp_security_group);
    }

    /// Extract the committee quorum threshold from Conway genesis.
    /// Returns (numerator, denominator) if the committee section has a threshold.
    pub fn committee_threshold(&self) -> Option<(u64, u64)> {
        let committee = self.committee.as_ref()?;
        let threshold = committee.get("threshold")?;
        let num = threshold.get("numerator")?.as_u64()?;
        let den = threshold.get("denominator")?.as_u64()?;
        Some((num, den))
    }

    /// Extract committee members from Conway genesis.
    ///
    /// Returns a list of (credential_hash_bytes, expiration_epoch) pairs.
    /// Keys in genesis are formatted as "scriptHash-<hex>" or "keyHash-<hex>".
    pub fn committee_members(&self) -> Vec<([u8; 32], u64)> {
        let committee = match self.committee.as_ref() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let members = match committee.get("members").and_then(|m| m.as_object()) {
            Some(m) => m,
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        for (key, expiry) in members {
            let expiration = match expiry.as_u64() {
                Some(e) => e,
                None => continue,
            };
            // Parse "scriptHash-<hex>" or "keyHash-<hex>" format
            let hex_str = if let Some(h) = key.strip_prefix("scriptHash-") {
                h
            } else if let Some(h) = key.strip_prefix("keyHash-") {
                h
            } else {
                continue;
            };
            if let Ok(bytes) = hex::decode(hex_str) {
                // Committee credentials are 28 bytes; pad to 32 for our Hash32 representation
                let mut hash = [0u8; 32];
                let len = bytes.len().min(32);
                hash[..len].copy_from_slice(&bytes[..len]);
                result.push((hash, expiration));
            }
        }
        result
    }
}

/// Convert a float to a rational approximation
fn float_to_rational(f: f64) -> Rational {
    if f == 0.0 {
        return Rational {
            numerator: 0,
            denominator: 1,
        };
    }
    // Use 1_000_000 as denominator for good precision
    let den = 1_000_000u64;
    let num = (f * den as f64).round() as u64;
    // Simplify with GCD
    let g = gcd(num, den);
    Rational {
        numerator: num / g,
        denominator: den / g,
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float_to_rational() {
        let r = float_to_rational(0.3);
        assert_eq!(r.numerator, 3);
        assert_eq!(r.denominator, 10);

        let r = float_to_rational(0.05);
        assert_eq!(r.numerator, 1);
        assert_eq!(r.denominator, 20);

        let r = float_to_rational(0.003);
        assert_eq!(r.numerator, 3);
        assert_eq!(r.denominator, 1000);
    }

    #[test]
    fn test_parse_alonzo_genesis() {
        let json = r#"{
            "lovelacePerUTxOWord": 34482,
            "executionPrices": {
                "prSteps": { "numerator": 721, "denominator": 10000000 },
                "prMem": { "numerator": 577, "denominator": 10000 }
            },
            "maxTxExUnits": { "exUnitsMem": 10000000, "exUnitsSteps": 10000000000 },
            "maxBlockExUnits": { "exUnitsMem": 50000000, "exUnitsSteps": 40000000000 },
            "maxValueSize": 5000,
            "collateralPercentage": 150,
            "maxCollateralInputs": 3,
            "costModels": {
                "PlutusV1": {}
            }
        }"#;

        let genesis: AlonzoGenesis = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.max_value_size, 5000);
        assert_eq!(genesis.collateral_percentage, 150);
        assert_eq!(genesis.max_collateral_inputs, 3);
        assert_eq!(genesis.max_tx_ex_units.ex_units_mem, 10000000);
        assert_eq!(genesis.max_block_ex_units.ex_units_steps, 40000000000);
        assert_eq!(genesis.execution_prices.pr_steps.numerator(), 721);
        assert_eq!(genesis.execution_prices.pr_mem.denominator(), 10000);

        let mut pp = ProtocolParameters::mainnet_defaults();
        genesis.apply_to_protocol_params(&mut pp);
        assert_eq!(pp.max_val_size, 5000);
        assert_eq!(pp.collateral_percentage, 150);
        assert_eq!(pp.max_tx_ex_units.mem, 10000000);
        assert_eq!(pp.execution_costs.step_price.numerator, 721);
    }

    #[test]
    fn test_parse_conway_genesis() {
        let json = r#"{
            "poolVotingThresholds": {
                "committeeNormal": 0.51,
                "committeeNoConfidence": 0.51,
                "hardForkInitiation": 0.51,
                "motionNoConfidence": 0.51,
                "ppSecurityGroup": 0.51
            },
            "dRepVotingThresholds": {
                "motionNoConfidence": 0.67,
                "committeeNormal": 0.67,
                "committeeNoConfidence": 0.6,
                "updateToConstitution": 0.75,
                "hardForkInitiation": 0.6,
                "ppNetworkGroup": 0.67,
                "ppEconomicGroup": 0.67,
                "ppTechnicalGroup": 0.67,
                "ppGovGroup": 0.75,
                "treasuryWithdrawal": 0.67
            },
            "committeeMinSize": 7,
            "committeeMaxTermLength": 146,
            "govActionLifetime": 6,
            "govActionDeposit": 100000000000,
            "dRepDeposit": 500000000,
            "dRepActivity": 20,
            "minFeeRefScriptCostPerByte": 15
        }"#;

        let genesis: ConwayGenesis = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.committee_min_size, 7);
        assert_eq!(genesis.d_rep_deposit, 500000000);
        assert_eq!(genesis.gov_action_deposit, 100000000000);
        assert_eq!(genesis.d_rep_activity, 20);

        let mut pp = ProtocolParameters::mainnet_defaults();
        genesis.apply_to_protocol_params(&mut pp);
        assert_eq!(pp.drep_deposit, Lovelace(500000000));
        assert_eq!(pp.gov_action_deposit, Lovelace(100000000000));
        assert_eq!(pp.committee_min_size, 7);
        assert_eq!(pp.committee_max_term_length, 146);
        // DRep voting thresholds
        assert_eq!(pp.dvt_constitution.numerator, 3);
        assert_eq!(pp.dvt_constitution.denominator, 4); // 0.75

        // No committee section → empty members and no threshold
        assert!(genesis.committee_threshold().is_none());
        assert!(genesis.committee_members().is_empty());
    }

    #[test]
    fn test_conway_genesis_committee_members() {
        let json = r#"{
            "poolVotingThresholds": {
                "committeeNormal": 0.51, "committeeNoConfidence": 0.51,
                "hardForkInitiation": 0.51, "motionNoConfidence": 0.51, "ppSecurityGroup": 0.51
            },
            "dRepVotingThresholds": {
                "motionNoConfidence": 0.67, "committeeNormal": 0.67, "committeeNoConfidence": 0.6,
                "updateToConstitution": 0.75, "hardForkInitiation": 0.6, "ppNetworkGroup": 0.67,
                "ppEconomicGroup": 0.67, "ppTechnicalGroup": 0.67, "ppGovGroup": 0.75,
                "treasuryWithdrawal": 0.67
            },
            "committeeMinSize": 1,
            "committeeMaxTermLength": 146,
            "govActionLifetime": 6,
            "govActionDeposit": 100000000,
            "dRepDeposit": 500000000,
            "dRepActivity": 20,
            "committee": {
                "members": {
                    "scriptHash-ff9babf23fef3f54ec29132c07a8e23807d7b395b143ecd8ff79f4c7": 1000,
                    "keyHash-aabbccdd00112233445566778899aabbccddeeff00112233445566778899aabb": 500
                },
                "threshold": { "numerator": 2, "denominator": 3 }
            }
        }"#;

        let genesis: ConwayGenesis = serde_json::from_str(json).unwrap();

        // Threshold
        let (num, den) = genesis.committee_threshold().unwrap();
        assert_eq!(num, 2);
        assert_eq!(den, 3);

        // Members
        let members = genesis.committee_members();
        assert_eq!(members.len(), 2);

        // Check the scriptHash member (28-byte credential padded to 32)
        let script_hash_hex = "ff9babf23fef3f54ec29132c07a8e23807d7b395b143ecd8ff79f4c7";
        let expected_bytes = hex::decode(script_hash_hex).unwrap();
        let found = members.iter().any(|(hash, exp)| {
            hash[..28] == expected_bytes[..] && hash[28..] == [0, 0, 0, 0] && *exp == 1000
        });
        assert!(found, "scriptHash member not found with correct expiration");

        // Check keyHash member
        let found_key = members.iter().any(|(_, exp)| *exp == 500);
        assert!(
            found_key,
            "keyHash member not found with correct expiration"
        );
    }

    #[test]
    fn test_parse_byron_genesis() {
        let json = r#"{
            "avvmDistr": {
                "Y2FyZGFubyBpcyBhd2Vzb21l": "1000000"
            },
            "nonAvvmBalances": {
                "37btjrVyb4KEB2STADSsj3MYSAdj52X9FgGzKZEiHbsyZH1r39ZZRH6FvkSRMxaVBMPKknvEPYhHPV1Qgr6FSNLF1sfhaMQ4bDYB2Y3FNkPZCz": "3333000000",
                "2cWKMJemoBajcwN6kT4oHXBH5JTwHtCFhVYKDRAS1QbjKZJj8GUZPF7v9G5DxaJfmUqidz": "999000000"
            },
            "bootStakeholders": {},
            "heavyDelegation": {},
            "startTime": 1654041600,
            "blockVersionData": {
                "slotDuration": "20000",
                "maxBlockSize": "2000000",
                "maxTxSize": "4096",
                "txFeePolicy": {
                    "summand": "155381000000000",
                    "multiplier": "43946000000"
                }
            },
            "protocolConsts": {
                "k": 2160,
                "protocolMagic": 764824073
            }
        }"#;

        let genesis: ByronGenesis = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.protocol_magic(), 764824073);
        assert_eq!(genesis.security_param(), 2160);
        assert_eq!(genesis.start_time, 1654041600);
        assert_eq!(genesis.non_avvm_balances.len(), 2);
        assert_eq!(genesis.avvm_distr.len(), 1);
        assert_eq!(genesis.block_version_data.slot_duration, "20000");
        assert_eq!(genesis.block_version_data.max_block_size, "2000000");

        // Test initial_utxos extraction
        let utxos = genesis.initial_utxos();
        assert_eq!(utxos.len(), 2);
        // Verify lovelace amounts
        let total: u64 = utxos.iter().map(|e| e.lovelace).sum();
        assert_eq!(total, 3333000000 + 999000000);
    }

    #[test]
    fn test_parse_shelley_genesis() {
        let json = r#"{
            "networkMagic": 2,
            "networkId": "Testnet",
            "systemStart": "2022-10-25T00:00:00Z",
            "activeSlotsCoeff": 0.05,
            "securityParam": 432,
            "epochLength": 86400,
            "slotLength": 1,
            "maxLovelaceSupply": 45000000000000000,
            "maxKESEvolutions": 62,
            "slotsPerKESPeriod": 129600,
            "updateQuorum": 5,
            "protocolParams": {
                "minFeeA": 44,
                "minFeeB": 155381,
                "maxBlockBodySize": 65536,
                "maxTxSize": 16384,
                "maxBlockHeaderSize": 1100,
                "keyDeposit": 2000000,
                "poolDeposit": 500000000,
                "eMax": 18,
                "nOpt": 150,
                "a0": 0.3,
                "rho": 0.003,
                "tau": 0.2,
                "minPoolCost": 340000000,
                "minUTxOValue": 1000000,
                "protocolVersion": { "major": 6, "minor": 0 }
            }
        }"#;

        let genesis: ShelleyGenesis = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.network_magic, 2);
        assert_eq!(genesis.system_start, "2022-10-25T00:00:00Z");
        assert_eq!(genesis.active_slots_coeff, 0.05);
        assert_eq!(genesis.epoch_length, 86400);
        assert_eq!(genesis.protocol_params.n_opt, 150);
        assert_eq!(genesis.protocol_params.min_pool_cost, 340000000);

        // Apply to protocol params
        let mut pp = ProtocolParameters::mainnet_defaults();
        genesis.apply_to_protocol_params(&mut pp);
        assert_eq!(pp.n_opt, 150);
        assert_eq!(pp.min_pool_cost, Lovelace(340000000));
        assert_eq!(pp.max_block_body_size, 65536);
    }

    #[test]
    fn test_byron_genesis_load_with_hash() {
        // Write a temporary Byron genesis JSON file and verify load_with_hash
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("byron-genesis.json");
        let json = r#"{
            "avvmDistr": {},
            "nonAvvmBalances": {},
            "bootStakeholders": {},
            "heavyDelegation": {},
            "startTime": 1654041600,
            "blockVersionData": {
                "slotDuration": "20000",
                "maxBlockSize": "2000000",
                "maxTxSize": "4096",
                "txFeePolicy": { "summand": "155381000000000", "multiplier": "43946000000" }
            },
            "protocolConsts": { "k": 2160, "protocolMagic": 764824073 }
        }"#;
        std::fs::write(&path, json).unwrap();

        let (genesis, hash) = ByronGenesis::load_with_hash(&path).unwrap();
        assert_eq!(genesis.protocol_magic(), 764824073);
        assert_eq!(genesis.security_param(), 2160);

        // Hash should be deterministic for the same content
        let expected = torsten_primitives::hash::blake2b_256(json.as_bytes());
        assert_eq!(hash, expected);

        // Hash should be non-zero
        assert_ne!(hash, torsten_primitives::hash::Hash32::ZERO);
    }

    #[test]
    fn test_shelley_genesis_load_with_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shelley-genesis.json");
        let json = r#"{
            "networkMagic": 2,
            "networkId": "Testnet",
            "systemStart": "2022-10-25T00:00:00Z",
            "activeSlotsCoeff": 0.05,
            "securityParam": 432,
            "epochLength": 86400,
            "slotLength": 1,
            "maxLovelaceSupply": 45000000000000000,
            "maxKESEvolutions": 62,
            "slotsPerKESPeriod": 129600,
            "updateQuorum": 5,
            "protocolParams": {
                "minFeeA": 44,
                "minFeeB": 155381,
                "maxBlockBodySize": 65536,
                "maxTxSize": 16384,
                "maxBlockHeaderSize": 1100,
                "keyDeposit": 2000000,
                "poolDeposit": 500000000,
                "eMax": 18,
                "nOpt": 150,
                "a0": 0.3,
                "rho": 0.003,
                "tau": 0.2,
                "minPoolCost": 340000000,
                "minUTxOValue": 1000000,
                "protocolVersion": { "major": 6, "minor": 0 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let (genesis, hash) = ShelleyGenesis::load_with_hash(&path).unwrap();
        assert_eq!(genesis.network_magic, 2);

        // Hash should be deterministic
        let expected = torsten_primitives::hash::blake2b_256(json.as_bytes());
        assert_eq!(hash, expected);
        assert_ne!(hash, torsten_primitives::hash::Hash32::ZERO);
    }

    #[test]
    fn test_genesis_hash_differs_between_files() {
        let dir = tempfile::tempdir().unwrap();

        let path1 = dir.path().join("genesis1.json");
        let json1 = r#"{
            "avvmDistr": {},
            "nonAvvmBalances": {},
            "bootStakeholders": {},
            "heavyDelegation": {},
            "startTime": 1654041600,
            "blockVersionData": {
                "slotDuration": "20000",
                "maxBlockSize": "2000000",
                "maxTxSize": "4096",
                "txFeePolicy": { "summand": "155381000000000", "multiplier": "43946000000" }
            },
            "protocolConsts": { "k": 2160, "protocolMagic": 764824073 }
        }"#;
        std::fs::write(&path1, json1).unwrap();

        let path2 = dir.path().join("genesis2.json");
        let json2 = r#"{
            "avvmDistr": {},
            "nonAvvmBalances": {},
            "bootStakeholders": {},
            "heavyDelegation": {},
            "startTime": 1654041600,
            "blockVersionData": {
                "slotDuration": "20000",
                "maxBlockSize": "2000000",
                "maxTxSize": "4096",
                "txFeePolicy": { "summand": "155381000000000", "multiplier": "43946000000" }
            },
            "protocolConsts": { "k": 2160, "protocolMagic": 1 }
        }"#;
        std::fs::write(&path2, json2).unwrap();

        let (_, hash1) = ByronGenesis::load_with_hash(&path1).unwrap();
        let (_, hash2) = ByronGenesis::load_with_hash(&path2).unwrap();

        // Different genesis files must produce different hashes
        assert_ne!(hash1, hash2);
    }
}
