//! Test vector schema definitions.
//!
//! These types define the JSON format for conformance test vectors. The schema
//! uses simplified types that mirror the Agda formal specification's abstract
//! types (e.g., TxId as hex string, addresses as simplified structs).
//!
//! The Agda spec uses abstract types that differ from Cardano's concrete wire
//! format. The test vectors bridge this gap by using an intermediate JSON
//! representation that can be generated from both the Agda-compiled Haskell
//! code and hand-crafted examples.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Top-level test vector
// ---------------------------------------------------------------------------

/// A single conformance test vector.
///
/// Each vector specifies an STS rule, provides the environment/state/signal
/// triple, and declares the expected outcome (success with new state, or
/// failure with error descriptions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceTestVector {
    /// STS rule name: "UTXO", "CERT", "GOV", "EPOCH", etc.
    pub rule: String,
    /// Human-readable description of what this test case covers.
    pub description: String,
    /// Rule-specific environment (protocol parameters, slot, etc.).
    pub environment: serde_json::Value,
    /// Pre-state before the transition.
    pub input_state: serde_json::Value,
    /// Signal that triggers the transition (transaction, certificate, etc.).
    pub signal: serde_json::Value,
    /// Expected outcome.
    pub expected_output: ConformanceExpectedOutput,
}

/// Expected outcome of applying the signal to the state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConformanceExpectedOutput {
    /// The transition succeeds, producing a new state.
    #[serde(rename = "success")]
    Success { state: serde_json::Value },
    /// The transition fails with one or more errors.
    #[serde(rename = "failure")]
    Failure { errors: Vec<String> },
}

// ---------------------------------------------------------------------------
// UTXO rule types
// ---------------------------------------------------------------------------

/// Environment for the UTXO rule.
///
/// Maps to the Agda `UTxOEnv` record:
///   record UTxOEnv : Set where
///     slot       : Slot
///     pparams    : PParams
///     treasury   : Coin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoEnvironment {
    /// Current slot number.
    pub slot: u64,
    /// Protocol parameters relevant to UTXO validation.
    pub protocol_params: UtxoProtocolParams,
    /// Current treasury balance (used for donation validation).
    pub treasury: u64,
}

/// Simplified protocol parameters for UTXO validation.
///
/// These correspond to the subset of PParams used by the UTXO STS rule
/// in the formal specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoProtocolParams {
    /// Minimum fee coefficient (per-byte).
    pub min_fee_a: u64,
    /// Minimum fee constant.
    pub min_fee_b: u64,
    /// Maximum transaction size in bytes.
    pub max_tx_size: u64,
    /// Key deposit (lovelace).
    pub key_deposit: u64,
    /// Pool deposit (lovelace).
    pub pool_deposit: u64,
    /// Minimum ADA per UTxO byte.
    pub ada_per_utxo_byte: u64,
    /// Maximum value size in bytes.
    pub max_val_size: u64,
    /// Collateral percentage (for Plutus transactions).
    pub collateral_percentage: u64,
    /// Maximum number of collateral inputs.
    pub max_collateral_inputs: u64,
    /// Maximum transaction execution units (memory).
    pub max_tx_ex_mem: u64,
    /// Maximum transaction execution units (steps).
    pub max_tx_ex_steps: u64,
    /// DRep deposit (Conway governance).
    pub drep_deposit: u64,
    /// Governance action deposit (Conway governance).
    pub gov_action_deposit: u64,
    /// Reference script cost per byte (in lovelace, Conway).
    pub min_fee_ref_script_cost_per_byte: u64,
}

/// UTxO state for conformance tests.
///
/// Maps to the Agda `UTxOState` record:
///   record UTxOState : Set where
///     utxo       : UTxO
///     fees       : Coin
///     deposits   : Deposits
///     donations  : Coin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoState {
    /// The UTxO set: maps (tx_hash, index) pairs to outputs.
    pub utxo: Vec<UtxoEntry>,
    /// Accumulated fees.
    pub fees: u64,
    /// Total deposits held.
    pub deposits: u64,
    /// Total donations (Conway).
    pub donations: u64,
}

/// A single UTxO entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoEntry {
    /// Transaction hash (hex-encoded 32 bytes).
    pub tx_hash: String,
    /// Output index.
    pub index: u32,
    /// The output.
    pub output: TxOutput,
}

/// Simplified transaction output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    /// Address (simplified representation).
    pub address: TestAddress,
    /// Lovelace value.
    pub lovelace: u64,
    /// Optional multi-asset tokens: policy_hex -> { asset_name_hex -> quantity }.
    #[serde(default)]
    pub assets: BTreeMap<String, BTreeMap<String, i64>>,
}

