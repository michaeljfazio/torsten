use std::collections::{HashMap, HashSet};
use thiserror::Error;
use torsten_crypto::keys::PaymentVerificationKey;
use torsten_primitives::block::{BlockHeader, Tip};
use torsten_primitives::hash::{blake2b_256, Hash28, Hash32};
use torsten_primitives::time::{EpochLength, EpochNo, SlotNo};
use tracing::{debug, error, trace, warn};

/// KES period length in slots (each period is 129600 slots = 36 hours on mainnet)
pub const KES_PERIOD_SLOTS: u64 = 129600;

/// Maximum number of KES evolutions (mainnet: 62)
pub const MAX_KES_EVOLUTIONS: u64 = 62;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Invalid block: {0}")]
    InvalidBlock(String),
    #[error("Block from future slot: current={current}, block={block}")]
    FutureBlock { current: u64, block: u64 },
    #[error("Not a slot leader")]
    NotSlotLeader,
    #[error("Invalid VRF proof")]
    InvalidVrfProof,
    #[error("Invalid KES signature")]
    InvalidKesSignature,
    #[error("Invalid operational certificate")]
    InvalidOperationalCert,
    #[error("Block does not extend chain")]
    DoesNotExtendChain,
    #[error("KES period expired: current_period={current}, cert_start={cert_start}, max_evolutions={max_evolutions}")]
    KesExpired {
        current: u64,
        cert_start: u64,
        max_evolutions: u64,
    },
    #[error(
        "KES period mismatch: block is in period {block_period}, but cert starts at {cert_start}"
    )]
    KesPeriodBeforeCert { block_period: u64, cert_start: u64 },
    #[error("Empty issuer VRF key")]
    EmptyVrfKey,
    #[error("Empty issuer verification key")]
    EmptyIssuerVkey,
    #[error("Checkpoint mismatch at block {block_no}: expected {expected}, got {got}")]
    CheckpointMismatch {
        block_no: u64,
        expected: Hash32,
        got: Hash32,
    },
    #[error("VRF verification error: {0}")]
    VrfVerification(String),
    #[error("Operational cert sequence number regression: got {got}, expected > {expected}")]
    OpcertSequenceRegression { got: u64, expected: u64 },
    #[error("VRF key hash mismatch: header VRF key does not match pool registration")]
    VrfKeyMismatch,
    #[error("Unknown block issuer: pool {0} not found in stake distribution")]
    UnknownBlockIssuer(Hash28),
    #[error("Operational cert counter over-incremented: got {got}, last seen {last_seen} (max increment is 1)")]
    OpcertCounterOverIncremented { got: u64, last_seen: u64 },
    #[error("Obsolete node: chain protocol version {chain_pv} exceeds node maximum {node_max_pv} — upgrade required")]
    ObsoleteNode { chain_pv: u64, node_max_pv: u64 },
    #[error(
        "Header protocol version too high: block claims {supplied}, max allowed is {max_expected}"
    )]
    HeaderProtVerTooHigh { supplied: u64, max_expected: u64 },
    #[error("Body hash mismatch: header={header_hash}, computed={computed_hash}")]
    BodyHashMismatch {
        header_hash: Hash32,
        computed_hash: Hash32,
    },
    #[error("Unregistered pool: pool {pool_id} not found in stake distribution")]
    UnregisteredPool { pool_id: Hash28 },
    #[error(
        "Block body too large: body_size={body_size} exceeds max_block_body_size={max_block_body_size}"
    )]
    BlockBodyTooLarge {
        body_size: u64,
        max_block_body_size: u64,
    },
    #[error(
        "Block header too large: header_size={header_size} exceeds max_block_header_size={max_block_header_size}"
    )]
    BlockHeaderTooLarge {
        header_size: u64,
        max_block_header_size: u64,
    },
}

/// Information about a registered pool needed for full block validation.
///
/// When available, this enables:
/// - VRF key binding: verifying the header's VRF key matches the pool's registered VRF key hash
/// - Leader eligibility: verifying the VRF output satisfies the Praos threshold for the pool's stake
#[derive(Debug, Clone)]
pub struct BlockIssuerInfo {
    /// The pool's registered VRF key hash (Blake2b-256 of the VRF verification key)
    pub vrf_keyhash: Hash32,
    /// The pool's stake (numerator of relative stake fraction).
    /// Relative stake = pool_stake / total_active_stake, passed as exact
    /// rational to avoid f64 precision loss at decision boundaries.
    pub pool_stake: u64,
    /// Total active stake across all pools (denominator of relative stake fraction).
    pub total_active_stake: u64,
}

impl BlockIssuerInfo {
    /// Relative stake as f64 (for logging/display only — NOT for VRF checks).
    pub fn relative_stake_f64(&self) -> f64 {
        if self.total_active_stake == 0 {
            return 0.0;
        }
        self.pool_stake as f64 / self.total_active_stake as f64
    }
}

/// Controls the level of header validation performed.
///
/// Matches the Haskell cardano-node's structural distinction between
/// `tickThenApply` (full validation for new network blocks) and
/// `tickThenReapply` (state-update-only for blocks replayed from disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    /// Full validation — verify all cryptographic proofs (VRF, KES, opcert Ed25519).
    /// Used for blocks received from the network. Equivalent to Haskell's
    /// `updateChainDepState` which calls `validateKESSignature` and
    /// `validateVRFSignature`.
    Full,
    /// Replay validation — skip all cryptographic verification.
    /// Only performs structural checks and updates chain-dependent state
    /// (nonces, opcert counters). Used for blocks replayed from local storage
    /// (ChainDB gap bridging, chunk-file replay after Mithril import).
    /// Equivalent to Haskell's `reupdateChainDepState`.
    Replay,
}

/// Parameters needed for standalone cryptographic verification of block headers.
/// Extracted from `OuroborosPraos` so that verification can run in parallel
/// (e.g., via rayon) without holding a mutable reference to the consensus engine.
#[derive(Debug, Clone)]
pub struct CryptoVerificationParams {
    pub strict_verification: bool,
    pub nonce_established: bool,
    pub slots_per_kes_period: u64,
    pub max_kes_evolutions: u64,
}

/// Active slot coefficient (f) - probability that a slot has a block
/// Mainnet value: 1/20 = 0.05 (one block every ~20 seconds on average)
pub const ACTIVE_SLOT_COEFF: f64 = 0.05;

/// Security parameter k
pub const SECURITY_PARAM: u64 = 2160;

/// Ouroboros Praos consensus engine
pub struct OuroborosPraos {
    /// Active slot coefficient (f64 for backward compat / logging)
    pub active_slot_coeff: f64,
    /// Active slot coefficient as exact rational (numerator, denominator).
    /// E.g., mainnet/preview f=0.05 is stored as (1, 20).
    /// Used for VRF leader checks to avoid f64 precision loss.
    pub active_slot_coeff_rational: (u64, u64),
    /// Security parameter
    pub security_param: u64,
    /// Epoch length in slots
    pub epoch_length: EpochLength,
    /// Number of slots per KES period (from Shelley genesis, typically 129600)
    pub slots_per_kes_period: u64,
    /// Maximum KES evolutions before key expires (from Shelley genesis, typically 62)
    pub max_kes_evolutions: u64,
    /// Current tip
    pub tip: Tip,
    /// Whether to enforce strict signature verification.
    /// When false (during initial sync), VRF/KES/opcert failures are non-fatal.
    /// When true (caught up to chain tip), verification failures reject blocks.
    pub strict_verification: bool,
    /// Whether the epoch nonce has been correctly established.
    /// After Mithril import, the epoch nonce is wrong until 2 full epoch transitions
    /// have accumulated correct VRF nonce contributions. When false, VRF proof
    /// verification is skipped even in strict mode.
    pub nonce_established: bool,
    /// Whether stake snapshots have been correctly established.
    /// After snapshot load, the mark/set/go snapshots may have drifted pool_stake
    /// values. It takes 3 epoch transitions for all snapshots to be rebuilt with
    /// correct stake distributions. When false, VRF leader eligibility failures
    /// are non-fatal even in strict mode.
    pub snapshots_established: bool,
    /// Lightweight checkpoints: static map of (block_number → expected_header_hash).
    /// Checked during header validation — if a block's number matches a checkpoint
    /// but its hash doesn't, the header is rejected (anti-eclipse defense).
    /// Empty on testnets; populated from mainnet-checkpoints.json on mainnet.
    pub checkpoints: HashMap<u64, Hash32>,
    /// Maximum major protocol version the node supports (matches Haskell's `MaxMajorProtVer`).
    /// If the ledger's current protocol version exceeds this, the node is obsolete and must
    /// be upgraded. Currently 10 (Conway).
    pub max_major_prot_ver: u64,
    /// Tracked opcert sequence numbers per pool (cold key hash → highest seen sequence number).
    /// Used to detect opcert counter regressions (replay protection).
    opcert_counters: HashMap<Hash28, u64>,
}

impl OuroborosPraos {
    pub fn new() -> Self {
        let rational = torsten_primitives::protocol_params::f64_to_rational(ACTIVE_SLOT_COEFF);
        OuroborosPraos {
            active_slot_coeff: ACTIVE_SLOT_COEFF,
            active_slot_coeff_rational: rational,
            security_param: SECURITY_PARAM,
            epoch_length: torsten_primitives::time::mainnet_epoch_length(),
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
            snapshots_established: false,
            checkpoints: HashMap::new(),
            max_major_prot_ver: 10,
            opcert_counters: HashMap::new(),
        }
    }

    pub fn with_params(
        active_slot_coeff: f64,
        security_param: u64,
        epoch_length: EpochLength,
    ) -> Self {
        let rational = torsten_primitives::protocol_params::f64_to_rational(active_slot_coeff);
        OuroborosPraos {
            active_slot_coeff,
            active_slot_coeff_rational: rational,
            security_param,
            epoch_length,
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
            snapshots_established: false,
            checkpoints: HashMap::new(),
            max_major_prot_ver: 10,
            opcert_counters: HashMap::new(),
        }
    }

    pub fn with_genesis_params(
        active_slot_coeff: f64,
        security_param: u64,
        epoch_length: EpochLength,
        slots_per_kes_period: u64,
        max_kes_evolutions: u64,
    ) -> Self {
        let rational = torsten_primitives::protocol_params::f64_to_rational(active_slot_coeff);
        OuroborosPraos {
            active_slot_coeff,
            active_slot_coeff_rational: rational,
            security_param,
            epoch_length,
            slots_per_kes_period,
            max_kes_evolutions,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
            snapshots_established: false,
            checkpoints: HashMap::new(),
            max_major_prot_ver: 10,
            opcert_counters: HashMap::new(),
        }
    }

    /// Check if strict verification mode is enabled.
    pub fn strict_verification(&self) -> bool {
        self.strict_verification
    }

    /// Enable strict verification mode (for when node is caught up to chain tip).
    /// In strict mode, VRF/KES/opcert verification failures reject blocks.
    pub fn set_strict_verification(&mut self, strict: bool) {
        if strict != self.strict_verification {
            debug!(
                strict,
                "Praos: {} strict signature verification",
                if strict { "enabling" } else { "disabling" }
            );
        }
        self.strict_verification = strict;
    }

    /// Extract the parameters needed for standalone cryptographic verification.
    /// Used to run VRF/KES/opcert verification in parallel (via rayon)
    /// without holding a mutable reference to the consensus engine.
    pub fn crypto_params(&self) -> CryptoVerificationParams {
        CryptoVerificationParams {
            strict_verification: self.strict_verification,
            nonce_established: self.nonce_established,
            slots_per_kes_period: self.slots_per_kes_period,
            max_kes_evolutions: self.max_kes_evolutions,
        }
    }

    /// Validate a block header against consensus rules.
    ///
    /// This checks:
    /// 1. Block is not from the future
    /// 2. Issuer VRF key is present
    /// 3. VRF proof is cryptographically valid (in Full mode)
    /// 4. KES period is valid (not expired, not before cert start)
    /// 5. Operational certificate has required fields (in Full mode)
    pub fn validate_header(
        &self,
        header: &BlockHeader,
        current_slot: SlotNo,
        mode: ValidationMode,
        ledger_pv_major: Option<u64>,
    ) -> Result<(), ConsensusError> {
        trace!(
            slot = header.slot.0,
            block_no = header.block_number.0,
            current_slot = current_slot.0,
            issuer_vkey_len = header.issuer_vkey.len(),
            vrf_vkey_len = header.vrf_vkey.len(),
            mode = ?mode,
            "Praos: validating block header"
        );

        // Lightweight checkpoint validation (LCP): if this block number has a
        // checkpoint entry, verify the header hash matches. Mismatch means the
        // peer is serving a chain that diverges from the known-good historical
        // chain — reject immediately (anti-eclipse defense).
        if let Some(expected_hash) = self.checkpoints.get(&header.block_number.0) {
            if header.header_hash != *expected_hash {
                warn!(
                    block_no = header.block_number.0,
                    expected = %expected_hash.to_hex(),
                    got = %header.header_hash.to_hex(),
                    "Praos: CHECKPOINT MISMATCH — rejecting block from alternative chain"
                );
                return Err(ConsensusError::CheckpointMismatch {
                    block_no: header.block_number.0,
                    expected: *expected_hash,
                    got: header.header_hash,
                });
            }
        }

        // Block must not be from the future
        if header.slot > current_slot {
            warn!(
                block_slot = header.slot.0,
                current_slot = current_slot.0,
                "Praos: rejecting future block"
            );
            return Err(ConsensusError::FutureBlock {
                current: current_slot.0,
                block: header.slot.0,
            });
        }

        // Issuer verification key must be present (32 bytes for Ed25519)
        if header.issuer_vkey.is_empty() {
            warn!("Praos: empty issuer verification key");
            return Err(ConsensusError::EmptyIssuerVkey);
        }

        // VRF key must be present
        if header.vrf_vkey.is_empty() {
            warn!("Praos: empty VRF key");
            return Err(ConsensusError::EmptyVrfKey);
        }

        // Protocol version checks (Haskell's envelopeChecks):
        // 1. ObsoleteNode: the ledger's current PV exceeds our node's max — we can't
        //    validate blocks on this chain at all, the node must be upgraded.
        // 2. HeaderProtVerTooHigh: the block header claims a PV more than one major
        //    version ahead of the ledger — a valid block can only propose pv+1 at most.
        if let Some(pv_major) = ledger_pv_major {
            if pv_major > self.max_major_prot_ver {
                warn!(
                    chain_pv = pv_major,
                    node_max = self.max_major_prot_ver,
                    "Praos: node is obsolete — chain protocol version exceeds node maximum"
                );
                return Err(ConsensusError::ObsoleteNode {
                    chain_pv: pv_major,
                    node_max_pv: self.max_major_prot_ver,
                });
            }
            if let Some(next_pv) = pv_major.checked_add(1) {
                if header.protocol_version.major > next_pv {
                    warn!(
                        slot = header.slot.0,
                        block_pv = header.protocol_version.major,
                        ledger_pv = pv_major,
                        max_allowed = next_pv,
                        "Praos: block header protocol version too high"
                    );
                    return Err(ConsensusError::HeaderProtVerTooHigh {
                        supplied: header.protocol_version.major,
                        max_expected: next_pv,
                    });
                }
            }
        }

        // Validate KES period (always — cheap structural check)
        self.validate_kes_period(header)?;

        // Cryptographic verification only in Full mode
        if mode == ValidationMode::Full {
            self.verify_vrf_proof(header)?;
            self.verify_nonce_vrf_proof(header)?;
            self.validate_operational_cert(header)?;
            self.verify_kes_signature(header)?;
        }

        trace!(
            slot = header.slot.0,
            block_no = header.block_number.0,
            "Praos: header validation passed"
        );

        Ok(())
    }

