//! Transaction validation — Phase-1 and Phase-2.
//!
//! This module is the public surface of the validation subsystem. It:
//! - Defines [`ValidationError`], the unified error type for all validation rules.
//! - Provides [`validate_transaction`] and [`validate_transaction_with_pools`] as
//!   the sole public entry points.
//! - Re-exports [`evaluate_native_script`] for callers that need to evaluate
//!   native scripts outside of full transaction validation (e.g. mempool admission).
//!
//! Internal rule logic is split across focused sub-modules:
//! - [`phase1`]    — Rules 1–10, 13–14 (structural/witness rules)
//! - [`collateral`] — Rules 11, 11b, 11c (collateral for Plutus transactions)
//! - [`scripts`]   — Rule 12 + script hash utilities + native script evaluation
//! - [`conway`]    — Era-gating checks + deposit/refund accounting

mod collateral;
mod conway;
mod datum;
mod phase1;
mod scripts;

#[cfg(test)]
mod tests;

pub use scripts::evaluate_native_script;
// Re-exported for use by the block-application layer (block-level ref script
// size check in state/apply.rs — Haskell's `conwayBbodyTransition`).
pub(crate) use scripts::script_ref_byte_size;
// Re-export the tier cap so apply.rs can reuse the same constant for the
// block-body check, keeping the tiered-fee short-circuit in sync.
pub(crate) use scripts::MAX_REF_SCRIPT_SIZE_TIER_CAP;
// Re-exported for use by the block-application layer (per-transaction 200 KiB
// ref script size check — Haskell's `ppMaxRefScriptSizePerTxG` enforcement).
pub(crate) use scripts::calculate_ref_script_size;
// Re-exported for use by plutus.rs (V3 non-Unit return value check): maps
// script hashes to their language version so the evaluator can apply the
// correct success predicate per-result.
pub(crate) use collateral::plutus_script_version_map;
// Re-exported for use by plutus.rs (per-redeemer V3 Unit-return check): maps
// (redeemer_tag_byte, index) to the language version of the script that
// redeemer executes, allowing the Unit check to be applied only to V3 redeemers.
pub(crate) use collateral::redeemer_script_version_map;

use std::collections::{HashMap, HashSet};

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::network::NetworkId;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::transaction::{GovAction, Transaction};
use dugite_primitives::value::Lovelace;
use tracing::{debug, trace, warn};

use crate::plutus::{evaluate_plutus_scripts, SlotConfig};
use crate::utxo::UtxoLookup;

#[derive(Default)]
pub struct ValidationContext {
    pub registered_pools: Option<HashSet<Hash28>>,
    pub current_treasury: Option<u64>,
    pub reward_accounts: Option<HashMap<Hash32, Lovelace>>,
    pub current_epoch: Option<u64>,
    pub registered_dreps: Option<HashSet<Hash32>>,
    pub registered_vrf_keys: Option<HashMap<Hash32, Hash28>>,
    pub node_network: Option<NetworkId>,
    pub committee_members: Option<HashSet<Hash32>>,
    pub committee_resigned: Option<HashSet<Hash32>>,
    pub stake_key_deposits: Option<HashMap<Hash32, u64>>,
    /// The constitution's guardrail script hash, if any.
    ///
    /// When `Some`, governance proposals of type `ParameterChange` or
    /// `TreasuryWithdrawals` must carry a matching `policy_hash`.  When `None`,
    /// the constitution policy-hash check is skipped.
    pub constitution_script_hash: Option<Hash28>,
    /// DRep vote delegations — keys are stake credential hashes of accounts
    /// that have delegated to any DRep (including AlwaysAbstain / AlwaysNoConfidence).
    pub vote_delegations: Option<HashSet<Hash32>>,
}

