use crate::address::Address;
use crate::credentials::Credential;
use crate::hash::{
    AuxiliaryDataHash, DatumHash, Hash28, Hash32, PolicyId, ScriptHash, TransactionHash,
};
use crate::time::SlotNo;
use crate::value::{AssetName, Lovelace, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A reference to a specific output from a previous transaction
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TransactionInput {
    pub transaction_id: TransactionHash,
    pub index: u32,
}

impl std::fmt::Display for TransactionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.transaction_id, self.index)
    }
}

/// Transaction output (Babbage/Conway era - post-Alonzo)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionOutput {
    pub address: Address,
    pub value: Value,
    pub datum: OutputDatum,
    pub script_ref: Option<ScriptRef>,
    /// Raw CBOR encoding of this output (for Plutus script evaluation)
    #[serde(skip)]
    pub raw_cbor: Option<Vec<u8>>,
}

/// How datum is attached to a UTxO
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputDatum {
    None,
    DatumHash(DatumHash),
    InlineDatum(PlutusData),
}

/// Reference to a script embedded in a UTxO
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptRef {
    NativeScript(NativeScript),
    PlutusV1(Vec<u8>),
    PlutusV2(Vec<u8>),
    PlutusV3(Vec<u8>),
}

/// Native script (multi-sig and time-lock)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeScript {
    ScriptPubkey(Hash32),
    ScriptAll(Vec<NativeScript>),
    ScriptAny(Vec<NativeScript>),
    ScriptNOfK(u32, Vec<NativeScript>),
    InvalidBefore(SlotNo),
    InvalidHereafter(SlotNo),
}

/// Plutus data (arbitrary structured data for smart contracts)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlutusData {
    Constr(u64, Vec<PlutusData>),
    Map(Vec<(PlutusData, PlutusData)>),
    List(Vec<PlutusData>),
    Integer(i128),
    Bytes(Vec<u8>),
}

/// Redeemer purpose
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RedeemerTag {
    Spend,
    Mint,
    Cert,
    Reward,
    Vote,
    Propose,
}

/// Redeemer for Plutus script execution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redeemer {
    pub tag: RedeemerTag,
    pub index: u32,
    pub data: PlutusData,
    pub ex_units: ExUnits,
}

/// Execution units for Plutus script execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExUnits {
    pub mem: u64,
    pub steps: u64,
}

/// Certificate for staking operations and governance
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Certificate {
    StakeRegistration(Credential),
    StakeDeregistration(Credential),
    /// Conway-era stake registration with explicit deposit (cert tag 7)
    ConwayStakeRegistration {
        credential: Credential,
        deposit: Lovelace,
    },
    /// Conway-era stake deregistration with explicit refund (cert tag 8)
    ConwayStakeDeregistration {
        credential: Credential,
        refund: Lovelace,
    },
    StakeDelegation {
        credential: Credential,
        pool_hash: Hash28,
    },
    PoolRegistration(PoolParams),
    PoolRetirement {
        pool_hash: Hash28,
        epoch: u64,
    },
    // Conway-era governance certificates
    RegDRep {
        credential: Credential,
        deposit: Lovelace,
        anchor: Option<Anchor>,
    },
    UnregDRep {
        credential: Credential,
        refund: Lovelace,
    },
    UpdateDRep {
        credential: Credential,
        anchor: Option<Anchor>,
    },
    VoteDelegation {
        credential: Credential,
        drep: DRep,
    },
    StakeVoteDelegation {
        credential: Credential,
        pool_hash: Hash28,
        drep: DRep,
    },
    RegStakeDeleg {
        credential: Credential,
        pool_hash: Hash28,
        deposit: Lovelace,
    },
    CommitteeHotAuth {
        cold_credential: Credential,
        hot_credential: Credential,
    },
    CommitteeColdResign {
        cold_credential: Credential,
        anchor: Option<Anchor>,
    },
    /// Combined: register stake + delegate to pool + delegate vote (CIP-1694)
    RegStakeVoteDeleg {
        credential: Credential,
        pool_hash: Hash28,
        drep: DRep,
        deposit: Lovelace,
    },
    /// Combined: register stake + delegate vote (CIP-1694)
    VoteRegDeleg {
        credential: Credential,
        drep: DRep,
        deposit: Lovelace,
    },
    // Pre-Conway legacy certificates
    /// Genesis key delegation (Shelley-era, cert tag 5)
    GenesisKeyDelegation {
        genesis_hash: Hash32,
        genesis_delegate_hash: Hash32,
        vrf_keyhash: Hash32,
    },
    /// Move instantaneous rewards (Shelley-era, cert tag 6)
    MoveInstantaneousRewards {
        /// Source: 0 = Reserves, 1 = Treasury
        source: MIRSource,
        /// Target: either per-credential amounts or transfer to other pot
        target: MIRTarget,
    },
}