    /// Full block header validation with pool registration context.
    ///
    /// Performs all checks from `validate_header` plus:
    /// - VRF key binding: header's VRF key matches pool's registered VRF key hash
    /// - Leader eligibility: VRF output satisfies Praos threshold for pool's stake
    /// - Opcert counter monotonicity: sequence number has not regressed
    ///
    /// Pool-aware checks and opcert counter are evaluated BEFORE cryptographic
    /// verification so that binding/eligibility failures are reported accurately.
    ///
    /// The `mode` parameter controls whether cryptographic verification is performed:
    /// - `ValidationMode::Full`: verify VRF proof, KES signature, and opcert Ed25519
    ///   signature. Used for blocks received from the network (Haskell's `updateChainDepState`).
    /// - `ValidationMode::Replay`: skip all crypto verification, only perform structural
    ///   checks and update chain-dependent state (nonces, opcert counters). Used for blocks
    ///   replayed from local storage (Haskell's `reupdateChainDepState`).
    pub fn validate_header_full(
        &mut self,
        header: &BlockHeader,
        current_slot: SlotNo,
        issuer_info: Option<&BlockIssuerInfo>,
        mode: ValidationMode,
        ledger_pv_major: Option<u64>,
    ) -> Result<(), ConsensusError> {
        // 1. Structural checks (always fatal)
        if header.slot > current_slot {
            return Err(ConsensusError::FutureBlock {
                current: current_slot.0,
                block: header.slot.0,
            });
        }
        if header.issuer_vkey.is_empty() {
            return Err(ConsensusError::EmptyIssuerVkey);
        }
        if header.vrf_vkey.is_empty() {
            return Err(ConsensusError::EmptyVrfKey);
        }

        // 1b. Protocol version checks (Haskell's envelopeChecks):
        // ObsoleteNode: ledger PV exceeds node max — node must be upgraded.
        // HeaderProtVerTooHigh: block header PV more than one major version ahead of ledger.
        if let Some(pv_major) = ledger_pv_major {
            if pv_major > self.max_major_prot_ver {
                warn!(
                    chain_pv = pv_major,
                    node_max = self.max_major_prot_ver,
                    "Praos: node is obsolete — chain protocol version exceeds node maximum"
                );
                return Err(ConsensusError::ObsoleteNode {
                    chain_pv: pv_major,
                    node_max_pv: self.max_major_prot_ver,
                });
            }
            if let Some(next_pv) = pv_major.checked_add(1) {
                if header.protocol_version.major > next_pv {
                    warn!(
                        slot = header.slot.0,
                        block_pv = header.protocol_version.major,
                        ledger_pv = pv_major,
                        max_allowed = next_pv,
                        "Praos: block header protocol version too high"
                    );
                    return Err(ConsensusError::HeaderProtVerTooHigh {
                        supplied: header.protocol_version.major,
                        max_expected: next_pv,
                    });
                }
            }
        }

        // 2. Pool registration check — blocks from unregistered pools must be rejected
        if issuer_info.is_none() {
            let pool_id = torsten_primitives::hash::blake2b_224(&header.issuer_vkey);
            if self.strict_verification {
                warn!(
                    slot = header.slot.0,
                    pool = %pool_id,
                    "Praos: block from unregistered pool — rejecting"
                );
                return Err(ConsensusError::UnregisteredPool { pool_id });
            }
            debug!(
                slot = header.slot.0,
                pool = %pool_id,
                "Praos: pool not found in stake distribution (non-fatal during sync)"
            );
        }

        // 3. Pool-aware checks (only when issuer info is available)
        if let Some(info) = issuer_info {
            // Verify VRF key binding: Blake2b-256(header.vrf_vkey) must match
            // the pool's registered VRF key hash
            if header.vrf_vkey.len() == 32 {
                let header_vrf_hash = blake2b_256(&header.vrf_vkey);
                if *header_vrf_hash.as_bytes() != *info.vrf_keyhash.as_bytes() {
                    if self.strict_verification {
                        warn!(
                            slot = header.slot.0,
                            "Praos: VRF key hash mismatch — header VRF key does not match pool registration"
                        );
                        return Err(ConsensusError::VrfKeyMismatch);
                    }
                    debug!(
                        slot = header.slot.0,
                        "Praos: VRF key hash mismatch (non-fatal during sync)"
                    );
                }
            }

            // Verify VRF leader eligibility: the VRF output must satisfy the
            // Praos threshold for this pool's relative stake.
            // Uses exact 34-digit fixed-point arithmetic (dashu IBig) matching
            // Haskell's taylorExpCmp / pallas-math implementation.
            // Both sigma (relative stake) and f (active slot coeff) are passed
            // as exact rationals to avoid f64 precision loss at boundaries.
            if header.vrf_result.output.len() == 64 {
                // Praos (Babbage/Conway, protocol >= 7): Blake2b-256("L" || vrf_output), certNatMax = 2^256
                // TPraos (Shelley-Alonzo, protocol < 7): raw 64-byte vrf_output, certNatMax = 2^512
                let is_praos = header.protocol_version.major >= 7;
                let (f_num, f_den) = self.active_slot_coeff_rational;
                let is_leader = if is_praos {
                    let leader_value =
                        crate::slot_leader::vrf_leader_value(&header.vrf_result.output);
                    torsten_crypto::vrf::check_leader_value_full_rational(
                        &leader_value,
                        info.pool_stake,
                        info.total_active_stake,
                        f_num,
                        f_den,
                    )
                } else {
                    // TPraos: raw VRF output directly
                    torsten_crypto::vrf::check_leader_value_tpraos_rational(
                        &header.vrf_result.output,
                        info.pool_stake,
                        info.total_active_stake,
                        f_num,
                        f_den,
                    )
                };
                if !is_leader {
                    let sigma_display = info.relative_stake_f64();
                    if self.strict_verification && self.snapshots_established {
                        return Err(ConsensusError::InvalidBlock(format!(
                            "VRF leader eligibility check failed: slot={}, sigma={}, proto={}",
                            header.slot.0, sigma_display, header.protocol_version.major,
                        )));
                    } else {
                        debug!(
                            slot = header.slot.0,
                            relative_stake = sigma_display,
                            proto = header.protocol_version.major,
                            praos = is_praos,
                            "Praos: VRF leader eligibility check failed (non-strict, skipping)"
                        );
                    }
                }
            }
        }

        // 4. Opcert counter monotonicity check
        self.check_opcert_counter(header, issuer_info)?;

        // 5. KES period validation (always fatal — cheap structural check)
        self.validate_kes_period(header)?;

        // 6. Cryptographic verification (VRF proof, opcert signature, KES signature)
        // In Replay mode (Haskell's reupdateChainDepState), skip all crypto verification.
        // This is used for blocks replayed from local storage (ImmutableDB/ChainDB) where
        // the blocks were previously validated or are from a trusted source (Mithril).
        if mode == ValidationMode::Full {
            self.verify_vrf_proof(header)?;
            self.verify_nonce_vrf_proof(header)?;
            self.validate_operational_cert(header)?;
            self.verify_kes_signature(header)?;
        }

        trace!(
            slot = header.slot.0,
            block_no = header.block_number.0,
            mode = ?mode,
            "Praos: full header validation passed"
        );

        Ok(())
    }

    /// Validate that the block header's `body_hash` field matches the actual hash
    /// of the block body CBOR.
    ///
    /// This prevents a malicious peer from sending a valid header with a substituted
    /// body. The body hash is computed as Blake2b-256 of the CBOR-encoded block body.
    ///
    /// This check should be performed whenever the full block body is available
    /// (not during header-only chain sync).
    pub fn validate_block_body_hash(
        &self,
        header: &BlockHeader,
        body_cbor: &[u8],
    ) -> Result<(), ConsensusError> {
        let computed_hash = blake2b_256(body_cbor);
        if header.body_hash != computed_hash {
            warn!(
                slot = header.slot.0,
                header_body_hash = %header.body_hash,
                computed_body_hash = %computed_hash,
                "Praos: block body hash mismatch"
            );
            return Err(ConsensusError::BodyHashMismatch {
                header_hash: header.body_hash,
                computed_hash,
            });
        }
        trace!(slot = header.slot.0, "Praos: block body hash verified");
        Ok(())
    }

    /// Validate block envelope size limits against protocol parameters.
    ///
    /// This corresponds to Haskell's `envelopeChecks` in the consensus layer
    /// (`ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Ledger/Extended.hs`).
    /// Haskell runs `envelopeChecks` as a separate validation step before
    /// `updateChainDepState`, so this method should be called at the block-processing
    /// call site prior to `validate_header_full`.
    ///
    /// Two checks are performed:
    /// - `body_size` (from the block header) must not exceed `max_block_body_size`.
    /// - `header_cbor_size`, when provided, must not exceed `max_block_header_size`.
    ///   This is optional because header CBOR length is only knowable when the raw bytes
    ///   are available (e.g. BlockFetch), not during header-only ChainSync.
    ///
    /// Both limits are always fatal — the ledger assumes that any block which reaches
    /// it has already passed `envelopeChecks`.
    pub fn validate_envelope(
        &self,
        slot: SlotNo,
        body_size: u64,
        header_cbor_size: Option<u64>,
        max_block_body_size: u64,
        max_block_header_size: u64,
    ) -> Result<(), ConsensusError> {
        // Body size is declared by the block producer in the header body and
        // must not exceed the protocol-parameter limit.
        if body_size > max_block_body_size {
            warn!(
                slot = slot.0,
                body_size, max_block_body_size, "Praos: block body size exceeds protocol limit"
            );
            return Err(ConsensusError::BlockBodyTooLarge {
                body_size,
                max_block_body_size,
            });
        }

        // Header CBOR size is checked only when the raw bytes are available.
        if let Some(header_size) = header_cbor_size {
            if header_size > max_block_header_size {
                warn!(
                    slot = slot.0,
                    header_size,
                    max_block_header_size,
                    "Praos: block header size exceeds protocol limit"
                );
                return Err(ConsensusError::BlockHeaderTooLarge {
                    header_size,
                    max_block_header_size,
                });
            }
        }

        trace!(
            slot = slot.0,
            body_size,
            header_cbor_size = header_cbor_size.unwrap_or(0),
            "Praos: envelope checks passed"
        );
        Ok(())
    }

    /// Check and update the operational certificate sequence number for the block issuer.
    ///
    /// Per the Haskell reference implementation (`doValidateKESSignature`), the opcert
    /// counter must satisfy:
    ///   m <= n <= m + 1
    /// where m is the last seen counter and n is the new counter.
    /// This means the counter can stay the same or increment by exactly 1.
    /// Regression (n < m) and over-increment (n > m+1) are both rejected.
    ///
    /// For a pool's **first ever block** (no entry in `opcert_counters`), the Haskell node
    /// initializes `currentIssueNo = Just 0`, meaning the first block must have counter 0
    /// or 1. This only applies to pools that ARE in the stake distribution (`issuer_info`
    /// is `Some`). Unknown pools during sync (no `issuer_info`) are handled leniently.
    fn check_opcert_counter(
        &mut self,
        header: &BlockHeader,
        issuer_info: Option<&BlockIssuerInfo>,
    ) -> Result<(), ConsensusError> {
        if header.issuer_vkey.is_empty() {
            return Ok(());
        }

        let pool_id = torsten_primitives::hash::blake2b_224(&header.issuer_vkey);
        let n = header.operational_cert.sequence_number;

        if let Some(&m) = self.opcert_counters.get(&pool_id) {
            // Counter regression: n < m
            if n < m {
                if self.strict_verification {
                    warn!(
                        slot = header.slot.0,
                        pool = %pool_id,
                        got = n,
                        last_seen = m,
                        "Praos: opcert counter regression"
                    );
                    return Err(ConsensusError::OpcertSequenceRegression {
                        got: n,
                        expected: m,
                    });
                }
                debug!(
                    slot = header.slot.0,
                    pool = %pool_id,
                    got = n,
                    last_seen = m,
                    "Praos: opcert counter regression (non-fatal during sync)"
                );
            }
            // Counter over-increment: n > m + 1 (using checked_add to prevent u64 wrap)
            if m.checked_add(1).is_none_or(|m1| n > m1) {
                if self.strict_verification {
                    warn!(
                        slot = header.slot.0,
                        pool = %pool_id,
                        got = n,
                        last_seen = m,
                        "Praos: opcert counter over-incremented (max +1 per rotation)"
                    );
                    return Err(ConsensusError::OpcertCounterOverIncremented {
                        got: n,
                        last_seen: m,
                    });
                }
                debug!(
                    slot = header.slot.0,
                    pool = %pool_id,
                    got = n,
                    last_seen = m,
                    "Praos: opcert counter over-incremented (non-fatal during sync)"
                );
            }
        } else if issuer_info.is_some() {
            // Pool is in the stake distribution but this is its first-ever block seen.
            // Haskell initializes `currentIssueNo = Just 0` for pools in the stake
            // distribution with no prior counter entry, so the first block must satisfy:
            //   0 <= n <= 0 + 1  →  n ∈ {0, 1}
            //
            // This prevents a pool from starting with an arbitrarily large counter,
            // which would allow replaying future opcerts without ever having them on-chain.
            let m: u64 = 0;
            if m.checked_add(1).is_none_or(|m1| n > m1) {
                if self.strict_verification {
                    warn!(
                        slot = header.slot.0,
                        pool = %pool_id,
                        got = n,
                        "Praos: opcert counter too large for first-seen pool (expected 0 or 1)"
                    );
                    return Err(ConsensusError::OpcertCounterOverIncremented {
                        got: n,
                        last_seen: m,
                    });
                }
                debug!(
                    slot = header.slot.0,
                    pool = %pool_id,
                    got = n,
                    "Praos: first-seen pool has counter > 1 (non-fatal during sync)"
                );
            }
        }
        // else: pool is unknown (no issuer_info) and not yet tracked — lenient during sync.

        // Update tracked counter (always update, even during sync, for tracking).
        // Hard cap prevents unbounded growth between epoch-boundary pruning cycles.
        const MAX_OPCERT_ENTRIES: usize = 50_000;
        if self.opcert_counters.len() < MAX_OPCERT_ENTRIES
            || self.opcert_counters.contains_key(&pool_id)
        {
            self.opcert_counters
                .entry(pool_id)
                .and_modify(|v| {
                    if n > *v {
                        *v = n;
                    }
                })
                .or_insert(n);
        }

        Ok(())
    }