impl ValidationContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pools(mut self, pools: HashSet<Hash28>) -> Self {
        self.registered_pools = Some(pools);
        self
    }

    pub fn with_treasury(mut self, treasury: u64) -> Self {
        self.current_treasury = Some(treasury);
        self
    }

    pub fn with_reward_accounts(mut self, accounts: HashMap<Hash32, Lovelace>) -> Self {
        self.reward_accounts = Some(accounts);
        self
    }

    pub fn with_epoch(mut self, epoch: u64) -> Self {
        self.current_epoch = Some(epoch);
        self
    }

    pub fn with_dreps(mut self, dreps: HashSet<Hash32>) -> Self {
        self.registered_dreps = Some(dreps);
        self
    }

    pub fn with_vrf_keys(mut self, keys: HashMap<Hash32, Hash28>) -> Self {
        self.registered_vrf_keys = Some(keys);
        self
    }

    pub fn with_network(mut self, network: NetworkId) -> Self {
        self.node_network = Some(network);
        self
    }

    pub fn with_committee_members(mut self, members: HashSet<Hash32>) -> Self {
        self.committee_members = Some(members);
        self
    }

    pub fn with_committee_resigned(mut self, resigned: HashSet<Hash32>) -> Self {
        self.committee_resigned = Some(resigned);
        self
    }

    pub fn with_stake_key_deposits(mut self, deposits: HashMap<Hash32, u64>) -> Self {
        self.stake_key_deposits = Some(deposits);
        self
    }

    pub fn with_constitution_script_hash(mut self, hash: Hash28) -> Self {
        self.constitution_script_hash = Some(hash);
        self
    }

    pub fn with_vote_delegations(mut self, delegations: HashSet<Hash32>) -> Self {
        self.vote_delegations = Some(delegations);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_full_ledger_state(
        mut self,
        pools: HashSet<Hash28>,
        treasury: u64,
        accounts: HashMap<Hash32, Lovelace>,
        epoch: u64,
        dreps: HashSet<Hash32>,
        vrf_keys: HashMap<Hash32, Hash28>,
        network: NetworkId,
        committee_members: HashSet<Hash32>,
        committee_resigned: HashSet<Hash32>,
    ) -> Self {
        self.registered_pools = Some(pools);
        self.current_treasury = Some(treasury);
        self.reward_accounts = Some(accounts);
        self.current_epoch = Some(epoch);
        self.registered_dreps = Some(dreps);
        self.registered_vrf_keys = Some(vrf_keys);
        self.node_network = Some(network);
        self.committee_members = Some(committee_members);
        self.committee_resigned = Some(committee_resigned);
        self
    }
}

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("No inputs in transaction")]
    NoInputs,
    #[error("Input not found in UTxO set: {0}")]
    InputNotFound(String),
    #[error("Value not conserved: inputs={inputs}, outputs={outputs}, fee={fee}")]
    ValueNotConserved { inputs: u64, outputs: u64, fee: u64 },
    #[error("Fee too small: minimum={minimum}, actual={actual}")]
    FeeTooSmall { minimum: u64, actual: u64 },
    #[error("Output too small: minimum={minimum}, actual={actual}")]
    OutputTooSmall { minimum: u64, actual: u64 },
    #[error("Transaction too large: maximum={maximum}, actual={actual}")]
    TxTooLarge { maximum: u64, actual: u64 },
    #[error("Missing required signer: {0}")]
    MissingRequiredSigner(String),
    #[error("Missing witness for input: {0}")]
    MissingWitness(String),
    #[error("TTL expired: current_slot={current_slot}, ttl={ttl}")]
    TtlExpired { current_slot: u64, ttl: u64 },
    #[error("Transaction not yet valid: current_slot={current_slot}, valid_from={valid_from}")]
    NotYetValid { current_slot: u64, valid_from: u64 },
    #[error("Script validation failed: {0}")]
    ScriptFailed(String),
    #[error("Insufficient collateral")]
    InsufficientCollateral,
    #[error("Too many collateral inputs: max={max}, actual={actual}")]
    TooManyCollateralInputs { max: u64, actual: u64 },
    #[error("Collateral input not found in UTxO set: {0}")]
    CollateralNotFound(String),
    #[error("Collateral input contains tokens (must be pure ADA): {0}")]
    CollateralHasTokens(String),
    #[error("Collateral mismatch: total_collateral={declared}, effective={computed}")]
    CollateralMismatch { declared: u64, computed: u64 },
    #[error("Reference input not found in UTxO set: {0}")]
    ReferenceInputNotFound(String),
    #[error("Reference input overlaps with regular input: {0}")]
    ReferenceInputOverlapsInput(String),
    #[error("Multi-asset not conserved for policy {policy}: inputs+mint={input_side}, outputs={output_side}")]
    MultiAssetNotConserved {
        policy: String,
        input_side: i128,
        output_side: i128,
    },
    #[error("Negative minting without policy script")]
    InvalidMint,
    #[error("Max execution units exceeded")]
    ExUnitsExceeded,
    #[error("Script data hash mismatch: expected {expected}, got {actual}")]
    ScriptDataHashMismatch { expected: String, actual: String },
    #[error("Script data hash present but no scripts or redeemers")]
    UnexpectedScriptDataHash,
    #[error("Missing script data hash (required when scripts/redeemers present)")]
    MissingScriptDataHash,
    #[error("Duplicate input in transaction: {0}")]
    DuplicateInput(String),
    #[error("Native script validation failed")]
    NativeScriptFailed,
    #[error("Witness signature verification failed for vkey: {0}")]
    InvalidWitnessSignature(String),
    #[error("Output address network mismatch: expected {expected:?}, got {actual:?}")]
    NetworkMismatch {
        expected: dugite_primitives::network::NetworkId,
        actual: dugite_primitives::network::NetworkId,
    },
    #[error("Auxiliary data hash declared but no auxiliary data present")]
    AuxiliaryDataHashWithoutData,
    #[error("Auxiliary data present but no auxiliary data hash in tx body")]
    AuxiliaryDataWithoutHash,
    #[error("Block execution units exceeded: {resource} limit={limit}, total={total}")]
    BlockExUnitsExceeded {
        resource: String,
        limit: u64,
        total: u64,
    },
    #[error("Output value too large: maximum={maximum}, actual={actual}")]
    OutputValueTooLarge { maximum: u64, actual: u64 },
    #[error("Plutus transaction missing raw CBOR for script evaluation")]
    MissingRawCbor,
    #[error("Plutus transaction missing slot configuration for script evaluation")]
    MissingSlotConfig,
    #[error("Script-locked input at index {index} has no matching Spend redeemer")]
    MissingSpendRedeemer { index: u32 },
    /// A script-locked withdrawal or Plutus minting policy has no matching
    /// redeemer of the required tag/index.
    ///
    /// Mirrors Haskell's `scriptsNeeded` check: every entry in the `Reward`
    /// and `Mint` buckets that corresponds to a Plutus script must have an
    /// explicit redeemer at the correct sorted position.
    #[error("Missing {tag} redeemer at index {index}")]
    MissingRedeemer { tag: String, index: u32 },
    #[error("Redeemer index out of range: tag={tag}, index={index}, max={max}")]
    RedeemerIndexOutOfRange { tag: String, index: u32, max: usize },
    #[error("Missing VKey witness for input credential: {0}")]
    MissingInputWitness(String),
    #[error("Missing script witness for script-locked input: {0}")]
    MissingScriptWitness(String),
    #[error("Missing VKey witness for withdrawal credential: {0}")]
    MissingWithdrawalWitness(String),
    #[error("Missing script witness for script-locked withdrawal: {0}")]
    MissingWithdrawalScriptWitness(String),
    #[error("Missing VKey witness for certificate credential: {0}")]
    MissingCertificateWitness(String),
    #[error("Value overflow in transaction accounting")]
    ValueOverflow,
    #[error("Era gating violation: {certificate_type} requires {required_era}, current era is {current_era}")]
    EraGatingViolation {
        certificate_type: String,
        required_era: String,
        current_era: String,
    },
    #[error("Governance feature requires Conway era (protocol >= 9), current protocol version is {current_version}")]
    GovernancePreConway { current_version: u64 },
    /// Conway LEDGERS rule: the block producer's declared treasury value in the
    /// transaction body (`currentTreasuryValue`, field 19) must match the
    /// ledger's tracked treasury balance exactly.
    ///
    /// Reference: Cardano Blueprint `LEDGERS` flowchart, "submittedTreasuryValue
    /// == currentTreasuryValue" predicate.
    #[error("Treasury value mismatch: tx declared {declared}, ledger has {actual}")]
    TreasuryValueMismatch { declared: u64, actual: u64 },
    /// Conway LEDGERS rule: the `CommitteeHotAuth` certificate's cold credential
    /// must correspond to a member currently elected to the constitutional
    /// committee (`committee_expiration` map).  Authorising a hot key for an
    /// unrecognised cold credential is rejected ("failOnNonEmpty unelected").
    ///
    /// Reference: Cardano ledger `conwayWitsVKeyNeeded` / `CERT` rule,
    /// "ccHotKeyOK" predicate from the Haskell implementation.
    #[error("CommitteeHotAuth cold credential is not a current CC member: {cold_credential_hash}")]
    UnelectedCommitteeMember { cold_credential_hash: String },
    /// Conway LEDGERS rule: the `CommitteeHotAuth` certificate's cold credential
    /// belongs to a committee member that has previously resigned via
    /// `CommitteeColdResign`.  Resigned members may not re-authorise hot keys
    /// until they are re-elected (the Haskell `CERT` rule predicate
    /// "membersResigned ∩ {coldKey} = ∅").
    ///
    /// Reference: Haskell `ConwayCommitteeHasPreviouslyResigned` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Cert`.
    #[error(
        "CommitteeHotAuth rejected: cold credential {cold_credential_hash} has previously \
         resigned (ConwayCommitteeHasPreviouslyResigned)"
    )]
    CommitteeHasPreviouslyResigned { cold_credential_hash: String },
    /// Alonzo/Conway Phase-1 rule: a script-locked spending input carries a
    /// `DatumHash` in its UTxO but no corresponding datum bytes were supplied
    /// in `tx.witness_set.plutus_data`.
    ///
    /// Per Haskell's `checkWitnessesShelley` / Alonzo `UTXOW` rule
    /// "witsVKeyNeeded" extended with "reqSignerHashes" — every non-inline
    /// datum referenced by a script-locked input MUST be provided as a witness.
    #[error("Missing datum witness for script-locked input: datum hash {0}")]
    MissingDatumWitness(String),
    /// Alonzo/Conway Phase-1 rule: a datum supplied in
    /// `tx.witness_set.plutus_data` is not needed by any script-locked input
    /// or referenced output, making the transaction malformed.
    ///
    /// Haskell rejects transactions with extraneous datums under the
    /// `UTXOW` predicate "allowedSupplementalDatums ⊇ suppliedDatums".
    #[error("Extra (unreferenced) datum witness in transaction: datum hash {0}")]
    ExtraDatumWitness(String),
    /// Alonzo UTXO rule: a script-locked spending input has no datum
    /// (OutputDatum::None) and the locking script is PlutusV1 or PlutusV2.
    /// PlutusV3 inputs are exempt per CIP-0069.
    ///
    /// Reference: Haskell `UnspendableUTxONoDatumHash` in
    /// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
    #[error(
        "Script-locked input {input} has no datum (NoDatum) and locking script is {language} \
         (UnspendableUTxONoDatumHash — PlutusV3 exempt per CIP-0069)"
    )]
    UnspendableUTxONoDatumHash { input: String, language: String },
    /// Conway LEDGER rule (PV ≥ 10): a KeyHash reward account making a
    /// withdrawal must have an active DRep delegation (any delegation value
    /// including AlwaysAbstain/AlwaysNoConfidence satisfies this).
    ///
    /// Reference: Haskell `ConwayWdrlNotDelegatedToDRep` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Ledger`.
    #[error(
        "Withdrawal rejected: KeyHash reward account {credential_hash} has no DRep delegation \
         (ConwayWdrlNotDelegatedToDRep, requires PV >= 10)"
    )]
    WdrlNotDelegatedToDRep { credential_hash: String },
    /// Conway GOV rule: a `ParameterChange` proposal's `PParamsUpdate` is
    /// malformed — one or more fields fail the `ppuWellFormed` check.
    ///
    /// Reference: Haskell `MalformedProposal` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Gov`.
    #[error("Governance proposal rejected: malformed PParamsUpdate ({reason})")]
    MalformedProposal { reason: String },
    /// Alonzo UTXOW rule: a redeemer in the witness set has no matching
    /// script purpose (spending input, minting policy, withdrawal, cert, vote).
    ///
    /// Reference: Haskell `ExtraRedeemers` in
    /// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxow`.
    #[error("Extra redeemer with no matching script purpose: tag={tag}, index={index}")]
    ExtraRedeemer { tag: String, index: u32 },
    /// Alonzo UTXO rule: collateral inputs must be at VKey (non-script)
    /// addresses. Script-locked UTxOs cannot serve as collateral.
    /// Byron/bootstrap addresses are accepted as collateral.
    ///
    /// Reference: Haskell `ScriptsNotPaidUTxO` in
    /// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
    #[error("Collateral input(s) at script-locked addresses (ScriptsNotPaidUTxO): {inputs:?}")]
    ScriptLockedCollateral { inputs: Vec<String> },
    /// Conway rule: the total byte size of all reference scripts reachable
    /// from a single transaction's inputs and reference inputs must not exceed
    /// 200 KiB (`ppMaxRefScriptSizePerTxG`).
    ///
    /// Source: Haskell `ppMaxRefScriptSizePerTxG = L.to . const $ 200 * 1024`
    /// (Conway PParams). This is hardcoded, not a governance-updateable parameter.
    #[error(
        "Transaction reference script size {actual} exceeds per-transaction limit \
         {limit} bytes (Conway ppMaxRefScriptSizePerTxG)"
    )]
    TxRefScriptSizeTooLarge { actual: u64, limit: u64 },
    /// Pool retirement epoch exceeds `current_epoch + e_max`.
    ///
    /// Per Haskell's POOL rule (Shelley spec, Figure 14): "The pool's announced
    /// retirement epoch must satisfy `e <= cepoch + emax`."
    #[error(
        "Pool retirement epoch {retirement_epoch} exceeds maximum allowed \
         {max_epoch} (current_epoch={current_epoch} + e_max={e_max})"
    )]
    PoolRetirementTooLate {
        retirement_epoch: u64,
        current_epoch: u64,
        e_max: u64,
        max_epoch: u64,
    },
    /// Conway `ConwayStakeRegistration` deposit does not match protocol parameter
    /// `key_deposit`.
    ///
    /// Per Haskell's Conway `DELEG` rule: "The deposit amount declared in the
    /// certificate must equal the current `keyDeposit` protocol parameter."
    #[error(
        "Conway stake registration deposit mismatch: declared={declared}, \
         expected key_deposit={expected}"
    )]
    StakeRegistrationDepositMismatch { declared: u64, expected: u64 },
    /// Haskell `wdrlNotZero`: withdrawals with a zero amount are rejected.
    #[error("Zero withdrawal amount for reward account: {account}")]
    ZeroWithdrawal { account: String },
    /// Withdrawal amount does not match the on-chain reward balance for the account.
    #[error("Incorrect withdrawal amount for {account}: declared={declared}, actual={actual}")]
    IncorrectWithdrawalAmount {
        account: String,
        declared: u64,
        actual: u64,
    },
    /// Haskell `StakeKeyHasNonZeroAccountBalanceDELEG`: a stake deregistration
    /// is rejected when the reward account holds a non-zero balance.
    ///
    /// Per the Cardano ledger spec (Shelley DELEG rule and Conway DELEG rule),
    /// deregistering a stake credential with a non-empty reward account is
    /// invalid — the delegator must first withdraw all rewards before
    /// deregistering. This prevents silent loss of on-chain rewards.
    ///
    /// Reference: Haskell `StakeKeyHasNonZeroAccountBalanceDELEG` predicate in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Deleg`.
    #[error(
        "Stake deregistration rejected: reward account {credential_hash} has non-zero balance \
         ({balance} lovelace) — withdraw rewards before deregistering"
    )]
    StakeKeyHasNonZeroBalance {
        /// Hex-encoded credential hash (zero-padded to 32 bytes).
        credential_hash: String,
        /// Current reward balance in lovelace.
        balance: u64,
    },
    /// Conway `UnRegCert` (tag 8) declared refund does not match the current
    /// `key_deposit` protocol parameter.
    ///
    /// Per Haskell's Conway DELEG rule: the deposit amount carried in
    /// `ConwayStakeDeregistration` must equal the `keyDeposit` currently in
    /// effect. A mismatch means the transaction was constructed with stale
    /// protocol parameters and must be rejected.
    #[error(
        "Conway stake deregistration refund mismatch: declared={declared}, \
         expected key_deposit={expected}"
    )]
    StakeDeregistrationRefundMismatch { declared: u64, expected: u64 },
    /// Haskell `StakeKeyRegisteredDELEG`: a stake registration certificate
    /// names a credential that is already registered in the ledger.
    ///
    /// Both legacy `StakeRegistration` (tag 0) and Conway
    /// `ConwayStakeRegistration` (tag 7) are covered — Haskell enforces the
    /// same predicate for both certificate variants.
    ///
    /// Reference: Haskell `StakeKeyRegisteredDELEG` in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Deleg`.
    #[error(
        "Stake registration rejected: credential {credential_hash} is already registered \
         (StakeKeyRegisteredDELEG)"
    )]
    StakeKeyAlreadyRegistered {
        /// Hex-encoded credential hash (zero-padded to 32 bytes).
        credential_hash: String,
    },
    /// Haskell `DelegateeStakePoolNotRegisteredDELEG`: a stake delegation
    /// certificate names a pool ID that is not currently registered.
    ///
    /// Covers all delegation certificate variants: `StakeDelegation` (tag 2),
    /// `RegStakeDeleg` (tag 11), `StakeVoteDelegation` (tag 13),
    /// `RegStakeVoteDeleg` (tag 14).
    ///
    /// Reference: Haskell `DelegateeStakePoolNotRegisteredDELEG` predicate in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Deleg`.
    #[error(
        "Stake delegation rejected: target pool {pool_id} is not registered \
         (DelegateeStakePoolNotRegisteredDELEG)"
    )]
    DelegateePoolNotRegistered {
        /// Hex-encoded pool ID (Hash28).
        pool_id: String,
    },
    /// Haskell `ConwayDRepAlreadyRegistered`: a `RegDRep` certificate names a
    /// DRep credential that is already present in the DRep registry.
    ///
    /// Reference: Haskell `ConwayDRepAlreadyRegistered` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Deleg`.
    #[error(
        "DRep registration rejected: credential {credential_hash} is already registered \
         (ConwayDRepAlreadyRegistered)"
    )]
    DRepAlreadyRegistered {
        /// Hex-encoded DRep credential hash (zero-padded to 32 bytes).
        credential_hash: String,
    },
    /// Haskell `ConwayDRepIncorrectDeposit`: a `RegDRep` certificate declares a
    /// deposit amount that does not match the current `drep_deposit` protocol
    /// parameter.
    ///
    /// Reference: Haskell `ConwayDRepIncorrectDeposit` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.GovCert`.
    #[error(
        "DRep registration rejected: declared deposit {declared} does not match \
         drep_deposit parameter {expected} (ConwayDRepIncorrectDeposit)"
    )]
    DRepIncorrectDeposit {
        /// Deposit amount declared in the `RegDRep` certificate.
        declared: u64,
        /// Expected deposit from `drep_deposit` protocol parameter.
        expected: u64,
    },
    /// Haskell `ProposalDepositIncorrect`: a governance proposal declares a
    /// deposit amount that does not match the current `gov_action_deposit`
    /// protocol parameter.
    ///
    /// Reference: Haskell `ProposalDepositIncorrect` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Gov`.
    #[error(
        "Governance proposal rejected: declared deposit {declared} does not match \
         gov_action_deposit parameter {expected} (ProposalDepositIncorrect)"
    )]
    ProposalDepositIncorrect {
        /// Deposit amount declared in the `ProposalProcedure`.
        declared: u64,
        /// Expected deposit from `gov_action_deposit` protocol parameter.
        expected: u64,
    },
    /// Conway+ POOL rule: a `PoolRegistration` certificate uses a VRF key hash
    /// that is already registered to a different pool.
    ///
    /// Enforced only when `protocol_version_major >= 9` (Conway). In earlier
    /// eras, multiple pools sharing a VRF key is theoretically possible (though
    /// inadvisable). From Conway onward, Haskell rejects duplicate VRF keys to
    /// prevent ambiguity in the VRF-based leader election.
    ///
    /// Reference: Haskell `VRFKeyHashAlreadyRegistered` in
    /// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Pool`.
    #[error(
        "Pool registration rejected: VRF key {vrf_keyhash} is already registered to pool \
         {existing_pool_id} (VRFKeyHashAlreadyRegistered)"
    )]
    VrfKeyHashAlreadyRegistered {
        /// Hex-encoded VRF key hash (32 bytes).
        vrf_keyhash: String,
        /// Hex-encoded pool ID that currently holds the VRF key.
        existing_pool_id: String,
    },
    /// Shelley+ POOL rule: pool registration cost is below the minimum pool cost
    /// (`minPoolCost` / `min_pool_cost`) from the protocol parameters.
    ///
    /// Per Haskell's POOL rule (Shelley spec, Figure 14): "The declared pool cost
    /// must satisfy `poolCost >= minPoolCost`." This prevents pools from declaring
    /// artificially low costs to attract delegators at the expense of network
    /// sustainability.
    ///
    /// Reference: Haskell `StakePoolCostTooLowPOOL` in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Pool`.
    #[error(
        "Pool registration rejected: cost {actual} is below minimum pool cost {minimum} \
         (StakePoolCostTooLowPOOL)"
    )]
    StakePoolCostTooLow {
        /// Declared pool cost in lovelace.
        actual: u64,
        /// `minPoolCost` protocol parameter in lovelace.
        minimum: u64,
    },
    /// Alonzo+ POOL rule: pool registration reward account network must match the
    /// network ID declared in the transaction body.
    ///
    /// When a transaction body carries a `network_id` field (Alonzo+), every pool
    /// registration certificate's reward account must be on the same network.
    /// Mixing networks (e.g., a testnet reward account in a mainnet transaction)
    /// is rejected as `WrongNetworkInTxBody`.
    ///
    /// Reference: Haskell `WrongNetworkInTxBody` in
    /// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
    #[error(
        "Pool registration rejected: reward account network {actual:?} does not match \
         transaction network {expected:?} (WrongNetworkInTxBody)"
    )]
    PoolRewardAccountWrongNetwork {
        expected: dugite_primitives::network::NetworkId,
        actual: dugite_primitives::network::NetworkId,
    },
    /// Auxiliary data hash content mismatch.
    ///
    /// When both `auxiliary_data_hash` and `auxiliary_data` are present in a
    /// transaction, the declared hash must equal `blake2b_256(raw_aux_data_cbor)`.
    /// This check ensures the auxiliary data has not been altered after signing.
    ///
    /// Reference: Haskell `AuxiliaryDataHash` predicate in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxow`.
    #[error(
        "Auxiliary data hash mismatch: declared hash does not match blake2b_256 of aux data bytes \
         (AuxDataHashMismatch)"
    )]
    AuxiliaryDataHashMismatch,
    /// Output address network does not match the node's configured network.
    ///
    /// Every transaction output address must be on the same network as the node.
    /// This is an unconditional check (unlike Rule 5b which only fires when the
    /// tx body carries a `network_id` field).
    ///
    /// Reference: Haskell `WrongNetwork` in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxo`.
    #[error(
        "Output address network {actual:?} does not match node network {expected:?} \
         (WrongNetworkInOutput)"
    )]
    WrongNetworkInOutput {
        expected: dugite_primitives::network::NetworkId,
        actual: dugite_primitives::network::NetworkId,
    },
    /// Withdrawal reward address network does not match the node's configured network.
    ///
    /// Every withdrawal reward address must be on the same network as the node.
    ///
    /// Reference: Haskell `WrongNetworkWithdrawal` in
    /// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxow`.
    #[error(
        "Withdrawal reward address network {actual:?} does not match node network {expected:?} \
         (WrongNetworkWithdrawal)"
    )]
    WrongNetworkWithdrawal {
        expected: dugite_primitives::network::NetworkId,
        actual: dugite_primitives::network::NetworkId,
    },
    /// Conway GOV rule: a `ParameterChange` or `TreasuryWithdrawals` proposal's
    /// `policy_hash` does not match the constitution's guardrail script hash.
    ///
    /// When the constitution carries a guardrail script, every governed proposal
    /// must include a `policy_hash` that equals the constitution's script hash.
    /// A mismatch or omission prevents the guardrail from being executed during
    /// Phase-2, bypassing the constitutionality check.
    ///
    /// Reference: Haskell `ConwayGovFailure` predicate —
    /// `GovActionsDoNotExist` / policy-hash mismatch in the GOV rule.
    #[error(
        "Governance proposal policy_hash mismatch: constitution requires {expected}, \
         proposal has {actual} (ConstitutionPolicyMismatch)"
    )]
    ConstitutionPolicyMismatch {
        /// Hex-encoded expected constitution script hash.
        expected: String,
        /// Hex-encoded provided policy hash, or "None" if absent.
        actual: String,
    },
}