/// MIR source pot
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MIRSource {
    Reserves,
    Treasury,
}

/// MIR target
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MIRTarget {
    /// Distribute to specific stake credentials with amounts
    StakeCredentials(Vec<(Credential, i64)>),
    /// Transfer a coin amount to the other accounting pot
    OtherAccountingPot(u64),
}

/// Pre-Conway protocol parameter update proposal (Shelley-Babbage)
/// Contains proposed updates keyed by genesis delegate key hash, targeting a specific epoch
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProposal {
    /// Proposed updates: genesis_delegate_hash -> proposed_params
    pub proposed_updates: Vec<(Hash32, ProtocolParamUpdate)>,
    /// Target epoch for the update
    pub epoch: u64,
}

/// Delegated Representative
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DRep {
    KeyHash(Hash32),
    ScriptHash(ScriptHash),
    Abstain,
    NoConfidence,
}

/// URL + hash for off-chain metadata
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub url: String,
    pub data_hash: Hash32,
}

/// Stake pool parameters
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolParams {
    pub operator: Hash28,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin: Rational,
    pub reward_account: Vec<u8>,
    pub pool_owners: Vec<Hash28>,
    pub relays: Vec<Relay>,
    pub pool_metadata: Option<PoolMetadata>,
}

/// Rational number (for margin)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub numerator: u64,
    pub denominator: u64,
}

impl Rational {
    /// Convert to f64 (for display/logging only — use exact comparison methods for decisions)
    pub fn as_f64(&self) -> f64 {
        if self.denominator == 0 {
            return 0.0;
        }
        self.numerator as f64 / self.denominator as f64
    }

    /// Check if `value_num / value_den >= self` using exact integer cross-multiplication.
    /// Returns false if either denominator is zero.
    ///
    /// Matches Haskell's exact rational comparison for governance thresholds.
    pub fn is_met_by(&self, value_num: u64, value_den: u64) -> bool {
        if self.denominator == 0 || value_den == 0 {
            return false;
        }
        // value_num / value_den >= numerator / denominator
        // ⟺ value_num * denominator >= numerator * value_den
        // Use u128 to avoid overflow
        (value_num as u128) * (self.denominator as u128)
            >= (self.numerator as u128) * (value_den as u128)
    }

    /// Check if this rational is zero.
    pub fn is_zero(&self) -> bool {
        self.numerator == 0
    }

    /// Compare two Rationals: self >= other (exact).
    pub fn ge(&self, other: &Rational) -> bool {
        if self.denominator == 0 || other.denominator == 0 {
            return false;
        }
        (self.numerator as u128) * (other.denominator as u128)
            >= (other.numerator as u128) * (self.denominator as u128)
    }

    /// Compare two Rationals: self > other (exact).
    pub fn gt(&self, other: &Rational) -> bool {
        if self.denominator == 0 || other.denominator == 0 {
            return false;
        }
        (self.numerator as u128) * (other.denominator as u128)
            > (other.numerator as u128) * (self.denominator as u128)
    }
}

/// Pool relay
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Relay {
    SingleHostAddr {
        port: Option<u16>,
        ipv4: Option<[u8; 4]>,
        ipv6: Option<[u8; 16]>,
    },
    SingleHostName {
        port: Option<u16>,
        dns_name: String,
    },
    MultiHostName {
        dns_name: String,
    },
}

/// Pool metadata reference
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolMetadata {
    pub url: String,
    pub hash: Hash32,
}

/// Withdrawal from a reward account
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawal {
    pub reward_account: Vec<u8>,
    pub amount: Lovelace,
}

/// Conway governance action
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovAction {
    ParameterChange {
        prev_action_id: Option<GovActionId>,
        protocol_param_update: Box<ProtocolParamUpdate>,
        policy_hash: Option<ScriptHash>,
    },
    HardForkInitiation {
        prev_action_id: Option<GovActionId>,
        protocol_version: (u64, u64),
    },
    TreasuryWithdrawals {
        withdrawals: BTreeMap<Vec<u8>, Lovelace>,
        policy_hash: Option<ScriptHash>,
    },
    NoConfidence {
        prev_action_id: Option<GovActionId>,
    },
    UpdateCommittee {
        prev_action_id: Option<GovActionId>,
        members_to_remove: Vec<Credential>,
        members_to_add: BTreeMap<Credential, u64>,
        threshold: Rational,
    },
    NewConstitution {
        prev_action_id: Option<GovActionId>,
        constitution: Constitution,
    },
    InfoAction,
}