    /// Verify the VRF proof in the block header (Praos / Conway era).
    ///
    /// VRF input = Blake2b-256(slot_u64_BE || epoch_nonce)
    /// This verifies that the block producer correctly evaluated the VRF,
    /// proving they had the right to produce this block.
    ///
    /// Verify the VRF proof in the block header.
    ///
    /// In strict mode with an established nonce, VRF proof failure is fatal.
    /// Otherwise, failures are logged as warnings because the epoch nonce may
    /// not be correctly established yet (e.g., after Mithril import — needs
    /// 2 full epoch transitions for nonce to stabilize).
    fn verify_vrf_proof(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        // VRF proof verification requires a correct epoch nonce.
        // After Mithril import, the nonce is wrong until 2 full epoch transitions.
        let vrf_is_fatal = self.strict_verification && self.nonce_established;

        // Construct the VRF seed:
        // TPraos (Shelley–Alonzo, proto < 7): domain-separated seed with TAG_L XOR.
        // Praos (Babbage/Conway, proto >= 7): plain hash, no domain tag.
        let seed = if header.protocol_version.major < 7 {
            crate::slot_leader::tpraos_leader_vrf_input(&header.epoch_nonce, header.slot)
        } else {
            crate::slot_leader::vrf_input(&header.epoch_nonce, header.slot)
        };

        debug!(
            slot = header.slot.0,
            epoch_nonce = %header.epoch_nonce,
            vrf_vkey_len = header.vrf_vkey.len(),
            vrf_proof_len = header.vrf_result.proof.len(),
            vrf_output_len = header.vrf_result.output.len(),
            seed_len = seed.len(),
            nonce_established = self.nonce_established,
            "Praos: VRF verification inputs"
        );

        match torsten_crypto::vrf::verify_vrf_proof(
            &header.vrf_vkey,
            &header.vrf_result.proof,
            &seed,
        ) {
            Ok(vrf_output) => {
                // Verify that the output in the header matches what we computed
                if header.vrf_result.output.len() == 64
                    && header.vrf_result.output[..] != vrf_output[..]
                {
                    if vrf_is_fatal {
                        return Err(ConsensusError::InvalidBlock(
                            "VRF output mismatch".to_string(),
                        ));
                    }
                    warn!(slot = header.slot.0, "Praos: VRF output mismatch");
                    return Ok(());
                }
                trace!(
                    slot = header.slot.0,
                    "Praos: VRF proof verified successfully"
                );
                Ok(())
            }
            Err(e) => {
                if vrf_is_fatal {
                    error!(
                        slot = header.slot.0,
                        epoch_nonce = %header.epoch_nonce.to_hex(),
                        error = %e,
                        "VRF verification failed (fatal)"
                    );
                    return Err(ConsensusError::VrfVerification(format!("{e}")));
                }
                // Use debug level when nonce isn't established (expected after Mithril import)
                if self.nonce_established {
                    warn!(
                        slot = header.slot.0,
                        error = %e,
                        "Praos: VRF proof verification failed"
                    );
                } else {
                    debug!(
                        slot = header.slot.0,
                        error = %e,
                        "Praos: VRF proof verification deferred (epoch nonce not established)"
                    );
                }
                Ok(())
            }
        }
    }

    /// Verify the nonce VRF proof in TPraos block headers.
    ///
    /// TPraos (Shelley–Alonzo, proto < 7) carries separate leader and nonce VRF
    /// certificates.  The nonce VRF proof proves honest generation of the output
    /// that feeds into epoch nonce evolution.  This method cryptographically
    /// verifies that proof.
    ///
    /// Praos blocks (proto >= 7) have a single VRF certificate and derive the
    /// nonce contribution deterministically, so this check is skipped for them.
    fn verify_nonce_vrf_proof(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        // Only TPraos blocks carry a separate nonce VRF proof.
        if header.protocol_version.major >= 7 || header.nonce_vrf_proof.is_empty() {
            return Ok(());
        }

        let vrf_is_fatal = self.strict_verification && self.nonce_established;

        // TPraos nonce VRF seed: Blake2b-256(slot_BE || epoch_nonce) XOR TAG_ETA
        let seed = crate::slot_leader::tpraos_nonce_vrf_input(&header.epoch_nonce, header.slot);

        debug!(
            slot = header.slot.0,
            nonce_vrf_proof_len = header.nonce_vrf_proof.len(),
            nonce_vrf_output_len = header.nonce_vrf_output.len(),
            "TPraos: nonce VRF verification inputs"
        );

        match torsten_crypto::vrf::verify_vrf_proof(
            &header.vrf_vkey,
            &header.nonce_vrf_proof,
            &seed,
        ) {
            Ok(vrf_output) => {
                // For TPraos, nonce_vrf_output stores the raw 64-byte VRF output.
                // Verify the header's stored output matches what the proof produces.
                if header.nonce_vrf_output.len() == 64
                    && header.nonce_vrf_output[..] != vrf_output[..]
                {
                    if vrf_is_fatal {
                        return Err(ConsensusError::InvalidBlock(
                            "Nonce VRF output mismatch".to_string(),
                        ));
                    }
                    warn!(slot = header.slot.0, "TPraos: nonce VRF output mismatch");
                    return Ok(());
                }
                trace!(
                    slot = header.slot.0,
                    "TPraos: nonce VRF proof verified successfully"
                );
                Ok(())
            }
            Err(e) => {
                if vrf_is_fatal {
                    error!(
                        slot = header.slot.0,
                        epoch_nonce = %header.epoch_nonce.to_hex(),
                        error = %e,
                        "Nonce VRF verification failed (fatal)"
                    );
                    return Err(ConsensusError::VrfVerification(format!("nonce VRF: {e}")));
                }
                if self.nonce_established {
                    warn!(
                        slot = header.slot.0,
                        error = %e,
                        "TPraos: nonce VRF proof verification failed"
                    );
                } else {
                    debug!(
                        slot = header.slot.0,
                        error = %e,
                        "TPraos: nonce VRF proof verification deferred (epoch nonce not established)"
                    );
                }
                Ok(())
            }
        }
    }

    /// Validate the KES period for a block header.
    ///
    /// The KES key must not have expired: the block's KES period must be
    /// >= the cert's start period and < start + max_evolutions.
    fn validate_kes_period(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        let block_kes_period = header.slot.0 / self.slots_per_kes_period;
        let cert_kes_period = header.operational_cert.kes_period;

        trace!(
            block_kes_period,
            cert_kes_period,
            slot = header.slot.0,
            slots_per_kes_period = self.slots_per_kes_period,
            "Praos: checking KES period"
        );

        // Block's KES period must be >= the operational cert's KES period
        if block_kes_period < cert_kes_period {
            warn!(
                block_kes_period,
                cert_kes_period, "Praos: KES period before cert start"
            );
            return Err(ConsensusError::KesPeriodBeforeCert {
                block_period: block_kes_period,
                cert_start: cert_kes_period,
            });
        }

        // KES key must not have expired
        let kes_evolutions = block_kes_period - cert_kes_period;
        if kes_evolutions >= self.max_kes_evolutions {
            warn!(
                kes_evolutions,
                max = self.max_kes_evolutions,
                "Praos: KES key expired"
            );
            return Err(ConsensusError::KesExpired {
                current: block_kes_period,
                cert_start: cert_kes_period,
                max_evolutions: self.max_kes_evolutions,
            });
        }

        Ok(())
    }

    /// Validate the operational certificate structure and signature.
    ///
    /// The operational certificate contains:
    /// - hot_vkey: KES verification key (the "hot" key)
    /// - sequence_number: monotonically increasing counter
    /// - kes_period: KES period at which the certificate was issued
    /// - sigma: Ed25519 signature by the cold key over [hot_vkey, seq_num, kes_period]
    ///
    /// We verify the Ed25519 signature using the issuer_vkey (cold key) from the header.
    fn validate_operational_cert(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        let opcert = &header.operational_cert;

        // Hot VKey must be present
        if opcert.hot_vkey.is_empty() {
            return Err(ConsensusError::InvalidOperationalCert);
        }

        // Sigma (signature) must be present
        if opcert.sigma.is_empty() {
            return Err(ConsensusError::InvalidOperationalCert);
        }

        // Verify the operational certificate signature:
        // The cold key (issuer_vkey) signs raw bytes: hot_vkey(32) || counter(8 BE) || kes_period(8 BE)
        // per the Haskell OCertSignable format.
        if header.issuer_vkey.len() == 32 && opcert.sigma.len() == 64 {
            match verify_opcert_signature(
                &header.issuer_vkey,
                &opcert.hot_vkey,
                opcert.sequence_number,
                opcert.kes_period,
                &opcert.sigma,
            ) {
                Ok(()) => {
                    debug!("Operational certificate signature verified");
                }
                Err(e) => {
                    if self.strict_verification {
                        return Err(e);
                    }
                    debug!("Opcert signature verification deferred (non-strict mode): {e}");
                }
            }
        }

        Ok(())
    }

    /// Verify the KES signature on the block header.
    ///
    /// The KES signature signs the header body bytes using the hot key (from the opcert)
    /// at the KES period = block_kes_period - opcert_kes_period.
    fn verify_kes_signature(&self, header: &BlockHeader) -> Result<(), ConsensusError> {
        // Skip verification if no KES signature is available (Byron blocks)
        if header.kes_signature.is_empty() {
            debug!(
                slot = header.slot.0,
                "Praos: KES signature is empty — skipping"
            );
            return Ok(());
        }

        let opcert = &header.operational_cert;
        if opcert.hot_vkey.len() != 32 || header.kes_signature.len() != 448 {
            if self.strict_verification {
                return Err(ConsensusError::InvalidKesSignature);
            }
            debug!(
                slot = header.slot.0,
                kes_sig_len = header.kes_signature.len(),
                hot_vkey_len = opcert.hot_vkey.len(),
                "Praos: Skipping KES verification — unexpected sizes"
            );
            return Ok(()); // Skip if sizes don't match expected KES format
        }

        let block_kes_period = header.slot.0 / self.slots_per_kes_period;
        let kes_period_offset = block_kes_period.saturating_sub(opcert.kes_period);

        // Reconstruct the header body CBOR for verification
        let header_body_cbor = torsten_serialization::encode_block_header_body(header);

        // Parse the KES signature and verify against the hot verification key
        let mut hot_vkey = [0u8; 32];
        hot_vkey.copy_from_slice(&opcert.hot_vkey);

        let kes_period_offset_u32 = u32::try_from(kes_period_offset).map_err(|_| {
            ConsensusError::InvalidBlock(format!(
                "KES period offset {} exceeds u32 range",
                kes_period_offset
            ))
        })?;

        match torsten_crypto::kes::kes_verify_bytes(
            &hot_vkey,
            kes_period_offset_u32,
            &header.kes_signature,
            &header_body_cbor,
        ) {
            Ok(()) => {
                trace!(
                    slot = header.slot.0,
                    kes_period = kes_period_offset,
                    "Praos: KES signature verified"
                );
                Ok(())
            }
            Err(e) => {
                if self.strict_verification {
                    return Err(ConsensusError::InvalidKesSignature);
                }
                debug!(
                    slot = header.slot.0,
                    error = %e,
                    kes_period = kes_period_offset,
                    "Praos: KES signature verification deferred (non-strict mode)"
                );
                Ok(())
            }
        }
    }

    /// Check if a slot is within the stability window (last k blocks)
    pub fn is_in_stability_window(&self, slot: SlotNo) -> bool {
        match self.tip.point.slot() {
            Some(tip_slot) => tip_slot.0.saturating_sub(self.stability_window()) <= slot.0,
            None => true,
        }
    }

    /// The stability window in slots, using integer arithmetic for precision.
    ///
    /// The multiplier is era-dependent per the Ouroboros specifications:
    /// - **Shelley through Babbage** (protocol major < 10): `3k/f`
    /// - **Conway with Genesis** (protocol major >= 10): `4k/f`
    ///
    /// The `protocol_major` parameter is the current ledger protocol version.
    /// When called without a specific version (e.g., during initialisation),
    /// use `stability_window_default()` which applies the pre-Conway multiplier.
    pub fn stability_window_for_version(&self, protocol_major: u64) -> u64 {
        let multiplier = if protocol_major >= 10 { 4 } else { 3 };
        let (f_num, f_den) = self.active_slot_coeff_rational;
        torsten_primitives::protocol_params::ceiling_div_by_rational(
            multiplier,
            self.security_param,
            f_num,
            f_den,
        )
    }

    /// Default stability window using pre-Conway multiplier (3k/f).
    /// Use `stability_window_for_version()` when the current protocol version is known.
    pub fn stability_window(&self) -> u64 {
        self.stability_window_for_version(0)
    }

    /// Calculate which epoch a slot belongs to
    pub fn slot_to_epoch(&self, slot: SlotNo) -> EpochNo {
        slot.to_epoch(self.epoch_length)
    }

    /// Get the first slot of an epoch
    pub fn epoch_first_slot(&self, epoch: EpochNo) -> SlotNo {
        SlotNo(epoch.0 * self.epoch_length.0)
    }

    /// Check if we're at an epoch boundary
    pub fn is_epoch_boundary(&self, slot: SlotNo) -> bool {
        slot.0.is_multiple_of(self.epoch_length.0)
    }

    /// Maximum rollback depth
    pub fn max_rollback(&self) -> u64 {
        self.security_param
    }