// ---------------------------------------------------------------------------
// Public validation entry points
// ---------------------------------------------------------------------------

/// Validate a transaction against the current UTxO set and protocol parameters.
///
/// This is a convenience wrapper around [`validate_transaction_with_pools`] that
/// treats all pool registrations as new (no re-registration discount).
///
/// The `utxo_set` parameter accepts anything that implements [`UtxoLookup`],
/// including the standard on-chain `&UtxoSet` and the composite
/// `CompositeUtxoView` used by the mempool validator for chained tx support.
pub fn validate_transaction(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
) -> Result<(), Vec<ValidationError>> {
    validate_transaction_with_pools(
        tx,
        utxo_set,
        params,
        current_slot,
        tx_size,
        slot_config,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Validate a transaction using a [`ValidationContext`] struct.
///
/// This is the preferred entry point for validation with full ledger state,
/// replacing the many-parameter [`validate_transaction_with_pools`] function.
///
/// # Example
///
/// ```rust,ignore
/// use dugite_ledger::validation::{ValidationContext, validate_transaction_with_context};
///
/// let context = ValidationContext::new()
///     .with_pools(pool_ids)
///     .with_treasury(treasury)
///     .with_reward_accounts(accounts)
///     .with_epoch(epoch)
///     .with_dreps(drep_ids)
///     .with_network(NetworkId::Mainnet);
///
/// let result = validate_transaction_with_context(
///     &tx,
///     &utxo_set,
///     &params,
///     current_slot,
///     tx_size,
///     slot_config,
///     context,
/// );
/// ```
pub fn validate_transaction_with_context(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
    context: ValidationContext,
) -> Result<(), Vec<ValidationError>> {
    validate_transaction_with_pools(
        tx,
        utxo_set,
        params,
        current_slot,
        tx_size,
        slot_config,
        context.registered_pools.as_ref(),
        context.current_treasury,
        context.reward_accounts.as_ref(),
        context.current_epoch,
        context.registered_dreps.as_ref(),
        context.registered_vrf_keys.as_ref(),
        context.node_network,
        context.committee_members.as_ref(),
        context.committee_resigned.as_ref(),
        context.stake_key_deposits.as_ref(),
        context.constitution_script_hash,
        context.vote_delegations.as_ref(),
    )
}

/// Validate a transaction with an optional set of registered pools.
///
/// When `registered_pools` is `Some`, pool re-registrations (updating an existing
/// pool's parameters) do not charge an additional deposit — only new pool
/// registrations do. When `None`, all pool registrations are treated as new
/// (deposit always charged).
///
/// When `registered_dreps` is `Some`, duplicate DRep registration certificates
/// (`RegDRep`) are rejected with [`ValidationError::DRepAlreadyRegistered`].
/// When `None`, the DRep re-registration check is skipped.
///
/// When `registered_vrf_keys` is `Some`, pool registration certificates that
/// declare a VRF key hash already held by another pool are rejected with
/// [`ValidationError::VrfKeyHashAlreadyRegistered`] (Conway+ only).
/// When `None`, the VRF key deduplication check is skipped.
///
/// When `committee_members` is `Some`, `CommitteeHotAuth` certificates for cold
/// credentials NOT present in the committee are rejected with
/// [`ValidationError::UnelectedCommitteeMember`] (Conway+ only).
/// When `None`, the committee membership check is skipped.
///
/// When `committee_resigned` is `Some`, `CommitteeHotAuth` certificates for cold
/// credentials that have previously resigned are rejected with
/// [`ValidationError::CommitteeHasPreviouslyResigned`] (Conway+ only).
/// When `None`, the resigned-member check is skipped.
///
/// The `utxo_set` parameter accepts anything that implements [`UtxoLookup`],
/// including the standard on-chain `&UtxoSet` and the composite
/// `CompositeUtxoView` used by the mempool validator for chained tx support.
///
/// The validation pipeline is:
/// 1. Phase-1 structural rules (Rules 1–10, 13–14) via [`phase1::run_phase1_rules`].
/// 2. For Plutus transactions: collateral rules (Rules 11, 11b, 11c) and
///    script data hash (Rule 12).
/// 3. Phase-2 Plutus script execution when all Phase-1 checks pass and redeemers
///    are present.
#[allow(clippy::too_many_arguments)] // validation entry point legitimately needs all context parameters
pub fn validate_transaction_with_pools(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
    registered_pools: Option<&HashSet<Hash28>>,
    current_treasury: Option<u64>,
    reward_accounts: Option<&HashMap<Hash32, Lovelace>>,
    current_epoch: Option<u64>,
    registered_dreps: Option<&HashSet<Hash32>>,
    registered_vrf_keys: Option<&HashMap<Hash32, Hash28>>,
    node_network: Option<dugite_primitives::network::NetworkId>,
    committee_members: Option<&HashSet<Hash32>>,
    committee_resigned: Option<&HashSet<Hash32>>,
    stake_key_deposits: Option<&HashMap<Hash32, u64>>,
    constitution_script_hash: Option<Hash28>,
    vote_delegations: Option<&HashSet<Hash32>>,
) -> Result<(), Vec<ValidationError>> {
    trace!(
        tx_hash = %tx.hash.to_hex(),
        inputs = tx.body.inputs.len(),
        outputs = tx.body.outputs.len(),
        fee = tx.body.fee.0,
        tx_size,
        current_slot,
        "Validation: validating transaction"
    );

    let mut errors = Vec::new();

    // ------------------------------------------------------------------
    // Phase-1 structural rules (Rules 1–10, 13–14)
    // ------------------------------------------------------------------
    phase1::run_phase1_rules(
        tx,
        utxo_set,
        params,
        current_slot,
        tx_size,
        registered_pools,
        current_epoch,
        node_network,
        stake_key_deposits,
        &mut errors,
    );

    // ------------------------------------------------------------------
    // Stake deregistration: non-zero reward account balance check
    //
    // Haskell `StakeKeyHasNonZeroAccountBalanceDELEG` (Shelley DELEG rule and
    // Conway DELEG rule): a stake credential may not be deregistered while its
    // reward account holds any lovelace. The delegator must withdraw rewards
    // before deregistering.
    //
    // This check is only enforced when `reward_accounts` is provided (i.e.,
    // during block validation or mempool admission with ledger context). During
    // simple structural validation where the caller supplies `None`, the balance
    // check is skipped to match the withdrawal-amount check pattern above.
    //
    // Both legacy `StakeDeregistration` (tag 1) and Conway
    // `ConwayStakeDeregistration` (tag 8) are covered — Haskell enforces the
    // same predicate for both certificate variants.
    // ------------------------------------------------------------------
    if let Some(accounts) = reward_accounts {
        for cert in &tx.body.certificates {
            let opt_credential: Option<&dugite_primitives::credentials::Credential> = match cert {
                dugite_primitives::transaction::Certificate::StakeDeregistration(cred) => {
                    Some(cred)
                }
                dugite_primitives::transaction::Certificate::ConwayStakeDeregistration {
                    credential,
                    ..
                } => Some(credential),
                _ => None,
            };
            if let Some(credential) = opt_credential {
                // Replicate the Hash28 → Hash32 zero-padding used in
                // state/certificates.rs `credential_to_hash()` so the lookup
                // key matches the key stored in `self.reward_accounts`.
                let key = credential.to_hash().to_hash32_padded();
                if let Some(balance) = accounts.get(&key) {
                    if balance.0 > 0 {
                        errors.push(ValidationError::StakeKeyHasNonZeroBalance {
                            credential_hash: key.to_hex(),
                            balance: balance.0,
                        });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Stake key already registered check (Haskell `StakeKeyRegisteredDELEG`)
    //
    // A StakeRegistration or ConwayStakeRegistration certificate is rejected
    // when the named credential is already present in the reward accounts map
    // (i.e., the key has previously registered and not yet deregistered).
    //
    // This check is only enforced when `reward_accounts` is provided (block
    // validation mode). When `None`, the check is skipped to match the
    // pattern of other ledger-state-dependent checks (e.g. the balance check
    // above). Both the pre-Conway `StakeRegistration` (tag 0) and the Conway
    // `ConwayStakeRegistration` (tag 7) variants are covered.
    //
    // Reference: Haskell `StakeKeyRegisteredDELEG` in
    // `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Deleg`.
    // ------------------------------------------------------------------
    if let Some(accounts) = reward_accounts {
        for cert in &tx.body.certificates {
            let opt_cred: Option<&dugite_primitives::credentials::Credential> = match cert {
                dugite_primitives::transaction::Certificate::StakeRegistration(cred) => Some(cred),
                dugite_primitives::transaction::Certificate::ConwayStakeRegistration {
                    credential: cred,
                    ..
                } => Some(cred),
                // Combined registration certificates also register a stake key
                // and must be rejected if the credential is already registered.
                // Reference: Haskell `AlreadyRegisteredKey` in Conway DELEG rule.
                dugite_primitives::transaction::Certificate::RegStakeDeleg {
                    credential: cred,
                    ..
                } => Some(cred),
                dugite_primitives::transaction::Certificate::VoteRegDeleg {
                    credential: cred,
                    ..
                } => Some(cred),
                dugite_primitives::transaction::Certificate::RegStakeVoteDeleg {
                    credential: cred,
                    ..
                } => Some(cred),
                _ => None,
            };
            if let Some(credential) = opt_cred {
                // Use the same Hash28 → Hash32 zero-padding as the reward
                // account map key (mirrors `credential_to_hash` in state/mod.rs).
                let key = credential.to_hash().to_hash32_padded();
                if accounts.contains_key(&key) {
                    errors.push(ValidationError::StakeKeyAlreadyRegistered {
                        credential_hash: key.to_hex(),
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Delegation to unregistered pool (Haskell `DelegateeStakePoolNotRegisteredDELEG`)
    //
    // A delegation certificate that targets a pool ID not currently registered
    // in `pool_params` is rejected. This covers all variants that carry a
    // target pool hash: `StakeDelegation` (tag 2), `RegStakeDeleg` (tag 11),
    // `StakeVoteDelegation` (tag 13), `RegStakeVoteDeleg` (tag 14).
    //
    // `VoteRegDeleg` (tag 15) does NOT include a pool delegation component —
    // it registers and sets a DRep vote delegation only — so it is excluded.
    //
    // This check is only enforced when `registered_pools` is provided.
    //
    // Reference: Haskell `DelegateeStakePoolNotRegisteredDELEG` in
    // `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Deleg`.
    // ------------------------------------------------------------------
    if let Some(pools) = registered_pools {
        for cert in &tx.body.certificates {
            let opt_pool: Option<Hash28> = match cert {
                dugite_primitives::transaction::Certificate::StakeDelegation {
                    pool_hash, ..
                } => Some(*pool_hash),
                dugite_primitives::transaction::Certificate::RegStakeDeleg {
                    pool_hash, ..
                } => Some(*pool_hash),
                dugite_primitives::transaction::Certificate::StakeVoteDelegation {
                    pool_hash,
                    ..
                } => Some(*pool_hash),
                dugite_primitives::transaction::Certificate::RegStakeVoteDeleg {
                    pool_hash,
                    ..
                } => Some(*pool_hash),
                _ => None,
            };
            if let Some(pool_id) = opt_pool {
                if !pools.contains(&pool_id) {
                    errors.push(ValidationError::DelegateePoolNotRegistered {
                        pool_id: pool_id.to_hex(),
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // DRep already registered check (Haskell `ConwayDRepAlreadyRegistered`)
    //
    // A `RegDRep` certificate is rejected when the named DRep credential is
    // already present in the DRep registry. This check is only enforced in
    // Conway (protocol >= 9) when `registered_dreps` is provided.
    //
    // Reference: Haskell `ConwayDRepAlreadyRegistered` in
    // `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Deleg`.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        if let Some(dreps) = registered_dreps {
            for cert in &tx.body.certificates {
                if let dugite_primitives::transaction::Certificate::RegDRep { credential, .. } =
                    cert
                {
                    let key = credential.to_hash().to_hash32_padded();
                    if dreps.contains(&key) {
                        errors.push(ValidationError::DRepAlreadyRegistered {
                            credential_hash: key.to_hex(),
                        });
                    }
                }
            }
        }

        // ------------------------------------------------------------------
        // DRep deposit amount validation (Haskell `ConwayDRepIncorrectDeposit`)
        //
        // Each `RegDRep` certificate's inline deposit must exactly match the
        // current `drep_deposit` protocol parameter. Value conservation uses
        // the declared deposit for accounting, but the GOVCERT rule separately
        // validates that it equals the parameter.
        //
        // Reference: Haskell `ConwayDRepIncorrectDeposit` in
        // `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.GovCert`.
        // ------------------------------------------------------------------
        for cert in &tx.body.certificates {
            if let dugite_primitives::transaction::Certificate::RegDRep { deposit, .. } = cert {
                if *deposit != params.drep_deposit {
                    errors.push(ValidationError::DRepIncorrectDeposit {
                        declared: deposit.0,
                        expected: params.drep_deposit.0,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // VRF key deduplication (Haskell `VRFKeyHashAlreadyRegistered`, Conway+)
    //
    // From Conway (protocol >= 9), a pool registration certificate whose VRF
    // key hash is already registered to a DIFFERENT pool is rejected. A pool
    // re-registering its own parameters with the same VRF key is permitted (the
    // key already belongs to that pool, so the new registration is not a
    // collision).
    //
    // This check is only enforced when `registered_vrf_keys` is provided (block
    // validation mode). The map is keyed by VRF key hash (Hash32) and maps to
    // the pool ID (Hash28) that currently holds that key.
    //
    // Reference: Haskell `VRFKeyHashAlreadyRegistered` in
    // `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Pool`.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        if let Some(vrf_keys) = registered_vrf_keys {
            for cert in &tx.body.certificates {
                if let dugite_primitives::transaction::Certificate::PoolRegistration(pool_params) =
                    cert
                {
                    // Check if this VRF key is held by a different pool.
                    // Same pool re-registering with the same key is fine.
                    if let Some(&existing_pool) = vrf_keys.get(&pool_params.vrf_keyhash) {
                        if existing_pool != pool_params.operator {
                            errors.push(ValidationError::VrfKeyHashAlreadyRegistered {
                                vrf_keyhash: pool_params.vrf_keyhash.to_hex(),
                                existing_pool_id: existing_pool.to_hex(),
                            });
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // CommitteeHotAuth: elected-member and non-resigned checks (Conway+)
    //
    // Haskell `CERT` rule predicates in
    // `Cardano.Ledger.Conway.Rules.Cert`:
    //
    //   1. "failOnNonEmpty unelected": every cold credential in a
    //      CommitteeHotAuth certificate must appear in the current
    //      committee (committee_expiration / committee_members map).
    //      → `ValidationError::UnelectedCommitteeMember`
    //
    //   2. "membersResigned ∩ {coldKey} = ∅": a cold credential that has
    //      previously resigned via CommitteeColdResign may not re-authorize
    //      a hot key without being re-elected.
    //      → `ValidationError::CommitteeHasPreviouslyResigned`
    //
    // Both checks are only enforced in Conway (protocol >= 9) and only
    // when the relevant state is provided (block application mode).
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        for cert in &tx.body.certificates {
            if let dugite_primitives::transaction::Certificate::CommitteeHotAuth {
                cold_credential,
                ..
            } = cert
            {
                let cold_key = cold_credential.to_hash().to_hash32_padded();

                // Check 1: cold credential must be a current CC member.
                if let Some(members) = committee_members {
                    if !members.contains(&cold_key) {
                        errors.push(ValidationError::UnelectedCommitteeMember {
                            cold_credential_hash: cold_key.to_hex(),
                        });
                    }
                }

                // Check 2: cold credential must not have previously resigned.
                if let Some(resigned) = committee_resigned {
                    if resigned.contains(&cold_key) {
                        errors.push(ValidationError::CommitteeHasPreviouslyResigned {
                            cold_credential_hash: cold_key.to_hex(),
                        });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Withdrawal validation (Haskell `wdrlNotZero` + balance check)
    //
    // - Zero-amount withdrawals are rejected in Shelley–Babbage (proto < 9).
    //   In Conway (proto >= 9), zero-amount withdrawals are valid — they
    //   allow "touching" a reward account for DRep activity tracking.
    // - When `reward_accounts` is provided (block application mode),
    //   each withdrawal amount must exactly match the on-chain reward
    //   balance, and the account must be registered.
    // ------------------------------------------------------------------
    let conway_or_later = params.protocol_version_major >= 9;
    for (reward_account_bytes, amount) in &tx.body.withdrawals {
        // Format the reward account as a hex string for error messages.
        let account_hex = reward_account_bytes.iter().fold(
            String::with_capacity(reward_account_bytes.len() * 2),
            |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            },
        );
        // Conway relaxed the wdrlNotZero predicate — zero withdrawals are
        // now valid (used for DRep activity / reward account touching).
        if amount.0 == 0 && !conway_or_later {
            errors.push(ValidationError::ZeroWithdrawal {
                account: account_hex.clone(),
            });
        }
        if let Some(accounts) = reward_accounts {
            let key = crate::state::LedgerState::reward_account_to_hash(reward_account_bytes);
            match accounts.get(&key) {
                Some(balance) => {
                    if amount.0 != balance.0 {
                        errors.push(ValidationError::IncorrectWithdrawalAmount {
                            account: account_hex,
                            declared: amount.0,
                            actual: balance.0,
                        });
                    }
                }
                None => {
                    // Unregistered reward account — the withdrawal amount cannot
                    // match any balance, so report as incorrect (actual = 0).
                    errors.push(ValidationError::IncorrectWithdrawalAmount {
                        account: account_hex,
                        declared: amount.0,
                        actual: 0,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Conway LEDGER rule: ConwayWdrlNotDelegatedToDRep (PV >= 10)
    //
    // Every KeyHash reward account making a withdrawal must have a DRep
    // delegation. Script-credential accounts are exempt. Any delegation
    // value (including AlwaysAbstain/AlwaysNoConfidence) satisfies the check.
    // Uses the certState BEFORE the current tx's certificates are applied.
    //
    // Reference: Haskell `validateWithdrawalsDelegated` in
    // `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Ledger`.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 10 {
        if let Some(delegations) = vote_delegations {
            for reward_addr in tx.body.withdrawals.keys() {
                if reward_addr.len() < 29 {
                    continue;
                }
                let header = reward_addr[0];
                // Script-credential reward accounts (header bit 4 set) are exempt
                let is_script = (header & 0x10) != 0;
                if is_script {
                    continue;
                }
                // KeyHash credential — must have DRep delegation
                if let Ok(cred_hash) = Hash28::try_from(&reward_addr[1..29]) {
                    let key = cred_hash.to_hash32_padded();
                    if !delegations.contains(&key) {
                        errors.push(ValidationError::WdrlNotDelegatedToDRep {
                            credential_hash: key.to_hex(),
                        });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Conway LEDGER rule: currentTreasuryValue must match ledger treasury.
    // This prevents mempool admission of transactions with stale/wrong
    // treasury assertions, which would cause forged blocks to be rejected.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        if let (Some(declared), Some(actual)) = (tx.body.treasury_value.as_ref(), current_treasury)
        {
            if declared.0 != actual {
                errors.push(ValidationError::TreasuryValueMismatch {
                    declared: declared.0,
                    actual,
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Conway GOV rule: constitution guardrail policy_hash validation.
    //
    // ParameterChange and TreasuryWithdrawals proposals must carry a
    // `policy_hash` matching the constitution's guardrail script hash.
    // Without this check, a transaction could reference an arbitrary script
    // (or omit the policy_hash entirely), bypassing the guardrail.
    //
    // Reference: Haskell GOV rule — policy hash must match the constitution's
    // script hash for governed governance actions.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        if let Some(required_hash) = constitution_script_hash {
            for (idx, proposal) in tx.body.proposal_procedures.iter().enumerate() {
                let policy_hash = match &proposal.gov_action {
                    GovAction::ParameterChange { policy_hash, .. }
                    | GovAction::TreasuryWithdrawals { policy_hash, .. } => policy_hash.as_ref(),
                    _ => continue,
                };
                match policy_hash {
                    Some(provided) if *provided == required_hash => {
                        // Valid — policy hash matches constitution guardrail
                    }
                    Some(provided) => {
                        errors.push(ValidationError::ConstitutionPolicyMismatch {
                            expected: required_hash.to_hex(),
                            actual: provided.to_hex(),
                        });
                    }
                    None => {
                        errors.push(ValidationError::ConstitutionPolicyMismatch {
                            expected: required_hash.to_hex(),
                            actual: format!("None (proposal index {idx})"),
                        });
                    }
                }
            }
        }
    }

    // ppuWellFormed check for ParameterChange proposals (Conway GOV rule)
    conway::check_pparam_update_well_formed(params, &tx.body, &mut errors);

    // ------------------------------------------------------------------
    // Rules 11, 11b, 11c, 12 — Plutus-transaction-specific checks
    //
    // These are only enforced when the transaction includes Plutus scripts
    // or redeemers. They are split into their own modules to keep the rule
    // logic focused and independently testable.
    // ------------------------------------------------------------------
    if scripts::has_plutus_scripts(tx) {
        // Rule 11: collateral inputs, percentage, net-ADA check, total_collateral
        // Rule 11b: redeemer index bounds
        collateral::check_collateral(tx, utxo_set, params, &mut errors);

        // Rule 11c: every script-locked input/withdrawal and every Plutus minting
        // policy must have a matching redeemer (Spend / Reward / Mint respectively).
        // Matches Haskell's `scriptsNeeded` check.
        collateral::check_script_redeemers(tx, utxo_set, &mut errors);

        // Alonzo UTXOW: every redeemer in the witness set must map to a valid
        // script purpose. Redeemers with no matching purpose are rejected.
        // Matches Haskell's `hasExactSetOfRedeemers` / `ExtraRedeemers`.
        collateral::check_extra_redeemers(tx, utxo_set, &mut errors);

        // Rule 12: script data hash (mkScriptIntegrity) — covers redeemers,
        // datums, cost models, and language versions.
        scripts::check_script_data_hash(tx, utxo_set, params, &mut errors);

        // ------------------------------------------------------------------
        // Phase-2: Execute Plutus scripts when redeemers are present.
        //
        // Both `raw_cbor` and `slot_config` are required for Plutus evaluation.
        // A missing `raw_cbor` means the transaction was constructed locally
        // without being round-tripped through CBOR — that is a programming
        // error and must be surfaced. Silent bypass is not allowed.
        // ------------------------------------------------------------------
        let has_redeemers = !tx.witness_set.redeemers.is_empty();
        if errors.is_empty() && has_redeemers {
            if tx.raw_cbor.is_none() {
                debug!(
                    tx_hash = %tx.hash.to_hex(),
                    "Plutus transaction missing raw CBOR for script evaluation"
                );
                errors.push(ValidationError::MissingRawCbor);
            }
            if slot_config.is_none() {
                debug!(
                    tx_hash = %tx.hash.to_hex(),
                    "Plutus transaction missing slot configuration for script evaluation"
                );
                errors.push(ValidationError::MissingSlotConfig);
            }
            if let (Some(ref _raw), Some(sc)) = (&tx.raw_cbor, slot_config) {
                let cost_models_cbor = params.cost_models.to_cbor();
                // uplc::tx::eval_phase_two_raw expects initial_budget as (cpu_steps, mem_units).
                // Our ExUnits struct uses { mem, steps } where mem=memory_units and steps=cpu_steps.
                // Swap the fields to match the uplc convention: (steps, mem) = (cpu, mem).
                let max_ex = (params.max_tx_ex_units.steps, params.max_tx_ex_units.mem);
                if let Err(e) =
                    evaluate_plutus_scripts(tx, utxo_set, cost_models_cbor.as_deref(), max_ex, sc)
                {
                    errors.push(ValidationError::ScriptFailed(e.to_string()));
                }
            }
        }
    }

    if errors.is_empty() {
        debug!(tx_hash = %tx.hash.to_hex(), "Validation: transaction valid");
        Ok(())
    } else {
        warn!(
            tx_hash = %tx.hash.to_hex(),
            error_count = errors.len(),
            errors = ?errors,
            "Validation: transaction rejected"
        );
        Err(errors)
    }
}