/// Governance action identifier
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GovActionId {
    pub transaction_id: TransactionHash,
    pub action_index: u32,
}

/// Constitution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constitution {
    pub anchor: Anchor,
    pub script_hash: Option<ScriptHash>,
}

/// Governance proposal
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalProcedure {
    pub deposit: Lovelace,
    pub return_addr: Vec<u8>,
    pub gov_action: GovAction,
    pub anchor: Anchor,
}

/// Voter
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Voter {
    ConstitutionalCommittee(Credential),
    DRep(Credential),
    StakePool(Hash32),
}

/// Vote
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vote {
    No,
    Yes,
    Abstain,
}

/// Voting procedure
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VotingProcedure {
    pub vote: Vote,
    pub anchor: Option<Anchor>,
}

/// Protocol parameter update
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProtocolParamUpdate {
    pub min_fee_a: Option<u64>,
    pub min_fee_b: Option<u64>,
    pub max_block_body_size: Option<u64>,
    pub max_tx_size: Option<u64>,
    pub max_block_header_size: Option<u64>,
    pub key_deposit: Option<Lovelace>,
    pub pool_deposit: Option<Lovelace>,
    pub e_max: Option<u64>,
    pub n_opt: Option<u64>,
    pub a0: Option<Rational>,
    pub rho: Option<Rational>,
    pub tau: Option<Rational>,
    pub min_pool_cost: Option<Lovelace>,
    pub ada_per_utxo_byte: Option<Lovelace>,
    pub cost_models: Option<CostModels>,
    pub execution_costs: Option<ExUnitPrices>,
    pub max_tx_ex_units: Option<ExUnits>,
    pub max_block_ex_units: Option<ExUnits>,
    pub max_val_size: Option<u64>,
    pub collateral_percentage: Option<u64>,
    pub max_collateral_inputs: Option<u64>,
    pub min_fee_ref_script_cost_per_byte: Option<u64>,
    // Protocol version (used in pre-Conway hard fork proposals)
    pub protocol_version_major: Option<u64>,
    pub protocol_version_minor: Option<u64>,
    // Conway governance parameters
    pub drep_deposit: Option<Lovelace>,
    pub gov_action_deposit: Option<Lovelace>,
    pub gov_action_lifetime: Option<u64>,
    // Governance voting thresholds
    pub dvt_pp_network_group: Option<Rational>,
    pub dvt_pp_economic_group: Option<Rational>,
    pub dvt_pp_technical_group: Option<Rational>,
    pub dvt_pp_gov_group: Option<Rational>,
    pub dvt_hard_fork: Option<Rational>,
    pub dvt_no_confidence: Option<Rational>,
    pub dvt_committee_normal: Option<Rational>,
    pub dvt_committee_no_confidence: Option<Rational>,
    pub dvt_constitution: Option<Rational>,
    pub dvt_treasury_withdrawal: Option<Rational>,
    pub pvt_motion_no_confidence: Option<Rational>,
    pub pvt_committee_normal: Option<Rational>,
    pub pvt_committee_no_confidence: Option<Rational>,
    pub pvt_hard_fork: Option<Rational>,
    pub pvt_pp_security_group: Option<Rational>,
    pub min_committee_size: Option<u64>,
    pub committee_term_limit: Option<u64>,
    pub drep_activity: Option<u64>,
}

/// Plutus cost models
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostModels {
    pub plutus_v1: Option<Vec<i64>>,
    pub plutus_v2: Option<Vec<i64>>,
    pub plutus_v3: Option<Vec<i64>>,
}