/// Simplified address for test vectors.
///
/// The formal spec uses abstract addresses. We use a simplified representation
/// that maps to Torsten's address types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestAddress {
    /// Base address with payment and stake credentials.
    #[serde(rename = "base")]
    Base {
        network: u8,
        payment: TestCredential,
        stake: TestCredential,
    },
    /// Enterprise address with payment credential only.
    #[serde(rename = "enterprise")]
    Enterprise {
        network: u8,
        payment: TestCredential,
    },
    /// Reward address (for withdrawals/staking).
    #[serde(rename = "reward")]
    Reward { network: u8, stake: TestCredential },
    /// Byron address (raw hex payload).
    #[serde(rename = "byron")]
    Byron { payload_hex: String },
}

/// Simplified credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestCredential {
    /// Verification key hash credential.
    #[serde(rename = "vkey")]
    VKey {
        /// 28-byte key hash, hex-encoded.
        hash: String,
    },
    /// Script hash credential.
    #[serde(rename = "script")]
    Script {
        /// 28-byte script hash, hex-encoded.
        hash: String,
    },
}

/// Simplified transaction for UTXO conformance tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestTransaction {
    /// Transaction hash (hex-encoded 32 bytes).
    pub hash: String,
    /// Transaction inputs.
    pub inputs: Vec<TestInput>,
    /// Transaction outputs.
    pub outputs: Vec<TxOutput>,
    /// Fee in lovelace.
    pub fee: u64,
    /// Time-to-live (optional slot).
    pub ttl: Option<u64>,
    /// Validity interval start (optional slot).
    pub validity_start: Option<u64>,
    /// Certificates.
    #[serde(default)]
    pub certificates: Vec<TestCertificate>,
    /// Withdrawals: reward_address_hex -> lovelace.
    #[serde(default)]
    pub withdrawals: BTreeMap<String, u64>,
    /// Minting: policy_hex -> { asset_name_hex -> quantity }.
    #[serde(default)]
    pub mint: BTreeMap<String, BTreeMap<String, i64>>,
    /// Transaction size in bytes (for fee calculation).
    pub tx_size: u64,
    /// Whether the transaction is valid (for Plutus).
    #[serde(default = "default_true")]
    pub is_valid: bool,
    /// Required signers (hex-encoded 32-byte key hashes).
    #[serde(default)]
    pub required_signers: Vec<String>,
    /// Collateral inputs.
    #[serde(default)]
    pub collateral: Vec<TestInput>,
    /// Collateral return output.
    pub collateral_return: Option<TxOutput>,
    /// Total collateral declared.
    pub total_collateral: Option<u64>,
    /// Reference inputs.
    #[serde(default)]
    pub reference_inputs: Vec<TestInput>,
    /// Proposal procedures (Conway governance).
    #[serde(default)]
    pub proposal_procedures: Vec<serde_json::Value>,
    /// Treasury donation (Conway).
    pub donation: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// Transaction input reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestInput {
    /// Transaction hash (hex-encoded 32 bytes).
    pub tx_hash: String,
    /// Output index.
    pub index: u32,
}

// ---------------------------------------------------------------------------
// CERT rule types
// ---------------------------------------------------------------------------

/// Environment for the CERT rule.
///
/// Maps to the Agda `CertEnv` record:
///   record CertEnv : Set where
///     epoch    : Epoch
///     pp       : PParams
///     votes    : List GovVote
///     wdrls    : RwdAddr -> Coin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertEnvironment {
    /// Current epoch.
    pub epoch: u64,
    /// Protocol parameters for deposit calculation.
    pub protocol_params: CertProtocolParams,
    /// Governance votes relevant to this certificate (Conway).
    #[serde(default)]
    pub votes: Vec<serde_json::Value>,
    /// Withdrawals being processed in this transaction.
    #[serde(default)]
    pub withdrawals: BTreeMap<String, u64>,
}

/// Protocol parameters subset for CERT rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertProtocolParams {
    /// Key deposit (lovelace).
    pub key_deposit: u64,
    /// Pool deposit (lovelace).
    pub pool_deposit: u64,
    /// DRep deposit (Conway, lovelace).
    pub drep_deposit: u64,
    /// Minimum pool cost.
    pub min_pool_cost: u64,
    /// DRep activity period (epochs).
    pub drep_activity: u64,
    /// Maximum pool retirement epoch offset.
    pub e_max: u64,
}

/// Delegation state for CERT conformance tests.
///
/// Maps to the Agda `CertState` record:
///   record CertState : Set where
///     dState : DState
///     pState : PState
///     gState : GState
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertState {
    /// Delegation sub-state.
    pub d_state: DelegationSubState,
    /// Pool sub-state.
    pub p_state: PoolSubState,
    /// Governance sub-state (Conway).
    pub g_state: GovernanceSubState,
}

