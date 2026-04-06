//! Intermediate types representing the decoded Haskell ExtLedgerState.
//!
//! These mirror the Haskell CBOR structure and are converted to dugite's
//! native LedgerState in a separate step.

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{EpochNo, SlotNo};
use std::collections::HashMap;

/// Top-level decoded Haskell ledger snapshot.
#[derive(Debug)]
pub struct HaskellLedgerState {
    pub tip_slot: SlotNo,
    pub tip_block_no: u64,
    pub tip_hash: Hash32,
    pub epoch: EpochNo,
    pub new_epoch_state: HaskellNewEpochState,
    pub praos_state: HaskellPraosState,
}

/// NewEpochState fields extracted from the CBOR.
#[derive(Debug)]
pub struct HaskellNewEpochState {
    pub epoch: EpochNo,
    pub blocks_made_prev: HashMap<Hash28, u64>,
    pub blocks_made_cur: HashMap<Hash28, u64>,
    pub treasury: u64,
    pub reserves: u64,
    pub cur_pparams: ProtocolParameters,
    pub prev_pparams: ProtocolParameters,
    pub deposited: u64,
    pub fees: u64,
    pub donation: u64,
    pub cert_state: HaskellCertState,
    pub snapshots: HaskellSnapShots,
    pub pool_distr: HashMap<Hash28, HaskellPoolDistrEntry>,
    pub pool_distr_total_stake: u64,
    pub gov_state: HaskellGovState,
    /// Instant stake: credential → lovelace (reconstructed from DState + PState).
    pub instant_stake: HashMap<(u8, Hash28), u64>,
}

/// PraosState from the HeaderState telescope.
#[derive(Debug)]
pub struct HaskellPraosState {
    pub last_slot: Option<SlotNo>,
    pub opcert_counters: HashMap<Hash28, u64>,
    pub evolving_nonce: Hash32,
    pub candidate_nonce: Hash32,
    pub epoch_nonce: Hash32,
    pub lab_nonce: Hash32,
    pub last_epoch_block_nonce: Hash32,
}

/// Individual pool stake entry from PoolDistr.
#[derive(Debug)]
pub struct HaskellPoolDistrEntry {
    pub stake_ratio_num: u64,
    pub stake_ratio_den: u64,
    pub stake_coin: u64,
    pub vrf_hash: Hash32,
}

/// Decoded CertState (VState + PState + DState).
#[derive(Debug)]
pub struct HaskellCertState {
    pub vstate: HaskellVState,
    pub pstate: HaskellPState,
    pub dstate: HaskellDState,
}

/// VState: DRep registrations + committee state.
#[derive(Debug)]
pub struct HaskellVState {
    /// DRep credential → (expiry_epoch, deposit, anchor_url, anchor_hash)
    pub dreps: HashMap<(u8, Hash28), HaskellDRepState>,
    /// Committee cold credential → authorization (hot credential or resigned)
    pub committee_state: HashMap<(u8, Hash28), HaskellCommitteeAuth>,
    pub dormant_epochs: u64,
}

/// DRep registration state.
#[derive(Debug)]
pub struct HaskellDRepState {
    pub expiry: EpochNo,
    pub deposit: u64,
    pub anchor: Option<(String, Hash32)>,
    // delegators set skipped for now — reconstructed from DState accounts
}

/// Committee member authorization.
#[derive(Debug)]
pub enum HaskellCommitteeAuth {
    /// tag 0: CommitteeHotCredential (type, hash28)
    Hot(u8, Hash28),
    /// tag 1: CommitteeMemberResigned (optional anchor)
    Resigned(Option<(String, Hash32)>),
}

/// PState: pool registrations.
#[derive(Debug)]
pub struct HaskellPState {
    /// VRF hash → reference count
    pub vrf_key_hashes: HashMap<Hash32, u64>,
    pub stake_pools: HashMap<Hash28, HaskellStakePoolState>,
    pub future_pool_params: HashMap<Hash28, HaskellPoolParams>,
    pub retirements: HashMap<Hash28, EpochNo>,
}