impl CostModels {
    /// Encode cost models to CBOR bytes for the uplc evaluator.
    ///
    /// The encoding is a CBOR map: {0: [v1_costs...], 1: [v2_costs...], 2: [v3_costs...]}
    pub fn to_cbor(&self) -> Option<Vec<u8>> {
        let has_any =
            self.plutus_v1.is_some() || self.plutus_v2.is_some() || self.plutus_v3.is_some();
        if !has_any {
            return None;
        }

        use minicbor::Encoder;
        let mut buf = Vec::with_capacity(4096);
        let mut enc = Encoder::new(&mut buf);

        let count = [&self.plutus_v1, &self.plutus_v2, &self.plutus_v3]
            .iter()
            .filter(|m| m.is_some())
            .count();
        enc.map(count as u64).unwrap();

        if let Some(v1) = &self.plutus_v1 {
            enc.u32(0).unwrap();
            enc.array(v1.len() as u64).unwrap();
            for cost in v1 {
                enc.i64(*cost).unwrap();
            }
        }
        if let Some(v2) = &self.plutus_v2 {
            enc.u32(1).unwrap();
            enc.array(v2.len() as u64).unwrap();
            for cost in v2 {
                enc.i64(*cost).unwrap();
            }
        }
        if let Some(v3) = &self.plutus_v3 {
            enc.u32(2).unwrap();
            enc.array(v3.len() as u64).unwrap();
            for cost in v3 {
                enc.i64(*cost).unwrap();
            }
        }

        Some(buf)
    }
}

/// Execution unit prices
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExUnitPrices {
    pub mem_price: Rational,
    pub step_price: Rational,
}

/// A complete Cardano transaction (Babbage/Conway era)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// The blake2b-256 hash of the serialized transaction body
    pub hash: crate::hash::TransactionHash,
    pub body: TransactionBody,
    pub witness_set: TransactionWitnessSet,
    pub is_valid: bool,
    pub auxiliary_data: Option<AuxiliaryData>,
    /// Raw CBOR encoding of this transaction (for Plutus script evaluation)
    #[serde(skip)]
    pub raw_cbor: Option<Vec<u8>>,
}

impl Transaction {
    /// Create a minimal transaction with only a hash set, used for mempool tracking
    pub fn empty_with_hash(hash: crate::hash::TransactionHash) -> Self {
        Transaction {
            hash,
            body: TransactionBody {
                inputs: vec![],
                outputs: vec![],
                fee: crate::value::Lovelace(0),
                ttl: None,
                certificates: vec![],
                withdrawals: std::collections::BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: std::collections::BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: std::collections::BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        }
    }
}

/// Transaction body
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionBody {
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<TransactionOutput>,
    pub fee: Lovelace,
    pub ttl: Option<SlotNo>,
    pub certificates: Vec<Certificate>,
    pub withdrawals: BTreeMap<Vec<u8>, Lovelace>,
    pub auxiliary_data_hash: Option<AuxiliaryDataHash>,
    pub validity_interval_start: Option<SlotNo>,
    pub mint: BTreeMap<PolicyId, BTreeMap<AssetName, i64>>,
    pub script_data_hash: Option<Hash32>,
    pub collateral: Vec<TransactionInput>,
    pub required_signers: Vec<Hash32>,
    pub network_id: Option<u8>,
    pub collateral_return: Option<TransactionOutput>,
    pub total_collateral: Option<Lovelace>,
    pub reference_inputs: Vec<TransactionInput>,
    // Pre-Conway: update proposals (field 6 in CDDL)
    pub update: Option<UpdateProposal>,
    // Conway governance
    pub voting_procedures: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>>,
    pub proposal_procedures: Vec<ProposalProcedure>,
    pub treasury_value: Option<Lovelace>,
    pub donation: Option<Lovelace>,
}

/// Transaction witness set
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionWitnessSet {
    pub vkey_witnesses: Vec<VKeyWitness>,
    pub native_scripts: Vec<NativeScript>,
    pub bootstrap_witnesses: Vec<BootstrapWitness>,
    pub plutus_v1_scripts: Vec<Vec<u8>>,
    pub plutus_v2_scripts: Vec<Vec<u8>>,
    pub plutus_v3_scripts: Vec<Vec<u8>>,
    pub plutus_data: Vec<PlutusData>,
    pub redeemers: Vec<Redeemer>,
}

/// Verification key witness (signature)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VKeyWitness {
    pub vkey: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Bootstrap witness (Byron)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapWitness {
    pub vkey: Vec<u8>,
    pub signature: Vec<u8>,
    pub chain_code: Vec<u8>,
    pub attributes: Vec<u8>,
}

/// Auxiliary data (metadata + scripts)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliaryData {
    pub metadata: BTreeMap<u64, TransactionMetadatum>,
    pub native_scripts: Vec<NativeScript>,
    pub plutus_v1_scripts: Vec<Vec<u8>>,
    pub plutus_v2_scripts: Vec<Vec<u8>>,
    pub plutus_v3_scripts: Vec<Vec<u8>>,
}

/// Transaction metadata value
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionMetadatum {
    Map(Vec<(TransactionMetadatum, TransactionMetadatum)>),
    List(Vec<TransactionMetadatum>),
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
}