/// Delegation sub-state (DState).
///
/// Tracks stake credential registrations, delegations, and rewards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationSubState {
    /// Registered stake credentials with their deposits.
    /// credential_hash_hex -> deposit_lovelace
    pub registrations: BTreeMap<String, u64>,
    /// Stake delegations: credential_hash_hex -> pool_id_hex.
    pub delegations: BTreeMap<String, String>,
    /// Reward accounts: credential_hash_hex -> reward_lovelace.
    pub rewards: BTreeMap<String, u64>,
    /// Vote delegations: credential_hash_hex -> drep.
    #[serde(default)]
    pub vote_delegations: BTreeMap<String, TestDRep>,
}

/// Pool sub-state (PState).
///
/// Tracks pool registrations and retirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSubState {
    /// Registered pools: pool_id_hex -> pool_params.
    pub pools: BTreeMap<String, TestPoolParams>,
    /// Pending retirements: pool_id_hex -> retirement_epoch.
    pub retiring: BTreeMap<String, u64>,
}

/// Governance sub-state (GState, Conway era).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceSubState {
    /// Registered DReps: credential_hash_hex -> active_until_epoch.
    #[serde(default)]
    pub dreps: BTreeMap<String, u64>,
    /// Committee hot key authorizations: cold_hash_hex -> hot_hash_hex.
    #[serde(default)]
    pub committee_hot_keys: BTreeMap<String, String>,
}

/// Simplified DRep for test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestDRep {
    #[serde(rename = "key_hash")]
    KeyHash { hash: String },
    #[serde(rename = "script_hash")]
    ScriptHash { hash: String },
    #[serde(rename = "abstain")]
    Abstain,
    #[serde(rename = "no_confidence")]
    NoConfidence,
}

/// Simplified pool parameters for test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestPoolParams {
    /// Pool operator key hash (28-byte hex).
    pub operator: String,
    /// VRF key hash (32-byte hex).
    pub vrf_keyhash: String,
    /// Pledge in lovelace.
    pub pledge: u64,
    /// Fixed cost in lovelace.
    pub cost: u64,
    /// Margin as [numerator, denominator].
    pub margin: [u64; 2],
    /// Reward account (hex-encoded).
    pub reward_account: String,
    /// Pool owners (28-byte hex each).
    pub owners: Vec<String>,
}

/// Certificate for CERT conformance tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestCertificate {
    #[serde(rename = "stake_registration")]
    StakeRegistration { credential: TestCredential },

    #[serde(rename = "stake_deregistration")]
    StakeDeregistration { credential: TestCredential },

    #[serde(rename = "stake_delegation")]
    StakeDelegation {
        credential: TestCredential,
        pool_hash: String,
    },

    #[serde(rename = "pool_registration")]
    PoolRegistration { params: TestPoolParams },

    #[serde(rename = "pool_retirement")]
    PoolRetirement { pool_hash: String, epoch: u64 },

    #[serde(rename = "reg_drep")]
    RegDRep {
        credential: TestCredential,
        deposit: u64,
    },

    #[serde(rename = "unreg_drep")]
    UnregDRep {
        credential: TestCredential,
        refund: u64,
    },

    #[serde(rename = "vote_delegation")]
    VoteDelegation {
        credential: TestCredential,
        drep: TestDRep,
    },

    #[serde(rename = "conway_stake_registration")]
    ConwayStakeRegistration {
        credential: TestCredential,
        deposit: u64,
    },

    #[serde(rename = "conway_stake_deregistration")]
    ConwayStakeDeregistration {
        credential: TestCredential,
        refund: u64,
    },
}

// ---------------------------------------------------------------------------
// Test result types
// ---------------------------------------------------------------------------

/// Result of running a single conformance test.
#[derive(Debug)]
pub struct ConformanceTestResult {
    /// Path to the test vector file.
    pub vector_path: String,
    /// The rule being tested.
    pub rule: String,
    /// Description of the test case.
    pub description: String,
    /// Whether the test passed.
    pub passed: bool,
    /// Detailed mismatch information if the test failed.
    pub details: Option<String>,
}

impl std::fmt::Display for ConformanceTestResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.passed { "PASS" } else { "FAIL" };
        write!(
            f,
            "[{}] {} - {} ({})",
            status, self.rule, self.description, self.vector_path
        )?;
        if let Some(ref details) = self.details {
            write!(f, "\n  {}", details)?;
        }
        Ok(())
    }
}