/// StakePoolState (9 or 10 fields from PState).
#[derive(Debug)]
pub struct HaskellStakePoolState {
    pub vrf_hash: Hash32,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
    /// Raw 29-byte reward address bytes.
    pub reward_account: Vec<u8>,
    pub owners: Vec<Hash28>,
    pub relays: Vec<HaskellRelay>,
    pub metadata: Option<(String, Hash32)>,
    pub deposit: u64,
    // delegators (field 10) skipped — only in newer nodes
}

/// PoolParams (9 fields from future_pool_params) — same layout as StakePoolState.
pub type HaskellPoolParams = HaskellStakePoolState;

/// Pool relay descriptor.
#[derive(Debug)]
pub enum HaskellRelay {
    SingleHostAddr(Option<u16>, Option<[u8; 4]>, Option<[u8; 16]>),
    SingleHostName(Option<u16>, String),
    MultiHostName(String),
}

/// DState: accounts + genesis delegates.
#[derive(Debug)]
pub struct HaskellDState {
    /// Credential → ConwayAccountState (balance, deposit, pool_delegation, drep_delegation)
    pub accounts: HashMap<(u8, Hash28), HaskellAccountState>,
    /// Genesis key hash → (delegate hash, VRF key hash)
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    pub i_rewards_reserves: HashMap<(u8, Hash28), u64>,
    pub i_rewards_treasury: HashMap<(u8, Hash28), u64>,
    pub delta_reserves: i64,
    pub delta_treasury: i64,
}

/// ConwayAccountState = array(4) [balance, deposit, pool?, drep?]
#[derive(Debug)]
pub struct HaskellAccountState {
    pub balance: u64,
    pub deposit: u64,
    pub pool_delegation: Option<Hash28>,
    pub drep_delegation: Option<HaskellDRep>,
}

/// DRep delegation target.
#[derive(Debug)]
pub enum HaskellDRep {
    KeyHash(Hash28),
    ScriptHash(Hash28),
    AlwaysAbstain,
    AlwaysNoConfidence,
}

/// SnapShots (mark/set/go) decoded from the EpochState.
#[derive(Debug)]
pub struct HaskellSnapShots {
    pub mark: HaskellSnapShot,
    pub set: HaskellSnapShot,
    pub go: HaskellSnapShot,
    pub fee: u64,
}

/// Individual stake snapshot. Handles both old (array 3) and new (array 2) formats.
#[derive(Debug)]
pub struct HaskellSnapShot {
    /// Credential → staked lovelace.
    pub stake: HashMap<(u8, Hash28), u64>,
    /// Credential → pool hash.
    pub delegations: HashMap<(u8, Hash28), Hash28>,
    /// Pool hash → pool snapshot params.
    pub pool_params: HashMap<Hash28, HaskellSnapShotPool>,
}

/// Pool data within a snapshot.
#[derive(Debug)]
pub struct HaskellSnapShotPool {
    pub vrf_hash: Hash32,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
    pub reward_account: Vec<u8>,
    pub owners: Vec<Hash28>,
    pub relays: Vec<HaskellRelay>,
    pub metadata: Option<(String, Hash32)>,
}

/// Governance state (simplified — captures fields needed by dugite).
#[derive(Debug)]
pub struct HaskellGovState {
    /// Raw proposals CBOR (complex structure, decoded on-demand).
    pub proposals_raw: Vec<u8>,
    /// Committee raw CBOR bytes (if present).
    pub committee_raw: Option<Vec<u8>>,
    /// Constitution anchor + optional script hash.
    pub constitution: Option<HaskellConstitution>,
    /// DRep pulsing state raw CBOR.
    pub drep_pulsing_raw: Vec<u8>,
    /// FuturePParams variant tag (0=none, 1=definite, 2=potential).
    pub future_pparams_tag: u8,
    pub future_pparams: Option<ProtocolParameters>,
}

/// Constitution anchor.
#[derive(Debug)]
pub struct HaskellConstitution {
    pub anchor_url: String,
    pub anchor_hash: Hash32,
    pub script_hash: Option<Hash28>,
}
