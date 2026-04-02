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
/// that maps to Dugite's address types.
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
    /// Network ID (0 = testnet, 1 = mainnet). When set, outputs must match.
    pub network_id: Option<u8>,
    /// Auxiliary data hash (hex-encoded 32 bytes). When set without auxiliary_data, triggers error.
    pub auxiliary_data_hash: Option<String>,
    /// Whether auxiliary data is present (simulated).
    #[serde(default)]
    pub has_auxiliary_data: bool,
    /// Whether the transaction has Plutus redeemers (triggers collateral checks).
    #[serde(default)]
    pub has_redeemers: bool,
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
// GOV rule types
// ---------------------------------------------------------------------------

/// Environment for the GOV rule.
///
/// Maps to the Agda `GovEnv` record:
///   record GovEnv : Set where
///     txid       : TxId
///     epoch      : Epoch
///     pparams    : PParams
///     ppolicy    : Maybe ScriptHash
///     enactState : EnactState
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovEnvironment {
    /// Transaction hash that contains the proposal/vote.
    pub tx_hash: String,
    /// Current epoch.
    pub epoch: u64,
    /// Protocol parameters for governance.
    pub protocol_params: GovProtocolParams,
    /// Gov action lifetime (epochs).
    pub gov_action_lifetime: u64,
}

/// Protocol parameters subset for GOV rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovProtocolParams {
    /// Governance action deposit (lovelace).
    pub gov_action_deposit: u64,
    /// DRep deposit (lovelace).
    pub drep_deposit: u64,
}

/// Governance state for GOV conformance tests.
///
/// Contains active proposals, votes, and enacted roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovState {
    /// Active proposals: action_id_key -> proposal_state.
    #[serde(default)]
    pub proposals: BTreeMap<String, TestProposalState>,
    /// Votes by action: action_id_key -> [(voter, vote)].
    #[serde(default)]
    pub votes: BTreeMap<String, Vec<TestVoteEntry>>,
    /// Total proposals submitted.
    #[serde(default)]
    pub proposal_count: u64,
}

/// A governance proposal state in test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestProposalState {
    /// The governance action type.
    pub action_type: String,
    /// Deposit amount.
    pub deposit: u64,
    /// Return address (hex-encoded).
    pub return_addr: String,
    /// Proposed epoch.
    pub proposed_epoch: u64,
    /// Expiration epoch.
    pub expires_epoch: u64,
    /// Vote tallies.
    #[serde(default)]
    pub yes_votes: u64,
    #[serde(default)]
    pub no_votes: u64,
    #[serde(default)]
    pub abstain_votes: u64,
}

/// A vote entry in test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestVoteEntry {
    /// Voter type: "drep", "spo", "cc".
    pub voter_type: String,
    /// Voter credential hash (hex).
    pub voter_hash: String,
    /// Vote: "yes", "no", "abstain".
    pub vote: String,
}

/// Signal for GOV rule: either a proposal or a vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GovSignal {
    /// Submit a governance proposal.
    #[serde(rename = "proposal")]
    Proposal {
        action_index: u32,
        deposit: u64,
        return_addr: String,
        action: TestGovAction,
    },
    /// Cast a vote on an existing proposal.
    #[serde(rename = "vote")]
    Vote {
        /// Action ID: "tx_hash#index".
        action_id: String,
        voter: TestVoter,
        vote: String,
    },
}

/// Governance action for test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestGovAction {
    #[serde(rename = "info_action")]
    InfoAction,
    #[serde(rename = "treasury_withdrawals")]
    TreasuryWithdrawals { withdrawals: BTreeMap<String, u64> },
    #[serde(rename = "no_confidence")]
    NoConfidence { prev_action_id: Option<String> },
    #[serde(rename = "hard_fork_initiation")]
    HardForkInitiation {
        prev_action_id: Option<String>,
        protocol_version: [u64; 2],
    },
    #[serde(rename = "new_constitution")]
    NewConstitution {
        prev_action_id: Option<String>,
        anchor_url: String,
        anchor_hash: String,
        script_hash: Option<String>,
    },
}