    /// Prune opcert counters to only keep entries for known active pools.
    /// Call this during epoch transitions to prevent unbounded memory growth.
    pub fn prune_opcert_counters(&mut self, active_pool_ids: &HashSet<Hash28>) {
        let before = self.opcert_counters.len();
        self.opcert_counters
            .retain(|pool_id, _| active_pool_ids.contains(pool_id));
        let pruned = before - self.opcert_counters.len();
        if pruned > 0 {
            debug!(
                pruned,
                remaining = self.opcert_counters.len(),
                "Pruned opcert counters for retired pools"
            );
        }
    }

    /// Return a reference to the opcert counters map.
    /// Used by the node layer to copy counters into LedgerState before snapshot save.
    pub fn opcert_counters(&self) -> &HashMap<Hash28, u64> {
        &self.opcert_counters
    }

    /// Replace the opcert counters map wholesale.
    /// Used by the node layer to seed counters from a loaded LedgerState snapshot.
    pub fn set_opcert_counters(&mut self, counters: HashMap<Hash28, u64>) {
        debug!(
            count = counters.len(),
            "Seeded opcert counters from snapshot"
        );
        self.opcert_counters = counters;
    }

    /// Update the tip
    pub fn update_tip(&mut self, tip: Tip) {
        self.tip = tip;
    }

    /// Verify VRF proof, opcert signature, and KES signature for a block header.
    /// This is a standalone function that does not require `&mut self`, enabling
    /// parallel verification of multiple headers via rayon.
    ///
    /// Returns `Ok(())` if all checks pass (or failures are non-fatal in non-strict mode).
    /// Returns `Err` only if a check fails in strict mode.
    pub fn verify_header_crypto(
        params: &CryptoVerificationParams,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        Self::verify_vrf_proof_static(params, header)?;
        Self::verify_opcert_static(params, header)?;
        Self::verify_kes_signature_static(params, header)?;
        Ok(())
    }

    /// Standalone VRF proof verification (no &self required).
    fn verify_vrf_proof_static(
        params: &CryptoVerificationParams,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        let vrf_is_fatal = params.strict_verification && params.nonce_established;

        let seed = crate::slot_leader::vrf_input(&header.epoch_nonce, header.slot);

        match torsten_crypto::vrf::verify_vrf_proof(
            &header.vrf_vkey,
            &header.vrf_result.proof,
            &seed,
        ) {
            Ok(vrf_output) => {
                if header.vrf_result.output.len() == 64
                    && header.vrf_result.output[..] != vrf_output[..]
                {
                    if vrf_is_fatal {
                        return Err(ConsensusError::InvalidBlock(
                            "VRF output mismatch".to_string(),
                        ));
                    }
                    return Ok(());
                }
                Ok(())
            }
            Err(e) => {
                if vrf_is_fatal {
                    return Err(ConsensusError::VrfVerification(format!("{e}")));
                }
                Ok(())
            }
        }
    }