/// Voter for test vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TestVoter {
    #[serde(rename = "drep")]
    DRep { hash: String },
    #[serde(rename = "spo")]
    StakePool { hash: String },
    #[serde(rename = "cc")]
    ConstitutionalCommittee { hash: String },
}

// ---------------------------------------------------------------------------
// EPOCH rule types
// ---------------------------------------------------------------------------

/// Environment for the EPOCH rule.
///
/// Maps to a simplified epoch transition environment. The EPOCH rule is not
/// directly an Agda STS rule, but rather models the aggregate behavior of
/// epoch boundary processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochEnvironment {
    /// Current epoch (the epoch BEFORE the transition).
    pub current_epoch: u64,
    /// Protocol parameters affecting epoch transition behavior.
    pub protocol_params: EpochProtocolParams,
}

/// Protocol parameters relevant to epoch transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochProtocolParams {
    /// Pool deposit (refunded on retirement).
    pub pool_deposit: u64,
    /// Governance action lifetime in epochs.
    pub gov_action_lifetime: u64,
    /// DRep activity period (epochs).
    pub drep_activity: u64,
    /// Key deposit (for stake registration).
    pub key_deposit: u64,
    /// Governance action deposit.
    pub gov_action_deposit: u64,
    /// Pool retirement maximum epoch offset.
    pub e_max: u64,
}

/// Simplified ledger state for EPOCH rule testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochState {
    /// Active governance proposals: [{tx_hash, action_index, action_type, expires_epoch}]
    #[serde(default)]
    pub proposals: Vec<EpochProposal>,
    /// Pending pool retirements: [{pool_hash, retirement_epoch}]
    #[serde(default)]
    pub pending_retirements: Vec<EpochRetirement>,
    /// Registered DReps: [{credential_hash, last_active_epoch, active}]
    #[serde(default)]
    pub dreps: Vec<EpochDRep>,
    /// Reward accounts: [{credential_hash, balance}]
    #[serde(default)]
    pub reward_accounts: Vec<EpochRewardAccount>,
    /// Registered pools (simplified): [{pool_hash, reward_account}]
    #[serde(default)]
    pub pools: Vec<EpochPool>,
}

/// A governance proposal in the epoch state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochProposal {
    pub tx_hash: String,
    pub action_index: u32,
    pub action_type: String,
    pub expires_epoch: u64,
    /// Deposit to be refunded on expiry/ratification.
    pub deposit: u64,
    /// Return address (hex).
    pub return_addr: String,
}

/// A pending pool retirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRetirement {
    pub pool_hash: String,
    pub retirement_epoch: u64,
}

/// A registered DRep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochDRep {
    pub credential_hash: String,
    pub last_active_epoch: u64,
    pub active: bool,
}

/// A reward account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRewardAccount {
    pub credential_hash: String,
    pub balance: u64,
}

/// A registered pool (simplified).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochPool {
    pub pool_hash: String,
    pub reward_account: String,
}

/// Signal for the EPOCH rule: the new epoch number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochSignal {
    pub new_epoch: u64,
}

/// Expected output state for EPOCH rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochExpectedState {
    /// Remaining active proposals after ratification/expiry.
    #[serde(default)]
    pub proposals: Vec<EpochProposal>,
    /// Remaining pending retirements after processing.
    #[serde(default)]
    pub pending_retirements: Vec<EpochRetirement>,
    /// DRep states after activity check.
    #[serde(default)]
    pub dreps: Vec<EpochDRep>,
    /// Reward accounts after deposit refunds.
    #[serde(default)]
    pub reward_accounts: Vec<EpochRewardAccount>,
    /// Remaining registered pools after retirements.
    #[serde(default)]
    pub pools: Vec<EpochPool>,
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