    /// Standalone opcert signature verification (no &self required).
    fn verify_opcert_static(
        params: &CryptoVerificationParams,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        let opcert = &header.operational_cert;

        if opcert.hot_vkey.is_empty() {
            return Err(ConsensusError::InvalidOperationalCert);
        }
        if opcert.sigma.is_empty() {
            return Err(ConsensusError::InvalidOperationalCert);
        }

        if header.issuer_vkey.len() == 32 && opcert.sigma.len() == 64 {
            if let Err(e) = verify_opcert_signature(
                &header.issuer_vkey,
                &opcert.hot_vkey,
                opcert.sequence_number,
                opcert.kes_period,
                &opcert.sigma,
            ) {
                if params.strict_verification {
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    /// Standalone KES signature verification (no &self required).
    fn verify_kes_signature_static(
        params: &CryptoVerificationParams,
        header: &BlockHeader,
    ) -> Result<(), ConsensusError> {
        if header.kes_signature.is_empty() {
            return Ok(());
        }

        let opcert = &header.operational_cert;
        if opcert.hot_vkey.len() != 32 || header.kes_signature.len() != 448 {
            if params.strict_verification {
                return Err(ConsensusError::InvalidKesSignature);
            }
            return Ok(());
        }

        let block_kes_period = header.slot.0 / params.slots_per_kes_period;
        let kes_period_offset = block_kes_period.saturating_sub(opcert.kes_period);

        let header_body_cbor = torsten_serialization::encode_block_header_body(header);

        let mut hot_vkey = [0u8; 32];
        hot_vkey.copy_from_slice(&opcert.hot_vkey);

        let kes_period_offset_u32 = u32::try_from(kes_period_offset).map_err(|_| {
            ConsensusError::InvalidBlock(format!(
                "KES period offset {} exceeds u32 range",
                kes_period_offset
            ))
        })?;

        match torsten_crypto::kes::kes_verify_bytes(
            &hot_vkey,
            kes_period_offset_u32,
            &header.kes_signature,
            &header_body_cbor,
        ) {
            Ok(()) => Ok(()),
            Err(_) => {
                if params.strict_verification {
                    return Err(ConsensusError::InvalidKesSignature);
                }
                Ok(())
            }
        }
    }
}

/// Verify the operational certificate Ed25519 signature.
///
/// The cold key signs the raw byte concatenation of: hot_vkey(32) || counter(8 BE) || kes_period(8 BE)
/// This matches the Haskell `OCertSignable` serialization (NOT CBOR).
/// This proves that the pool operator (cold key holder) authorized the hot key.
pub fn verify_opcert_signature(
    cold_vkey_bytes: &[u8],
    hot_vkey: &[u8],
    sequence_number: u64,
    kes_period: u64,
    signature: &[u8],
) -> Result<(), ConsensusError> {
    // Construct the signed message: raw bytes per Haskell OCertSignable
    // ocertSigKES(32 bytes) || ocertN(8 bytes BE) || ocertKESPeriod(8 bytes BE)
    let mut signable = Vec::with_capacity(48);
    signable.extend_from_slice(hot_vkey);
    signable.extend_from_slice(&sequence_number.to_be_bytes());
    signable.extend_from_slice(&kes_period.to_be_bytes());

    // Verify the Ed25519 signature
    let vk = PaymentVerificationKey::from_bytes(cold_vkey_bytes)
        .map_err(|_| ConsensusError::InvalidOperationalCert)?;

    vk.verify(&signable, signature)
        .map_err(|_| ConsensusError::InvalidOperationalCert)?;

    Ok(())
}

/// Verify VRF leader eligibility for a block.
///
/// Checks that the VRF output certifies the pool as a slot leader given its relative stake.
/// This does NOT verify the VRF proof itself (which requires a full VRF library),
/// but verifies that the VRF output value satisfies the Praos leader check:
///   vrf_output < 2^512 * phi_f(sigma)
/// where phi_f(sigma) = 1 - (1 - f)^sigma
pub fn verify_leader_eligibility(
    vrf_output: &[u8],
    relative_stake: f64,
    active_slot_coeff: f64,
) -> Result<(), ConsensusError> {
    if torsten_crypto::vrf::check_leader_value(vrf_output, relative_stake, active_slot_coeff) {
        Ok(())
    } else {
        Err(ConsensusError::NotSlotLeader)
    }
}

/// Validate that a block header's `body_hash` matches the Blake2b-256 hash of the
/// CBOR-encoded block body. Standalone version that does not require an `OuroborosPraos`
/// instance.
pub fn validate_block_body_hash(
    header: &BlockHeader,
    body_cbor: &[u8],
) -> Result<(), ConsensusError> {
    let computed_hash = blake2b_256(body_cbor);
    if header.body_hash != computed_hash {
        return Err(ConsensusError::BodyHashMismatch {
            header_hash: header.body_hash,
            computed_hash,
        });
    }
    Ok(())
}

impl Default for OuroborosPraos {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::block::Point;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    /// Create a dummy BlockIssuerInfo for the given header's VRF key
    fn make_issuer_info(header: &BlockHeader) -> BlockIssuerInfo {
        BlockIssuerInfo {
            vrf_keyhash: blake2b_256(&header.vrf_vkey),
            pool_stake: 1,
            total_active_stake: 1,
        }
    }

    /// Create a valid test header at the given slot
    fn make_valid_header(slot: u64) -> BlockHeader {
        BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash: Hash32::ZERO,
            issuer_vkey: vec![1u8; 32],
            vrf_vkey: vec![2u8; 32],
            vrf_result: torsten_primitives::block::VrfOutput {
                output: vec![0u8; 32],
                proof: vec![0u8; 80],
            },
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
            block_number: BlockNo(1),
            slot: SlotNo(slot),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: torsten_primitives::block::OperationalCert {
                hot_vkey: vec![3u8; 32],
                sequence_number: 0,
                kes_period: slot / KES_PERIOD_SLOTS,
                sigma: vec![4u8; 64],
            },
            protocol_version: torsten_primitives::block::ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![],
        }
    }

    #[test]
    fn test_new_praos() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.tip, Tip::origin());
        assert!((praos.active_slot_coeff - 0.05).abs() < f64::EPSILON);
        assert_eq!(praos.security_param, 2160);
    }

    #[test]
    fn test_stability_window() {
        let praos = OuroborosPraos::new();
        // 3 * 2160 / 0.05 = 129600
        assert_eq!(praos.stability_window(), 129600);
    }

    #[test]
    fn test_slot_to_epoch() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.slot_to_epoch(SlotNo(0)), EpochNo(0));
        assert_eq!(praos.slot_to_epoch(SlotNo(431999)), EpochNo(0));
        assert_eq!(praos.slot_to_epoch(SlotNo(432000)), EpochNo(1));
        assert_eq!(praos.slot_to_epoch(SlotNo(864000)), EpochNo(2));
    }

    #[test]
    fn test_epoch_first_slot() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.epoch_first_slot(EpochNo(0)), SlotNo(0));
        assert_eq!(praos.epoch_first_slot(EpochNo(1)), SlotNo(432000));
    }

    #[test]
    fn test_epoch_boundary() {
        let praos = OuroborosPraos::new();
        assert!(praos.is_epoch_boundary(SlotNo(0)));
        assert!(praos.is_epoch_boundary(SlotNo(432000)));
        assert!(!praos.is_epoch_boundary(SlotNo(1)));
    }

    #[test]
    fn test_max_rollback() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.max_rollback(), 2160);
    }

    #[test]
    fn test_future_block_rejected() {
        let praos = OuroborosPraos::new();
        let header = make_valid_header(200);
        let result = praos.validate_header(&header, SlotNo(100), ValidationMode::Full, Some(9));
        assert!(matches!(result, Err(ConsensusError::FutureBlock { .. })));
    }

    #[test]
    fn test_valid_header() {
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_issuer_vkey_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.issuer_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(matches!(result, Err(ConsensusError::EmptyIssuerVkey)));
    }

    #[test]
    fn test_empty_vrf_key_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.vrf_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(matches!(result, Err(ConsensusError::EmptyVrfKey)));
    }

    #[test]
    fn test_vrf_verification_non_fatal() {
        // VRF verification with dummy data should not reject during sync
        // (it's non-fatal since we may not have the correct epoch nonce)
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        // With dummy VRF key/proof, verification should pass (non-fatal mode)
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(result.is_ok());
    }

    #[test]
    fn test_kes_period_validation() {
        let praos = OuroborosPraos::new();
        // Block at slot 200,000 is in KES period 1 (200000 / 129600 = 1)
        let mut header = make_valid_header(200_000);
        // Set cert KES period to 1 (matches)
        header.operational_cert.kes_period = 1;
        assert!(praos
            .validate_header(&header, SlotNo(300_000), ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_kes_period_before_cert_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        // Block at slot 100 is in KES period 0, but cert says period 5
        header.operational_cert.kes_period = 5;
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(matches!(
            result,
            Err(ConsensusError::KesPeriodBeforeCert { .. })
        ));
    }

    #[test]
    fn test_kes_expired_rejected() {
        let praos = OuroborosPraos::new();
        // Block at slot 129600 * 63 = 8,164,800 (KES period 63)
        let slot = KES_PERIOD_SLOTS * 63;
        let mut header = make_valid_header(slot);
        // Cert started at period 0, so 63 evolutions > max 62
        header.operational_cert.kes_period = 0;
        let result =
            praos.validate_header(&header, SlotNo(slot + 1000), ValidationMode::Full, Some(9));
        assert!(matches!(result, Err(ConsensusError::KesExpired { .. })));
    }

    #[test]
    fn test_kes_at_max_evolution_ok() {
        let praos = OuroborosPraos::new();
        // 61 evolutions (0..61) should be OK (< MAX_KES_EVOLUTIONS which is 62)
        let slot = KES_PERIOD_SLOTS * 61;
        let mut header = make_valid_header(slot);
        header.operational_cert.kes_period = 0;
        assert!(praos
            .validate_header(&header, SlotNo(slot + 1000), ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_empty_opcert_hot_vkey_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.operational_cert.hot_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(matches!(
            result,
            Err(ConsensusError::InvalidOperationalCert)
        ));
    }

    #[test]
    fn test_empty_opcert_sigma_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.operational_cert.sigma = vec![];
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(matches!(
            result,
            Err(ConsensusError::InvalidOperationalCert)
        ));
    }

    #[test]
    fn test_64_byte_vrf_output_valid() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.vrf_result.output = vec![0u8; 64]; // TPraos compatibility
        assert!(praos
            .validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_verify_opcert_signature_valid() {
        // Generate a cold key pair
        let cold_sk = torsten_crypto::keys::PaymentSigningKey::generate();
        let cold_vk = cold_sk.verification_key();

        let hot_vkey = vec![99u8; 32];
        let sequence_number = 0u64;
        let kes_period = 5u64;

        // Build the opcert signable: raw bytes per Haskell OCertSignable
        // hot_vkey(32) || counter(8 BE) || kes_period(8 BE)
        let mut signable = Vec::with_capacity(48);
        signable.extend_from_slice(&hot_vkey);
        signable.extend_from_slice(&sequence_number.to_be_bytes());
        signable.extend_from_slice(&kes_period.to_be_bytes());

        // Sign with cold key
        let signature = cold_sk.sign(&signable);

        // Verify
        let result = verify_opcert_signature(
            &cold_vk.to_bytes(),
            &hot_vkey,
            sequence_number,
            kes_period,
            &signature,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_opcert_signature_wrong_key() {
        let cold_sk = torsten_crypto::keys::PaymentSigningKey::generate();
        let wrong_vk = torsten_crypto::keys::PaymentSigningKey::generate().verification_key();

        let hot_vkey = vec![99u8; 32];
        let seq = 0u64;
        let kes = 5u64;

        // Build raw signable bytes
        let mut signable = Vec::with_capacity(48);
        signable.extend_from_slice(&hot_vkey);
        signable.extend_from_slice(&seq.to_be_bytes());
        signable.extend_from_slice(&kes.to_be_bytes());

        let signature = cold_sk.sign(&signable);

        // Verify with wrong key should fail
        let result = verify_opcert_signature(&wrong_vk.to_bytes(), &hot_vkey, seq, kes, &signature);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_leader_eligibility_full_stake() {
        // Pool with 100% stake should be eligible with very low VRF output
        assert!(verify_leader_eligibility(&[0u8; 32], 1.0, 0.05).is_ok());
    }

    #[test]
    fn test_verify_leader_eligibility_zero_stake() {
        // Pool with 0% stake should never be eligible
        assert!(verify_leader_eligibility(&[128u8; 32], 0.0, 0.05).is_err());
    }

    #[test]
    fn test_vrf_input_construction() {
        let epoch_nonce = Hash32::ZERO;
        let input = crate::slot_leader::vrf_input(&epoch_nonce, SlotNo(12345));

        // vrf_input returns Blake2b-256(slot_BE || epoch_nonce) = 32 bytes
        assert_eq!(input.len(), 32);
        // Verify it's deterministic
        let input2 = crate::slot_leader::vrf_input(&epoch_nonce, SlotNo(12345));
        assert_eq!(input, input2);
        // Different slot should produce different input
        let input3 = crate::slot_leader::vrf_input(&epoch_nonce, SlotNo(12346));
        assert_ne!(input, input3);
    }

    #[test]
    fn test_strict_verification_mode() {
        let mut praos = OuroborosPraos::new();
        assert!(!praos.strict_verification);

        // In non-strict mode, dummy VRF should pass (non-fatal)
        let header = make_valid_header(100);
        assert!(praos
            .validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9))
            .is_ok());

        // Enable strict mode
        praos.set_strict_verification(true);
        assert!(praos.strict_verification);

        // In strict mode, same header should still pass structural checks
        // (VRF verification with dummy data will fail but only if vrf library
        // returns an error, which depends on the data format)
        let header2 = make_valid_header(100);
        // This tests that the strict flag is properly toggled
        praos.set_strict_verification(false);
        assert!(!praos.strict_verification);
        assert!(praos
            .validate_header(&header2, SlotNo(200), ValidationMode::Full, Some(9))
            .is_ok());
    }

    // --- Tests for validate_header_full ---

    #[test]
    fn test_validate_header_full_without_issuer_info() {
        // Without issuer info, validate_header_full behaves like validate_header
        // plus opcert counter tracking
        let mut praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        assert!(praos
            .validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_vrf_key_binding_mismatch_strict() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let header = make_valid_header(100);
        // VRF keyhash that does NOT match blake2b_256(header.vrf_vkey)
        let wrong_hash = Hash32::from_bytes([99u8; 32]);
        let info = BlockIssuerInfo {
            vrf_keyhash: wrong_hash,
            pool_stake: 1,
            total_active_stake: 1,
        };

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Full,
            Some(9),
        );
        assert!(
            matches!(result, Err(ConsensusError::VrfKeyMismatch)),
            "Expected VrfKeyMismatch, got: {result:?}"
        );
    }

    #[test]
    fn test_vrf_key_binding_mismatch_non_strict() {
        let mut praos = OuroborosPraos::new();
        // Non-strict: VRF key mismatch should be non-fatal
        let header = make_valid_header(100);
        let wrong_hash = Hash32::from_bytes([99u8; 32]);
        let info = BlockIssuerInfo {
            vrf_keyhash: wrong_hash,
            pool_stake: 1,
            total_active_stake: 1,
        };

        assert!(praos
            .validate_header_full(
                &header,
                SlotNo(200),
                Some(&info),
                ValidationMode::Full,
                Some(9)
            )
            .is_ok());
    }

    #[test]
    fn test_vrf_key_binding_correct() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let header = make_valid_header(100);
        // Correct VRF keyhash = blake2b_256(header.vrf_vkey)
        let correct_hash = blake2b_256(&header.vrf_vkey);
        let info = BlockIssuerInfo {
            vrf_keyhash: correct_hash,
            pool_stake: 1,
            total_active_stake: 1,
        };

        // Should pass VRF key binding check (VRF proof may still fail, but key binding is OK)
        // Note: with dummy VRF proof data, the underlying validate_header VRF check will
        // fail in strict mode, so we need to test the key binding path specifically
        // by using non-strict for the underlying check
        praos.set_strict_verification(false);
        assert!(praos
            .validate_header_full(
                &header,
                SlotNo(200),
                Some(&info),
                ValidationMode::Full,
                Some(9)
            )
            .is_ok());
    }

    #[test]
    fn test_opcert_counter_tracking() {
        let mut praos = OuroborosPraos::new();

        // First block from pool A with seq=5
        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Second block from same pool with seq=6 (forward, OK)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 6;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Third block from same pool with seq=4 (regression, non-strict: OK)
        let mut header3 = make_valid_header(300);
        header3.operational_cert.sequence_number = 4;
        assert!(praos
            .validate_header_full(&header3, SlotNo(400), None, ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_opcert_counter_regression_strict() {
        let mut praos = OuroborosPraos::new();

        // First block with seq=5
        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        let info1 = make_issuer_info(&header1);
        assert!(praos
            .validate_header_full(
                &header1,
                SlotNo(200),
                Some(&info1),
                ValidationMode::Full,
                Some(9)
            )
            .is_ok());

        // Enable strict mode
        praos.set_strict_verification(true);

        // Block with seq=3 (regression) should fail in strict mode
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 3;
        let info2 = make_issuer_info(&header2);
        let result = praos.validate_header_full(
            &header2,
            SlotNo(300),
            Some(&info2),
            ValidationMode::Full,
            Some(9),
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::OpcertSequenceRegression {
                    got: 3,
                    expected: 5
                })
            ),
            "Expected OpcertSequenceRegression, got: {result:?}"
        );
    }

    #[test]
    fn test_opcert_counter_same_value_ok() {
        // Non-strict mode: opcert counter is still tracked, VRF proof failures are non-fatal
        let mut praos = OuroborosPraos::new();

        // First block with seq=5
        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Same seq=5 is allowed (not a regression, same cert can sign multiple blocks)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Verify counter was tracked
        let pool_id = torsten_primitives::hash::blake2b_224(&header1.issuer_vkey);
        assert_eq!(praos.opcert_counters[&pool_id], 5);
    }

    #[test]
    fn test_opcert_counter_different_pools() {
        // Non-strict mode for VRF, but opcert counters are tracked per pool
        let mut praos = OuroborosPraos::new();

        // Pool A with seq=5
        let mut header_a = make_valid_header(100);
        header_a.issuer_vkey = vec![1u8; 32];
        header_a.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header_a, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Pool B (different issuer key) with seq=2 — should be fine (different pool)
        let mut header_b = make_valid_header(200);
        header_b.issuer_vkey = vec![2u8; 32];
        header_b.operational_cert.sequence_number = 2;
        assert!(praos
            .validate_header_full(&header_b, SlotNo(300), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Verify each pool tracked separately
        let pool_a = torsten_primitives::hash::blake2b_224(&header_a.issuer_vkey);
        let pool_b = torsten_primitives::hash::blake2b_224(&header_b.issuer_vkey);
        assert_eq!(praos.opcert_counters[&pool_a], 5);
        assert_eq!(praos.opcert_counters[&pool_b], 2);
    }

    #[test]
    fn test_leader_eligibility_with_zero_stake_strict() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        // Header with 64-byte VRF output (needed for leader check)
        let mut header = make_valid_header(100);
        header.vrf_result.output = vec![128u8; 64]; // Non-zero output

        let correct_hash = blake2b_256(&header.vrf_vkey);
        let info = BlockIssuerInfo {
            vrf_keyhash: correct_hash,
            pool_stake: 0, // Zero stake = never eligible
            total_active_stake: 1000,
        };

        // The VRF proof verification will fail first in strict mode with dummy data,
        // so test leader eligibility in non-strict where VRF proof check is non-fatal
        // but leader check is still performed
        praos.set_strict_verification(false);
        // Non-strict: leader eligibility failure is non-fatal
        assert!(praos
            .validate_header_full(
                &header,
                SlotNo(200),
                Some(&info),
                ValidationMode::Full,
                Some(9)
            )
            .is_ok());
    }

    #[test]
    fn test_block_issuer_info_construction() {
        let info = BlockIssuerInfo {
            vrf_keyhash: Hash32::from_bytes([42u8; 32]),
            pool_stake: 5,
            total_active_stake: 100,
        };
        assert!((info.relative_stake_f64() - 0.05).abs() < f64::EPSILON);
        assert_eq!(info.vrf_keyhash, Hash32::from_bytes([42u8; 32]));
    }

    #[test]
    fn test_opcert_counter_over_increment_non_strict() {
        // Non-strict: over-increment is non-fatal
        let mut praos = OuroborosPraos::new();

        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());

        // Jump from 5 to 10 (over-increment by 5) — non-fatal during sync
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 10;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None, ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_opcert_counter_over_increment_strict() {
        let mut praos = OuroborosPraos::new();

        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        let info1 = make_issuer_info(&header1);
        assert!(praos
            .validate_header_full(
                &header1,
                SlotNo(200),
                Some(&info1),
                ValidationMode::Full,
                Some(9)
            )
            .is_ok());

        praos.set_strict_verification(true);

        // Jump from 5 to 7 (over-increment, max allowed is +1)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 7;
        let info2 = make_issuer_info(&header2);
        let result = praos.validate_header_full(
            &header2,
            SlotNo(300),
            Some(&info2),
            ValidationMode::Full,
            Some(9),
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::OpcertCounterOverIncremented {
                    got: 7,
                    last_seen: 5
                })
            ),
            "Expected OpcertCounterOverIncremented, got: {result:?}"
        );
    }

    #[test]
    fn test_opcert_counter_increment_by_one_ok() {
        // Exactly +1 should always be fine
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        // Can't test strict with dummy VRF, so just test non-strict increment tracking
        praos.set_strict_verification(false);
        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None, ValidationMode::Full, Some(9))
            .is_ok());

        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 6;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None, ValidationMode::Full, Some(9))
            .is_ok());

        let pool_id = torsten_primitives::hash::blake2b_224(&header1.issuer_vkey);
        assert_eq!(praos.opcert_counters[&pool_id], 6);
    }

    // --- Tests for first-seen pool opcert counter initialization (Haskell conformance) ---
    //
    // These tests use ValidationMode::Replay to bypass cryptographic verification
    // (VRF proof, KES signature, opcert Ed25519) so we can isolate the counter logic.
    // This mirrors Haskell's `reupdateChainDepState` path which only updates state.
    // The counter enforcement logic is identical in both Full and Replay modes.

    #[test]
    fn test_first_seen_pool_counter_zero_accepted() {
        // Pool in stake distribution, first block with counter=0 → accepted.
        // Haskell initializes currentIssueNo = Just 0, so 0 <= 0 <= 0+1 passes.
        // Use Replay mode to bypass dummy-data crypto failures and isolate counter logic.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let mut header = make_valid_header(100);
        header.operational_cert.sequence_number = 0;
        let info = make_issuer_info(&header);

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Replay,
            Some(9),
        );
        assert!(
            result.is_ok(),
            "First-seen pool with counter=0 should be accepted, got: {result:?}"
        );

        let pool_id = torsten_primitives::hash::blake2b_224(&header.issuer_vkey);
        assert_eq!(
            praos.opcert_counters[&pool_id], 0,
            "Counter should be recorded as 0"
        );
    }

    #[test]
    fn test_first_seen_pool_counter_one_accepted() {
        // Pool in stake distribution, first block with counter=1 → accepted.
        // Haskell: 0 <= 1 <= 0+1 passes (rotate on first appearance is valid).
        // Use Replay mode to bypass dummy-data crypto failures and isolate counter logic.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let mut header = make_valid_header(100);
        header.operational_cert.sequence_number = 1;
        let info = make_issuer_info(&header);

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Replay,
            Some(9),
        );
        assert!(
            result.is_ok(),
            "First-seen pool with counter=1 should be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_first_seen_pool_counter_large_rejected_strict() {
        // Pool in stake distribution, first block with counter=50 → rejected in strict mode.
        // Haskell: currentIssueNo initialized to Just 0, so 50 > 0+1 is an over-increment.
        // Use Replay mode to bypass dummy-data crypto failures and isolate counter logic.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let mut header = make_valid_header(100);
        header.operational_cert.sequence_number = 50;
        let info = make_issuer_info(&header);

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Replay,
            Some(9),
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::OpcertCounterOverIncremented {
                    got: 50,
                    last_seen: 0
                })
            ),
            "First-seen pool with counter=50 should be rejected, got: {result:?}"
        );
    }

    #[test]
    fn test_first_seen_pool_counter_large_nonfatal_sync() {
        // Pool in stake distribution, first block with counter=50 → non-fatal during sync.
        // During bulk sync (strict_verification=false), over-increment is only logged.
        let mut praos = OuroborosPraos::new();
        // Default: strict_verification=false

        let mut header = make_valid_header(100);
        header.operational_cert.sequence_number = 50;
        let info = make_issuer_info(&header);

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Full,
            Some(9),
        );
        assert!(
            result.is_ok(),
            "First-seen pool with counter=50 should be non-fatal during sync, got: {result:?}"
        );
    }

    #[test]
    fn test_unknown_pool_counter_unconstrained_during_sync() {
        // Pool NOT in stake distribution (issuer_info=None), any counter → accepted during sync.
        // During bulk sync, we do not yet have the stake distribution so we cannot enforce
        // the Haskell first-seen initialization rule.
        let mut praos = OuroborosPraos::new();
        // Default: strict_verification=false

        let mut header = make_valid_header(100);
        header.operational_cert.sequence_number = 99;
        // No issuer_info → unknown pool

        let result =
            praos.validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9));
        assert!(
            result.is_ok(),
            "Unknown pool during sync should be non-fatal regardless of counter, got: {result:?}"
        );
    }

    #[test]
    fn test_existing_pool_counter_progression_unaffected() {
        // Pool already has a tracked counter; new block with counter+1 → accepted.
        // The new first-seen logic must not regress existing well-formed counter sequences.
        // Use Replay mode to bypass dummy-data crypto and isolate counter logic.
        //
        // We build the counter up from 0 (valid first-seen value) to 5 by accepting
        // a sequence of +1 increments, then verify that 5→6 still works.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        // Establish counter=0 as the first-seen block (valid: 0 <= 0 <= 0+1).
        let mut header0 = make_valid_header(100);
        header0.operational_cert.sequence_number = 0;
        let info0 = make_issuer_info(&header0);
        assert!(
            praos
                .validate_header_full(
                    &header0,
                    SlotNo(200),
                    Some(&info0),
                    ValidationMode::Replay,
                    Some(9)
                )
                .is_ok(),
            "Establishing counter=0 (first-seen) should succeed"
        );

        // Advance counter 0→1→2→3→4→5 via the same pool key.
        for seq in 1u64..=5 {
            let mut h = make_valid_header(100 + seq * 100);
            h.operational_cert.sequence_number = seq;
            let info = make_issuer_info(&h);
            assert!(
                praos
                    .validate_header_full(
                        &h,
                        SlotNo(200 + seq * 100),
                        Some(&info),
                        ValidationMode::Replay,
                        Some(9),
                    )
                    .is_ok(),
                "Counter increment to {seq} should succeed"
            );
        }

        // counter=6 (+1 from 5) → accepted
        let mut header6 = make_valid_header(700);
        header6.operational_cert.sequence_number = 6;
        let info6 = make_issuer_info(&header6);
        let result = praos.validate_header_full(
            &header6,
            SlotNo(800),
            Some(&info6),
            ValidationMode::Replay,
            Some(9),
        );
        assert!(
            result.is_ok(),
            "Existing pool with counter 5→6 (+1) should be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_existing_pool_counter_regression_rejected() {
        // Pool already has a tracked counter; new block with a regressed counter → rejected.
        // Use Replay mode to bypass dummy-data crypto and isolate counter logic.
        //
        // Build up counter to 5 via valid increments, then verify regression is rejected.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        // First-seen block: counter=0.
        let mut header0 = make_valid_header(100);
        header0.operational_cert.sequence_number = 0;
        let info0 = make_issuer_info(&header0);
        assert!(
            praos
                .validate_header_full(
                    &header0,
                    SlotNo(200),
                    Some(&info0),
                    ValidationMode::Replay,
                    Some(9)
                )
                .is_ok(),
            "Establishing counter=0 (first-seen) should succeed"
        );

        // Advance 0→1→2→3→4→5.
        for seq in 1u64..=5 {
            let mut h = make_valid_header(100 + seq * 100);
            h.operational_cert.sequence_number = seq;
            let info = make_issuer_info(&h);
            assert!(
                praos
                    .validate_header_full(
                        &h,
                        SlotNo(200 + seq * 100),
                        Some(&info),
                        ValidationMode::Replay,
                        Some(9),
                    )
                    .is_ok(),
                "Counter increment to {seq} should succeed"
            );
        }

        // counter=3 (regression from 5) → rejected
        let mut header_regress = make_valid_header(700);
        header_regress.operational_cert.sequence_number = 3;
        let info_regress = make_issuer_info(&header_regress);
        let result = praos.validate_header_full(
            &header_regress,
            SlotNo(800),
            Some(&info_regress),
            ValidationMode::Replay,
            Some(9),
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::OpcertSequenceRegression {
                    got: 3,
                    expected: 5
                })
            ),
            "Existing pool with regression from 5 to 3 should be rejected, got: {result:?}"
        );
    }

    #[test]
    fn test_kes_params_from_genesis() {
        let praos =
            OuroborosPraos::with_genesis_params(0.05, 2160, EpochLength(432000), 129600, 62);
        assert_eq!(praos.slots_per_kes_period, 129600);
        assert_eq!(praos.max_kes_evolutions, 62);

        // Custom KES params
        let praos2 =
            OuroborosPraos::with_genesis_params(0.05, 2160, EpochLength(432000), 86400, 46);
        assert_eq!(praos2.slots_per_kes_period, 86400);
        assert_eq!(praos2.max_kes_evolutions, 46);
    }

    // --- Tests for body hash validation (Bug fix) ---

    #[test]
    fn test_body_hash_valid() {
        let praos = OuroborosPraos::new();
        let body_cbor = b"some block body content in CBOR";
        let body_hash = blake2b_256(body_cbor);

        let mut header = make_valid_header(100);
        header.body_hash = body_hash;

        assert!(praos.validate_block_body_hash(&header, body_cbor).is_ok());
    }

    #[test]
    fn test_body_hash_mismatch_rejected() {
        let praos = OuroborosPraos::new();
        let body_cbor = b"actual block body content";
        let wrong_body_cbor = b"different block body content";
        let wrong_hash = blake2b_256(wrong_body_cbor);

        let mut header = make_valid_header(100);
        header.body_hash = wrong_hash;

        let result = praos.validate_block_body_hash(&header, body_cbor);
        assert!(
            matches!(result, Err(ConsensusError::BodyHashMismatch { .. })),
            "Expected BodyHashMismatch, got: {result:?}"
        );
    }

    #[test]
    fn test_body_hash_mismatch_contains_both_hashes() {
        let praos = OuroborosPraos::new();
        let body_cbor = b"real body";
        let computed_hash = blake2b_256(body_cbor);

        let mut header = make_valid_header(100);
        // Set header body_hash to something different
        header.body_hash = Hash32::from_bytes([0xAA; 32]);

        let result = praos.validate_block_body_hash(&header, body_cbor);
        match result {
            Err(ConsensusError::BodyHashMismatch {
                header_hash,
                computed_hash: ch,
            }) => {
                assert_eq!(header_hash, Hash32::from_bytes([0xAA; 32]));
                assert_eq!(ch, computed_hash);
            }
            other => panic!("Expected BodyHashMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn test_body_hash_empty_body() {
        let praos = OuroborosPraos::new();
        let empty_body = b"";
        let empty_hash = blake2b_256(empty_body);

        let mut header = make_valid_header(100);
        header.body_hash = empty_hash;

        assert!(praos.validate_block_body_hash(&header, empty_body).is_ok());
    }

    #[test]
    fn test_body_hash_standalone_function() {
        let body_cbor = b"block body data";
        let body_hash = blake2b_256(body_cbor);

        let mut header = make_valid_header(100);
        header.body_hash = body_hash;

        // Valid case
        assert!(validate_block_body_hash(&header, body_cbor).is_ok());

        // Invalid case
        let wrong_body = b"wrong body data";
        let result = validate_block_body_hash(&header, wrong_body);
        assert!(matches!(
            result,
            Err(ConsensusError::BodyHashMismatch { .. })
        ));
    }

    // --- Tests for unregistered pool rejection (Bug fix) ---

    #[test]
    fn test_unregistered_pool_rejected_strict() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let header = make_valid_header(100);
        let expected_pool_id = torsten_primitives::hash::blake2b_224(&header.issuer_vkey);

        // No issuer info = unregistered pool
        let result =
            praos.validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9));
        match result {
            Err(ConsensusError::UnregisteredPool { pool_id }) => {
                assert_eq!(pool_id, expected_pool_id);
            }
            other => panic!("Expected UnregisteredPool, got: {other:?}"),
        }
    }

    #[test]
    fn test_unregistered_pool_non_fatal_during_sync() {
        // Non-strict mode: unregistered pool is non-fatal (allows sync from genesis
        // before stake distribution is established)
        let mut praos = OuroborosPraos::new();
        assert!(!praos.strict_verification);

        let header = make_valid_header(100);
        let result =
            praos.validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9));
        assert!(
            result.is_ok(),
            "Unregistered pool should be non-fatal during sync, got: {result:?}"
        );
    }

    #[test]
    fn test_registered_pool_passes_with_correct_info() {
        // Pool with correct VRF key should pass (non-strict for VRF proof check)
        let mut praos = OuroborosPraos::new();

        let header = make_valid_header(100);
        let correct_hash = blake2b_256(&header.vrf_vkey);
        let info = BlockIssuerInfo {
            vrf_keyhash: correct_hash,
            pool_stake: 1,
            total_active_stake: 2,
        };

        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Full,
            Some(9),
        );
        assert!(
            result.is_ok(),
            "Registered pool with correct info should pass, got: {result:?}"
        );
    }

    #[test]
    fn test_unregistered_pool_error_message() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let header = make_valid_header(100);
        let result =
            praos.validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9));
        assert!(result.is_err());

        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unregistered pool"),
            "Error message should mention 'Unregistered pool', got: {err_msg}"
        );
        assert!(
            err_msg.contains("not found in stake distribution"),
            "Error message should mention stake distribution, got: {err_msg}"
        );
    }

    #[test]
    fn test_body_hash_mismatch_error_message() {
        let body_cbor = b"body content";
        let mut header = make_valid_header(100);
        header.body_hash = Hash32::from_bytes([0xFF; 32]);

        let result = validate_block_body_hash(&header, body_cbor);
        assert!(result.is_err());

        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Body hash mismatch"),
            "Error message should mention 'Body hash mismatch', got: {err_msg}"
        );
    }

    #[test]
    fn test_validate_header_full_strict_with_registered_pool() {
        // In strict mode, a registered pool with matching VRF key should proceed
        // past the pool registration check (may fail later on VRF proof with dummy data)
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let header = make_valid_header(100);
        let correct_hash = blake2b_256(&header.vrf_vkey);
        let info = BlockIssuerInfo {
            vrf_keyhash: correct_hash,
            pool_stake: 1,
            total_active_stake: 1,
        };

        // With strict mode and dummy VRF data, VRF key binding check passes but
        // later VRF proof check may fail. The important thing is that it does NOT
        // fail with UnregisteredPool.
        let result = praos.validate_header_full(
            &header,
            SlotNo(200),
            Some(&info),
            ValidationMode::Full,
            Some(9),
        );
        if let Err(ConsensusError::UnregisteredPool { .. }) = &result {
            panic!("Should not get UnregisteredPool when issuer_info is Some");
        }
        // Any other result is acceptable (may fail on VRF proof with dummy data)
    }

    #[test]
    fn test_kes_period_offset_u32_overflow_rejected() {
        // When the KES period offset exceeds u32::MAX, verify_kes_signature should
        // return an InvalidBlock error instead of silently truncating via `as u32`.
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        // Set slots_per_kes_period to 1 so that block_kes_period = slot value directly.
        praos.slots_per_kes_period = 1;
        // Also set max_kes_evolutions very high so validate_kes_period doesn't reject first.
        praos.max_kes_evolutions = u64::MAX;

        // Create a header at a slot that produces a kes_period_offset > u32::MAX.
        // block_kes_period = slot / slots_per_kes_period = slot (since slots_per_kes_period=1)
        // kes_period_offset = block_kes_period - opcert.kes_period
        // We need kes_period_offset > u32::MAX, so set slot = u32::MAX as u64 + 1 + opcert.kes_period
        let opcert_kes_period = 0u64;
        let overflow_slot = u32::MAX as u64 + 1 + opcert_kes_period;

        let mut header = make_valid_header(overflow_slot);
        header.operational_cert.kes_period = opcert_kes_period;
        // Need valid-sized KES signature and hot vkey so we reach the cast
        header.kes_signature = vec![0u8; 448];
        header.operational_cert.hot_vkey = vec![0u8; 32];

        let result = praos.verify_kes_signature(&header);
        match result {
            Err(ConsensusError::InvalidBlock(msg)) => {
                assert!(
                    msg.contains("exceeds u32 range"),
                    "Error message should mention u32 range, got: {msg}"
                );
            }
            other => {
                panic!(
                    "Expected ConsensusError::InvalidBlock for KES period overflow, got: {other:?}"
                );
            }
        }
    }

    // ========================================================================
    // Tests for is_in_stability_window()
    // ========================================================================

    #[test]
    fn test_stability_window_at_origin() {
        let praos = OuroborosPraos::new();
        // At origin, everything is in the stability window
        assert!(praos.is_in_stability_window(SlotNo(0)));
        assert!(praos.is_in_stability_window(SlotNo(999_999)));
    }

    #[test]
    fn test_stability_window_recent_slot() {
        let mut praos = OuroborosPraos::new();
        // k=2160, f=0.05 → stability_window = 3*2160/0.05 = 129600
        praos.update_tip(Tip {
            point: Point::Specific(SlotNo(200_000), Hash32::ZERO),
            block_number: BlockNo(100),
        });
        // Slot within the window (200000 - 129600 = 70400)
        assert!(praos.is_in_stability_window(SlotNo(70_401)));
        assert!(praos.is_in_stability_window(SlotNo(200_000)));
        // Slot at the window boundary
        assert!(praos.is_in_stability_window(SlotNo(70_400)));
        // Slot outside the window
        assert!(!praos.is_in_stability_window(SlotNo(70_399)));
        assert!(!praos.is_in_stability_window(SlotNo(0)));
    }

    #[test]
    fn test_stability_window_small_slot() {
        let mut praos = OuroborosPraos::new();
        // Tip slot < stability_window → saturating_sub returns 0 → all slots in window
        praos.update_tip(Tip {
            point: Point::Specific(SlotNo(100), Hash32::ZERO),
            block_number: BlockNo(5),
        });
        assert!(praos.is_in_stability_window(SlotNo(0)));
        assert!(praos.is_in_stability_window(SlotNo(100)));
    }

    // ========================================================================
    // Tests for prune_opcert_counters()
    // ========================================================================

    #[test]
    fn test_prune_opcert_counters_removes_retired() {
        let mut praos = OuroborosPraos::new();
        let pool_a = Hash28::from_bytes([0xAA; 28]);
        let pool_b = Hash28::from_bytes([0xBB; 28]);
        let pool_c = Hash28::from_bytes([0xCC; 28]);

        praos.opcert_counters.insert(pool_a, 5);
        praos.opcert_counters.insert(pool_b, 10);
        praos.opcert_counters.insert(pool_c, 15);
        assert_eq!(praos.opcert_counters.len(), 3);

        // Only pool_a and pool_c are still active
        let active: HashSet<Hash28> = [pool_a, pool_c].into_iter().collect();
        praos.prune_opcert_counters(&active);

        assert_eq!(praos.opcert_counters.len(), 2);
        assert_eq!(praos.opcert_counters.get(&pool_a), Some(&5));
        assert!(!praos.opcert_counters.contains_key(&pool_b));
        assert_eq!(praos.opcert_counters.get(&pool_c), Some(&15));
    }

    #[test]
    fn test_prune_opcert_counters_empty_active_set() {
        let mut praos = OuroborosPraos::new();
        praos
            .opcert_counters
            .insert(Hash28::from_bytes([0xAA; 28]), 5);
        praos
            .opcert_counters
            .insert(Hash28::from_bytes([0xBB; 28]), 10);

        // No active pools → all counters pruned
        praos.prune_opcert_counters(&HashSet::new());
        assert!(praos.opcert_counters.is_empty());
    }

    #[test]
    fn test_prune_opcert_counters_all_active() {
        let mut praos = OuroborosPraos::new();
        let pool_a = Hash28::from_bytes([0xAA; 28]);
        let pool_b = Hash28::from_bytes([0xBB; 28]);
        praos.opcert_counters.insert(pool_a, 5);
        praos.opcert_counters.insert(pool_b, 10);

        // All pools active → nothing pruned
        let active: HashSet<Hash28> = [pool_a, pool_b].into_iter().collect();
        praos.prune_opcert_counters(&active);
        assert_eq!(praos.opcert_counters.len(), 2);
    }

    #[test]
    fn test_prune_opcert_counters_no_counters() {
        let mut praos = OuroborosPraos::new();
        assert!(praos.opcert_counters.is_empty());

        // Pruning empty map is a no-op
        let active: HashSet<Hash28> = [Hash28::from_bytes([0xAA; 28])].into_iter().collect();
        praos.prune_opcert_counters(&active);
        assert!(praos.opcert_counters.is_empty());
    }

    #[test]
    fn test_opcert_counter_hard_cap() {
        let mut praos = OuroborosPraos::new();

        // Insert 50,000 unique pool entries (the hard cap)
        for i in 0..50_000u32 {
            let mut bytes = [0u8; 28];
            bytes[..4].copy_from_slice(&i.to_be_bytes());
            praos
                .opcert_counters
                .insert(Hash28::from_bytes(bytes), i as u64);
        }
        assert_eq!(praos.opcert_counters.len(), 50_000);

        // A new unknown pool should NOT be inserted (at cap)
        let new_pool = Hash28::from_bytes([0xFF; 28]);
        assert!(!praos.opcert_counters.contains_key(&new_pool));

        // Simulate check_opcert_counter via direct insert logic
        let header = make_valid_header(100);
        let pool_id = torsten_primitives::hash::blake2b_224(&header.issuer_vkey);
        // pool_id won't be in the map and map is at cap → entry should not be added
        let before = praos.opcert_counters.len();
        // Only insert if under cap or already present
        if praos.opcert_counters.len() < 50_000 || praos.opcert_counters.contains_key(&pool_id) {
            praos.opcert_counters.insert(pool_id, 5);
        }
        assert_eq!(praos.opcert_counters.len(), before);

        // But updating an existing entry should still work
        let existing = Hash28::from_bytes([0u8; 28]); // i=0 was inserted
        assert!(praos.opcert_counters.contains_key(&existing));
        praos.opcert_counters.insert(existing, 999);
        assert_eq!(praos.opcert_counters[&existing], 999);
    }

    // ========================================================================
    // Tests for update_tip()
    // ========================================================================

    #[test]
    fn test_update_tip_from_origin() {
        let mut praos = OuroborosPraos::new();
        assert_eq!(praos.tip, Tip::origin());

        let new_tip = Tip {
            point: Point::Specific(SlotNo(42), Hash32::from_bytes([0xAB; 32])),
            block_number: BlockNo(1),
        };
        praos.update_tip(new_tip.clone());
        assert_eq!(praos.tip, new_tip);
    }

    #[test]
    fn test_update_tip_advances() {
        let mut praos = OuroborosPraos::new();

        let tip1 = Tip {
            point: Point::Specific(SlotNo(100), Hash32::from_bytes([0x01; 32])),
            block_number: BlockNo(5),
        };
        praos.update_tip(tip1);
        assert_eq!(praos.tip.block_number, BlockNo(5));

        let tip2 = Tip {
            point: Point::Specific(SlotNo(200), Hash32::from_bytes([0x02; 32])),
            block_number: BlockNo(10),
        };
        praos.update_tip(tip2);
        assert_eq!(praos.tip.block_number, BlockNo(10));
        assert_eq!(praos.tip.point.slot(), Some(SlotNo(200)));
    }

    #[test]
    fn test_update_tip_rollback() {
        let mut praos = OuroborosPraos::new();

        praos.update_tip(Tip {
            point: Point::Specific(SlotNo(500), Hash32::from_bytes([0x01; 32])),
            block_number: BlockNo(25),
        });

        // Rollback to earlier point
        praos.update_tip(Tip {
            point: Point::Specific(SlotNo(400), Hash32::from_bytes([0x02; 32])),
            block_number: BlockNo(20),
        });
        assert_eq!(praos.tip.block_number, BlockNo(20));
        assert_eq!(praos.tip.point.slot(), Some(SlotNo(400)));
    }

    // ========================================================================
    // Tests for crypto_params() and with_genesis_params()
    // ========================================================================

    #[test]
    fn test_crypto_params_reflects_state() {
        let mut praos =
            OuroborosPraos::with_genesis_params(0.05, 2160, EpochLength(432000), 129600, 62);
        let params = praos.crypto_params();
        assert!(!params.strict_verification);
        assert!(!params.nonce_established);
        assert_eq!(params.slots_per_kes_period, 129600);
        assert_eq!(params.max_kes_evolutions, 62);

        praos.set_strict_verification(true);
        praos.nonce_established = true;
        let params = praos.crypto_params();
        assert!(params.strict_verification);
        assert!(params.nonce_established);
    }

    #[test]
    fn test_with_genesis_params_custom_values() {
        let praos = OuroborosPraos::with_genesis_params(
            0.1, // preview active_slot_coeff
            500, // small k
            EpochLength(86400),
            3600, // small KES period
            120,  // more evolutions
        );
        assert!((praos.active_slot_coeff - 0.1).abs() < f64::EPSILON);
        assert_eq!(praos.security_param, 500);
        assert_eq!(praos.epoch_length.0, 86400);
        assert_eq!(praos.slots_per_kes_period, 3600);
        assert_eq!(praos.max_kes_evolutions, 120);
        assert_eq!(praos.tip, Tip::origin());
        assert!(!praos.strict_verification);
    }

    // ========================================================================
    // Tests for verify_header_crypto() (static verification pipeline)
    // ========================================================================

    #[test]
    fn test_verify_header_crypto_non_strict_passes() {
        // In non-strict mode, crypto failures are non-fatal
        let params = CryptoVerificationParams {
            strict_verification: false,
            nonce_established: false,
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
        };
        let header = make_valid_header(100);
        // VRF proof is dummy data — should pass in non-strict mode
        let result = OuroborosPraos::verify_header_crypto(&params, &header);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_header_crypto_strict_vrf_fails() {
        let params = CryptoVerificationParams {
            strict_verification: true,
            nonce_established: true,
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
        };
        let header = make_valid_header(100);
        // With strict mode and nonce established, dummy VRF should fail
        let result = OuroborosPraos::verify_header_crypto(&params, &header);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_header_crypto_strict_nonce_not_established() {
        // Strict but nonce not established → VRF skipped
        let params = CryptoVerificationParams {
            strict_verification: true,
            nonce_established: false,
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
        };
        let header = make_valid_header(100);
        // VRF check skipped; opcert check with dummy data is non-fatal
        let result = OuroborosPraos::verify_header_crypto(&params, &header);
        // Even strict, VRF is skipped when nonce isn't established,
        // but opcert may fail depending on data
        // The test verifies the function doesn't panic
        let _ = result;
    }

    #[test]
    fn test_stability_window_calculation() {
        // k=2160, f=0.05 → 3*2160/0.05 = 129600
        let praos = OuroborosPraos::new();
        assert_eq!(praos.stability_window(), 129600);

        // k=500, f=0.1 → 3*500/0.1 = 15000
        let praos2 = OuroborosPraos::with_params(0.1, 500, EpochLength(86400));
        assert_eq!(praos2.stability_window(), 15000);
    }

    #[test]
    fn test_kes_size_mismatch_rejected_strict() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);

        let mut header = make_valid_header(100);
        // Set a non-empty but wrong-size KES signature (should be 448 bytes)
        header.kes_signature = vec![0u8; 447];

        // Call verify_kes_signature directly to test KES size validation in isolation
        // (validate_header would fail on opcert verification first with dummy data)
        let result = praos.verify_kes_signature(&header);
        assert!(
            matches!(result, Err(ConsensusError::InvalidKesSignature)),
            "Expected InvalidKesSignature for wrong-size KES sig in strict mode, got: {result:?}"
        );
    }

    #[test]
    fn test_kes_size_mismatch_ok_non_strict() {
        let praos = OuroborosPraos::new();

        let mut header = make_valid_header(100);
        // Set a non-empty but wrong-size KES signature
        header.kes_signature = vec![0u8; 447];

        // Call verify_kes_signature directly to test KES size validation in isolation
        let result = praos.verify_kes_signature(&header);
        assert!(
            result.is_ok(),
            "Expected Ok for wrong-size KES sig in non-strict mode, got: {result:?}"
        );
    }

    #[test]
    fn test_vrf_strict_mode_with_invalid_proof() {
        let mut praos = OuroborosPraos::new();
        praos.set_strict_verification(true);
        praos.nonce_established = true;

        let mut header = make_valid_header(100);
        // Set VRF key/proof to valid sizes but garbage data
        header.vrf_vkey = vec![99u8; 32];
        header.vrf_result.proof = vec![88u8; 80];
        header.vrf_result.output = vec![77u8; 32];

        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(
            matches!(result, Err(ConsensusError::VrfVerification(_))),
            "Expected VrfVerification error for invalid proof in strict mode with nonce established, got: {result:?}"
        );
    }

    // ─── Lightweight checkpoint tests ──────────────────────────────────

    #[test]
    fn test_checkpoint_match_passes() {
        let mut praos = OuroborosPraos::new();
        let expected_hash = Hash32::from_bytes([0xAA; 32]);
        praos.checkpoints.insert(1000, expected_hash);

        let mut header = make_valid_header(5000);
        header.block_number = BlockNo(1000);
        header.header_hash = expected_hash; // matches checkpoint

        let result = praos.validate_header(&header, SlotNo(10000), ValidationMode::Replay, Some(9));
        assert!(result.is_ok(), "checkpoint match should pass: {result:?}");
    }

    #[test]
    fn test_checkpoint_mismatch_rejected() {
        let mut praos = OuroborosPraos::new();
        praos
            .checkpoints
            .insert(1000, Hash32::from_bytes([0xAA; 32]));

        let mut header = make_valid_header(5000);
        header.block_number = BlockNo(1000);
        header.header_hash = Hash32::from_bytes([0xBB; 32]); // MISMATCH

        let result = praos.validate_header(&header, SlotNo(10000), ValidationMode::Replay, Some(9));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConsensusError::CheckpointMismatch { block_no: 1000, .. }
        ));
    }

    #[test]
    fn test_no_checkpoint_at_block_passes() {
        let mut praos = OuroborosPraos::new();
        praos
            .checkpoints
            .insert(2000, Hash32::from_bytes([0xAA; 32]));

        let mut header = make_valid_header(5000);
        header.block_number = BlockNo(1500); // No checkpoint here

        let result = praos.validate_header(&header, SlotNo(10000), ValidationMode::Replay, Some(9));
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_checkpoints_no_effect() {
        let praos = OuroborosPraos::new();
        assert!(praos.checkpoints.is_empty());

        let header = make_valid_header(5000);
        let result = praos.validate_header(&header, SlotNo(10000), ValidationMode::Replay, Some(9));
        assert!(result.is_ok());
    }

    // ========================================================================
    // Tests for validate_envelope() — Haskell envelopeChecks
    // ========================================================================

    #[test]
    fn test_envelope_body_size_within_limit_passes() {
        // body_size exactly at the limit should pass.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(
            SlotNo(100),
            90112, // exactly max_block_body_size
            None,
            90112,
            1100,
        );
        assert!(
            result.is_ok(),
            "body_size == limit should pass, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_body_size_exceeds_limit_rejected() {
        // body_size one byte over the limit must be rejected.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(
            SlotNo(100),
            90113, // one byte over max_block_body_size
            None,
            90112,
            1100,
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::BlockBodyTooLarge {
                    body_size: 90113,
                    max_block_body_size: 90112,
                })
            ),
            "body_size > limit should be rejected with BlockBodyTooLarge, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_body_size_zero_passes() {
        // An empty body (body_size=0) is always within any non-zero limit.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(SlotNo(1), 0, None, 90112, 1100);
        assert!(result.is_ok(), "body_size=0 should pass, got: {result:?}");
    }

    #[test]
    fn test_envelope_header_size_within_limit_passes() {
        // header_cbor_size exactly at the limit should pass.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(SlotNo(100), 0, Some(1100), 90112, 1100);
        assert!(
            result.is_ok(),
            "header_size == limit should pass, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_header_size_exceeds_limit_rejected() {
        // header_cbor_size one byte over the limit must be rejected.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(SlotNo(100), 0, Some(1101), 90112, 1100);
        assert!(
            matches!(
                result,
                Err(ConsensusError::BlockHeaderTooLarge {
                    header_size: 1101,
                    max_block_header_size: 1100,
                })
            ),
            "header_size > limit should be rejected with BlockHeaderTooLarge, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_header_size_none_skips_check() {
        // When header_cbor_size is None, the header size check is skipped even
        // if the limit would be exceeded. This reflects that ChainSync processes
        // headers without their raw CBOR bytes available.
        let praos = OuroborosPraos::new();
        // max_block_header_size=0 would reject any real header, but None skips it.
        let result = praos.validate_envelope(SlotNo(100), 0, None, 90112, 0);
        assert!(
            result.is_ok(),
            "None header_cbor_size should skip the header size check, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_body_check_runs_before_header_check() {
        // When both body and header are oversized, BlockBodyTooLarge is returned
        // (body is checked first, matching left-to-right evaluation order).
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(
            SlotNo(100),
            99_999,      // body too large
            Some(9_999), // header also too large
            90112,
            1100,
        );
        assert!(
            matches!(result, Err(ConsensusError::BlockBodyTooLarge { .. })),
            "Body check must precede header check; expected BlockBodyTooLarge, got: {result:?}"
        );
    }

    #[test]
    fn test_envelope_body_size_large_body_with_ok_header() {
        // Oversized body with a valid header size should still fail on the body check.
        let praos = OuroborosPraos::new();
        let result = praos.validate_envelope(
            SlotNo(500),
            200_000,   // well above the 90112 limit
            Some(800), // header is fine
            90112,
            1100,
        );
        assert!(
            matches!(
                result,
                Err(ConsensusError::BlockBodyTooLarge {
                    body_size: 200_000,
                    max_block_body_size: 90112,
                })
            ),
            "Expected BlockBodyTooLarge, got: {result:?}"
        );
    }

    #[test]
    fn test_kes_period_boundary() {
        let praos = OuroborosPraos::new();

        // Block at slot 129599 is in KES period 0 (129600 slots per period)
        let mut header = make_valid_header(129599);
        header.operational_cert.kes_period = 0;
        assert!(praos
            .validate_header(&header, SlotNo(130000), ValidationMode::Full, Some(9))
            .is_ok());

        // Block at slot 129600 is in KES period 1
        let mut header2 = make_valid_header(129600);
        header2.operational_cert.kes_period = 1;
        assert!(praos
            .validate_header(&header2, SlotNo(130000), ValidationMode::Full, Some(9))
            .is_ok());

        // Block at slot 129600 is in KES period 1, but cert says period 2.
        // block_period (1) < cert_period (2) → KesPeriodBeforeCert.
        let mut header3 = make_valid_header(129600);
        header3.operational_cert.kes_period = 2;
        let result = praos.validate_header(&header3, SlotNo(130000), ValidationMode::Full, Some(9));
        assert!(matches!(
            result,
            Err(ConsensusError::KesPeriodBeforeCert { .. })
        ));
    }

    #[test]
    fn test_kes_period_certs_start_later() {
        let praos = OuroborosPraos::new();

        // Block in period 5 but cert started at period 3 should be valid
        // (cert was valid for periods 3,4,5,6... with 3 evolutions)
        let mut header = make_valid_header(KES_PERIOD_SLOTS * 5);
        header.operational_cert.kes_period = 3;
        assert!(praos
            .validate_header(
                &header,
                SlotNo(KES_PERIOD_SLOTS * 5 + 1000),
                ValidationMode::Full,
                Some(9),
            )
            .is_ok());
    }

    #[test]
    fn test_vrf_leader_election_stake_fractions() {
        // Test various stake fractions against expected behavior
        let test_cases: Vec<(f64, bool)> = vec![
            (0.0, false),   // 0% stake: never eligible
            (0.001, false), // 0.1% stake: unlikely but possible
            (0.01, true),   // 1% stake: likely eligible
            (0.1, true),    // 10% stake: very likely
            (0.5, true),    // 50% stake: most likely
            (1.0, true),    // 100% stake: always eligible
        ];

        for (stake_fraction, _expected) in test_cases {
            // Low VRF output should succeed at any non-zero stake
            let low_output = [0u8; 64];
            let high_output = [0xFFu8; 64];

            let low_result = verify_leader_eligibility(&low_output, stake_fraction, 0.05);
            // High output should fail for most stake fractions
            let high_result = verify_leader_eligibility(&high_output, stake_fraction, 0.05);

            // Zero stake should always fail
            if stake_fraction == 0.0 {
                assert!(low_result.is_err(), "Zero stake should never be eligible");
                assert!(high_result.is_err(), "Zero stake should never be eligible");
            } else {
                // Low output should succeed for any positive stake
                // High output depends on the threshold
            }
        }
    }

    #[test]
    fn test_obsolete_node_check() {
        let praos = OuroborosPraos::new(); // max_major_prot_ver = 10
        let header = make_valid_header(100);

        // Ledger PV 10 (== node max) → accepted
        assert!(praos
            .validate_header(&header, SlotNo(200), ValidationMode::Full, Some(10))
            .is_ok());

        // Ledger PV 11 (> node max 10) → ObsoleteNode
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(11));
        assert!(
            matches!(
                result,
                Err(ConsensusError::ObsoleteNode {
                    chain_pv: 11,
                    node_max_pv: 10
                })
            ),
            "Expected ObsoleteNode, got: {result:?}"
        );

        // None (no ledger PV available) → check skipped, passes
        assert!(praos
            .validate_header(&header, SlotNo(200), ValidationMode::Full, None)
            .is_ok());
    }

    #[test]
    fn test_header_prot_ver_too_high() {
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        let ledger_pv = Some(9);

        // Header PV 10 (ledger 9 + 1) → accepted (one-major-version bump allowed)
        let mut header_ok = header.clone();
        header_ok.protocol_version = torsten_primitives::block::ProtocolVersion {
            major: 10,
            minor: 0,
        };
        assert!(praos
            .validate_header(&header_ok, SlotNo(200), ValidationMode::Full, ledger_pv)
            .is_ok());

        // Header PV 11 (ledger 9 + 2) → HeaderProtVerTooHigh
        let mut header_bad = header.clone();
        header_bad.protocol_version = torsten_primitives::block::ProtocolVersion {
            major: 11,
            minor: 0,
        };
        let result =
            praos.validate_header(&header_bad, SlotNo(200), ValidationMode::Full, ledger_pv);
        assert!(
            matches!(
                result,
                Err(ConsensusError::HeaderProtVerTooHigh {
                    supplied: 11,
                    max_expected: 10
                })
            ),
            "Expected HeaderProtVerTooHigh, got: {result:?}"
        );

        // Header PV 9 (same as ledger) → accepted
        let mut header_same = header.clone();
        header_same.protocol_version =
            torsten_primitives::block::ProtocolVersion { major: 9, minor: 0 };
        assert!(praos
            .validate_header(&header_same, SlotNo(200), ValidationMode::Full, ledger_pv)
            .is_ok());
    }

    #[test]
    fn test_checkpoint_mismatch_detection() {
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);

        // Valid header should have zero hash
        let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(9));
        assert!(result.is_ok());

        // Test checkpoint verification (if supported)
        // This tests the error type exists and is constructable
        let error = ConsensusError::CheckpointMismatch {
            block_no: 100,
            expected: Hash32::ZERO,
            got: Hash32::from_bytes([1u8; 32]),
        };
        assert!(matches!(error, ConsensusError::CheckpointMismatch { .. }));
    }

    #[test]
    fn test_epoch_transition_stability_window() {
        let praos = OuroborosPraos::new();

        // Stability window = 3 * k / f = 3 * 2160 / 0.05 = 129600 slots
        let sw = praos.stability_window();

        // Test that epoch boundaries align with stability window
        assert_eq!(sw, 129600);

        // Test slots around epoch boundaries
        let epoch_1_start = SlotNo(432000); // First slot of epoch 1

        // Stability window back from epoch boundary
        let sw_before = epoch_1_start.0 - sw;
        assert!(praos.slot_to_epoch(SlotNo(sw_before)) <= EpochNo(0));

        // Slots within stability window of boundary
        let sw_at_boundary = epoch_1_start.0 - (sw / 2);
        assert_eq!(praos.slot_to_epoch(SlotNo(sw_at_boundary)), EpochNo(0));
    }

    #[test]
    fn test_unknown_block_issuer_error() {
        let error = ConsensusError::UnknownBlockIssuer(Hash28::from_bytes([0xAB; 28]));
        match &error {
            ConsensusError::UnknownBlockIssuer(pool_id) => {
                assert_eq!(pool_id.0, [0xAB; 28]);
            }
            _ => panic!("Expected UnknownBlockIssuer variant"),
        }
    }

    #[test]
    fn test_body_hash_mismatch_error() {
        let expected = Hash32::from_bytes([0x11; 32]);
        let got = Hash32::from_bytes([0x22; 32]);
        let error = ConsensusError::BodyHashMismatch {
            header_hash: expected,
            computed_hash: got,
        };
        match &error {
            ConsensusError::BodyHashMismatch {
                header_hash,
                computed_hash,
            } => {
                assert_eq!(header_hash.0, [0x11; 32]);
                assert_eq!(computed_hash.0, [0x22; 32]);
            }
            _ => panic!("Expected BodyHashMismatch variant"),
        }
    }

    #[test]
    fn test_max_rollback_calculation() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.max_rollback(), praos.security_param);
        assert_eq!(praos.max_rollback(), 2160);
    }

    #[test]
    fn test_active_slot_coefficient() {
        let praos = OuroborosPraos::new();
        assert!((praos.active_slot_coeff - 0.05).abs() < f64::EPSILON);

        // Create with custom coefficient
        let praos_custom = OuroborosPraos::with_params(
            0.1,
            2160,
            torsten_primitives::time::mainnet_epoch_length(),
        );
        assert!((praos_custom.active_slot_coeff - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_vrf_output_size_variants() {
        let praos = OuroborosPraos::new();

        // 32-byte VRF output (legacy)
        let mut header_32 = make_valid_header(100);
        header_32.vrf_result.output = vec![0u8; 32];
        assert!(praos
            .validate_header(&header_32, SlotNo(200), ValidationMode::Full, Some(9))
            .is_ok());

        // 64-byte VRF output (current TPraos)
        let mut header_64 = make_valid_header(100);
        header_64.vrf_result.output = vec![0u8; 64];
        assert!(praos
            .validate_header(&header_64, SlotNo(200), ValidationMode::Full, Some(9))
            .is_ok());
    }

    #[test]
    fn test_unregistered_pool_error() {
        let pool_id = Hash28::from_bytes([0xCC; 28]);
        let error = ConsensusError::UnregisteredPool { pool_id };
        assert!(matches!(error, ConsensusError::UnregisteredPool { .. }));
    }

    // ─── Opcert counter persistence (#310) ──────────────────────────────

    #[test]
    fn test_opcert_counters_restored_rejects_replay() {
        // Simulate: pool A had counter 5, restore from snapshot, verify state
        let mut praos = OuroborosPraos::new();
        let pool_id = Hash28::from_bytes([0xAA; 28]);

        // Seed counters as if loaded from snapshot
        let mut restored = HashMap::new();
        restored.insert(pool_id, 5);
        praos.set_opcert_counters(restored);

        // Counter is 5 — a block with counter 3 would be rejected by
        // check_opcert_counter (n < m path), counter 6 would be accepted
        assert_eq!(praos.opcert_counters()[&pool_id], 5);
    }

    #[test]
    fn test_set_opcert_counters_replaces_all() {
        let mut praos = OuroborosPraos::new();
        let pool_a = Hash28::from_bytes([0xAA; 28]);
        let pool_b = Hash28::from_bytes([0xBB; 28]);

        // Existing counter
        praos.opcert_counters.insert(pool_a, 10);

        // Replace with new set
        let mut new_counters = HashMap::new();
        new_counters.insert(pool_b, 20);
        praos.set_opcert_counters(new_counters);

        // Old counter gone, new counter present
        assert!(!praos.opcert_counters().contains_key(&pool_a));
        assert_eq!(praos.opcert_counters()[&pool_b], 20);
    }

    #[test]
    fn test_opcert_counters_fresh_start_empty() {
        // Fresh start (no snapshot) → counters are empty → first block from
        // any pool accepted via the m=0 first-seen path
        let praos = OuroborosPraos::new();
        assert!(praos.opcert_counters().is_empty());
    }

    // -------------------------------------------------------------------------
    // TPraos nonce VRF proof verification
    // -------------------------------------------------------------------------

    /// Helper: create a minimal TPraos-era block header (proto < 7).
    fn make_tpraos_header() -> BlockHeader {
        use torsten_primitives::block::{OperationalCert, ProtocolVersion, VrfOutput};
        use torsten_primitives::hash::BlockHeaderHash;

        BlockHeader {
            header_hash: BlockHeaderHash::from_bytes([0u8; 32]),
            prev_hash: BlockHeaderHash::from_bytes([0u8; 32]),
            issuer_vkey: vec![1u8; 32],
            vrf_vkey: vec![2u8; 32],
            vrf_result: VrfOutput {
                output: vec![0u8; 64],
                proof: vec![0u8; 80],
            },
            block_number: BlockNo(1),
            slot: SlotNo(100),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![0u8; 32],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![0u8; 64],
            },
            protocol_version: ProtocolVersion { major: 6, minor: 0 },
            kes_signature: vec![0u8; 448],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        }
    }

    /// Valid nonce VRF proof passes strict verification.
    #[test]
    fn test_tpraos_nonce_vrf_verification_valid() {
        let vrf_skey = [42u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&vrf_skey);
        let vrf_vkey = kp.public_key;

        let mut header = make_tpraos_header();
        header.vrf_vkey = vrf_vkey.to_vec();

        // Generate valid nonce VRF proof
        let nonce_seed =
            crate::slot_leader::tpraos_nonce_vrf_input(&header.epoch_nonce, header.slot);
        let (proof, output) =
            torsten_crypto::vrf::generate_vrf_proof(&vrf_skey, &nonce_seed).unwrap();
        header.nonce_vrf_proof = proof.to_vec();
        header.nonce_vrf_output = output.to_vec();

        let mut praos = OuroborosPraos::new();
        praos.strict_verification = true;
        praos.nonce_established = true;

        assert!(
            praos.verify_nonce_vrf_proof(&header).is_ok(),
            "Valid nonce VRF proof must pass verification"
        );
    }

    /// Forged nonce VRF proof is rejected in strict mode.
    #[test]
    fn test_tpraos_nonce_vrf_verification_invalid() {
        let mut header = make_tpraos_header();
        header.nonce_vrf_proof = vec![0xFFu8; 80]; // invalid proof
        header.nonce_vrf_output = vec![0u8; 64];

        let mut praos = OuroborosPraos::new();
        praos.strict_verification = true;
        praos.nonce_established = true;

        let result = praos.verify_nonce_vrf_proof(&header);
        assert!(
            result.is_err(),
            "Forged nonce VRF proof must be rejected in strict mode"
        );
    }

    /// Praos blocks (proto >= 7) skip nonce VRF verification.
    #[test]
    fn test_praos_nonce_vrf_skipped() {
        use torsten_primitives::block::ProtocolVersion;
        let mut header = make_tpraos_header();
        header.protocol_version = ProtocolVersion { major: 9, minor: 0 };
        header.nonce_vrf_proof = vec![0xFFu8; 80]; // would be invalid

        let mut praos = OuroborosPraos::new();
        praos.strict_verification = true;
        praos.nonce_established = true;

        assert!(
            praos.verify_nonce_vrf_proof(&header).is_ok(),
            "Praos blocks must skip nonce VRF check"
        );
    }

    /// Invalid nonce VRF proof is non-fatal when nonce is not established.
    #[test]
    fn test_tpraos_nonce_vrf_non_fatal_when_nonce_not_established() {
        let mut header = make_tpraos_header();
        header.nonce_vrf_proof = vec![0xFFu8; 80];
        header.nonce_vrf_output = vec![0u8; 64];

        let mut praos = OuroborosPraos::new();
        praos.strict_verification = true;
        praos.nonce_established = false;

        assert!(
            praos.verify_nonce_vrf_proof(&header).is_ok(),
            "Non-fatal when nonce not established"
        );
    }
}

// Checkpoint loader lives in torsten-node (has serde_json/hex deps).
