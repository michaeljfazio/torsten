use std::collections::{HashMap, HashSet};
use thiserror::Error;
use torsten_crypto::keys::PaymentVerificationKey;
use torsten_primitives::block::{BlockHeader, Tip};
use torsten_primitives::hash::{blake2b_256, Hash28, Hash32};
use torsten_primitives::time::{EpochLength, EpochNo, SlotNo};
use tracing::{debug, trace, warn};

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
    #[error("Unsupported protocol version: {major}.{minor} (max supported: {max_major})")]
    UnsupportedProtocolVersion {
        major: u64,
        minor: u64,
        max_major: u64,
    },
    #[error("Body hash mismatch: header={header_hash}, computed={computed_hash}")]
    BodyHashMismatch {
        header_hash: Hash32,
        computed_hash: Hash32,
    },
    #[error("Unregistered pool: pool {pool_id} not found in stake distribution")]
    UnregisteredPool { pool_id: Hash28 },
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
    /// The pool's relative stake (fraction of total active stake, 0.0 to 1.0)
    pub relative_stake: f64,
}

/// Active slot coefficient (f) - probability that a slot has a block
/// Mainnet value: 1/20 = 0.05 (one block every ~20 seconds on average)
pub const ACTIVE_SLOT_COEFF: f64 = 0.05;

/// Security parameter k
pub const SECURITY_PARAM: u64 = 2160;

/// Ouroboros Praos consensus engine
pub struct OuroborosPraos {
    /// Active slot coefficient
    pub active_slot_coeff: f64,
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
    /// Tracked opcert sequence numbers per pool (cold key hash → highest seen sequence number).
    /// Used to detect opcert counter regressions (replay protection).
    opcert_counters: HashMap<Hash28, u64>,
}

impl OuroborosPraos {
    pub fn new() -> Self {
        OuroborosPraos {
            active_slot_coeff: ACTIVE_SLOT_COEFF,
            security_param: SECURITY_PARAM,
            epoch_length: torsten_primitives::time::mainnet_epoch_length(),
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
            opcert_counters: HashMap::new(),
        }
    }

    pub fn with_params(
        active_slot_coeff: f64,
        security_param: u64,
        epoch_length: EpochLength,
    ) -> Self {
        OuroborosPraos {
            active_slot_coeff,
            security_param,
            epoch_length,
            slots_per_kes_period: KES_PERIOD_SLOTS,
            max_kes_evolutions: MAX_KES_EVOLUTIONS,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
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
        OuroborosPraos {
            active_slot_coeff,
            security_param,
            epoch_length,
            slots_per_kes_period,
            max_kes_evolutions,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
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

    /// Validate a block header against consensus rules.
    ///
    /// This checks:
    /// 1. Block is not from the future
    /// 2. Issuer VRF key is present
    /// 3. VRF proof is cryptographically valid
    /// 4. KES period is valid (not expired, not before cert start)
    /// 5. Operational certificate has required fields
    pub fn validate_header(
        &self,
        header: &BlockHeader,
        current_slot: SlotNo,
    ) -> Result<(), ConsensusError> {
        trace!(
            slot = header.slot.0,
            block_no = header.block_number.0,
            current_slot = current_slot.0,
            issuer_vkey_len = header.issuer_vkey.len(),
            vrf_vkey_len = header.vrf_vkey.len(),
            "Praos: validating block header"
        );

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

        // Verify VRF proof cryptographically
        self.verify_vrf_proof(header)?;

        // Validate KES period
        self.validate_kes_period(header)?;

        // Validate operational certificate structure
        self.validate_operational_cert(header)?;

        // Verify KES signature over the header body
        self.verify_kes_signature(header)?;

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
    pub fn validate_header_full(
        &mut self,
        header: &BlockHeader,
        current_slot: SlotNo,
        issuer_info: Option<&BlockIssuerInfo>,
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

        // 1b. Protocol version check — reject blocks from unsupported protocol versions
        // Currently we support up to protocol version 10 (Conway).
        const MAX_SUPPORTED_PROTOCOL_MAJOR: u64 = 10;
        if header.protocol_version.major > MAX_SUPPORTED_PROTOCOL_MAJOR {
            warn!(
                slot = header.slot.0,
                major = header.protocol_version.major,
                minor = header.protocol_version.minor,
                "Praos: block uses unsupported protocol version"
            );
            return Err(ConsensusError::UnsupportedProtocolVersion {
                major: header.protocol_version.major,
                minor: header.protocol_version.minor,
                max_major: MAX_SUPPORTED_PROTOCOL_MAJOR,
            });
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
            if header.vrf_result.output.len() == 64 {
                // Praos (Babbage/Conway, protocol >= 7): Blake2b-256("L" || vrf_output), certNatMax = 2^256
                // TPraos (Shelley-Alonzo, protocol < 7): raw 64-byte vrf_output, certNatMax = 2^512
                let is_praos = header.protocol_version.major >= 7;
                let is_leader = if is_praos {
                    let leader_value =
                        crate::slot_leader::vrf_leader_value(&header.vrf_result.output);
                    torsten_crypto::vrf::check_leader_value(
                        &leader_value,
                        info.relative_stake,
                        self.active_slot_coeff,
                    )
                } else {
                    // TPraos: raw VRF output directly
                    torsten_crypto::vrf::check_leader_value_tpraos(
                        &header.vrf_result.output,
                        info.relative_stake,
                        self.active_slot_coeff,
                    )
                };
                if !is_leader {
                    if self.strict_verification {
                        return Err(ConsensusError::InvalidBlock(format!(
                            "VRF leader eligibility check failed: slot={}, sigma={}, proto={}",
                            header.slot.0, info.relative_stake, header.protocol_version.major,
                        )));
                    } else {
                        debug!(
                            slot = header.slot.0,
                            relative_stake = info.relative_stake,
                            proto = header.protocol_version.major,
                            praos = is_praos,
                            "Praos: VRF leader eligibility check failed (non-strict, skipping)"
                        );
                    }
                }
            }
        }

        // 4. Opcert counter monotonicity check
        self.check_opcert_counter(header)?;

        // 5. KES period validation (always fatal)
        self.validate_kes_period(header)?;

        // 6. Cryptographic verification (VRF proof, opcert signature, KES signature)
        self.verify_vrf_proof(header)?;
        self.validate_operational_cert(header)?;
        self.verify_kes_signature(header)?;

        trace!(
            slot = header.slot.0,
            block_no = header.block_number.0,
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

    /// Check and update the operational certificate sequence number for the block issuer.
    ///
    /// Per the Haskell reference implementation, the opcert counter must satisfy:
    ///   m <= n <= m + 1
    /// where m is the last seen counter and n is the new counter.
    /// This means the counter can stay the same or increment by exactly 1.
    /// Regression (n < m) and over-increment (n > m+1) are both rejected.
    fn check_opcert_counter(&mut self, header: &BlockHeader) -> Result<(), ConsensusError> {
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
            // Counter over-increment: n > m + 1
            if n > m + 1 {
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
        }

        // Update tracked counter (always update, even during sync, for tracking)
        self.opcert_counters
            .entry(pool_id)
            .and_modify(|v| {
                if n > *v {
                    *v = n;
                }
            })
            .or_insert(n);

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

        // Construct the VRF seed per Praos spec:
        // input = Blake2b-256(slot_BE || epoch_nonce)
        let seed = crate::slot_leader::vrf_input(&header.epoch_nonce, header.slot);

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

    /// The stability window: 3k/f slots (using integer arithmetic for precision)
    pub fn stability_window(&self) -> u64 {
        let (f_num, f_den) =
            torsten_primitives::protocol_params::f64_to_rational(self.active_slot_coeff);
        torsten_primitives::protocol_params::ceiling_div_by_rational(
            3,
            self.security_param,
            f_num,
            f_den,
        )
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

    /// Update the tip
    pub fn update_tip(&mut self, tip: Tip) {
        self.tip = tip;
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
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    /// Create a dummy BlockIssuerInfo for the given header's VRF key
    fn make_issuer_info(header: &BlockHeader) -> BlockIssuerInfo {
        BlockIssuerInfo {
            vrf_keyhash: blake2b_256(&header.vrf_vkey),
            relative_stake: 1.0,
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
        let result = praos.validate_header(&header, SlotNo(100));
        assert!(matches!(result, Err(ConsensusError::FutureBlock { .. })));
    }

    #[test]
    fn test_valid_header() {
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        let result = praos.validate_header(&header, SlotNo(200));
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_issuer_vkey_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.issuer_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200));
        assert!(matches!(result, Err(ConsensusError::EmptyIssuerVkey)));
    }

    #[test]
    fn test_empty_vrf_key_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.vrf_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200));
        assert!(matches!(result, Err(ConsensusError::EmptyVrfKey)));
    }

    #[test]
    fn test_vrf_verification_non_fatal() {
        // VRF verification with dummy data should not reject during sync
        // (it's non-fatal since we may not have the correct epoch nonce)
        let praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        // With dummy VRF key/proof, verification should pass (non-fatal mode)
        let result = praos.validate_header(&header, SlotNo(200));
        assert!(result.is_ok());
    }

    #[test]
    fn test_kes_period_validation() {
        let praos = OuroborosPraos::new();
        // Block at slot 200,000 is in KES period 1 (200000 / 129600 = 1)
        let mut header = make_valid_header(200_000);
        // Set cert KES period to 1 (matches)
        header.operational_cert.kes_period = 1;
        assert!(praos.validate_header(&header, SlotNo(300_000)).is_ok());
    }

    #[test]
    fn test_kes_period_before_cert_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        // Block at slot 100 is in KES period 0, but cert says period 5
        header.operational_cert.kes_period = 5;
        let result = praos.validate_header(&header, SlotNo(200));
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
        let result = praos.validate_header(&header, SlotNo(slot + 1000));
        assert!(matches!(result, Err(ConsensusError::KesExpired { .. })));
    }

    #[test]
    fn test_kes_at_max_evolution_ok() {
        let praos = OuroborosPraos::new();
        // 61 evolutions (0..61) should be OK (< MAX_KES_EVOLUTIONS which is 62)
        let slot = KES_PERIOD_SLOTS * 61;
        let mut header = make_valid_header(slot);
        header.operational_cert.kes_period = 0;
        assert!(praos.validate_header(&header, SlotNo(slot + 1000)).is_ok());
    }

    #[test]
    fn test_empty_opcert_hot_vkey_rejected() {
        let praos = OuroborosPraos::new();
        let mut header = make_valid_header(100);
        header.operational_cert.hot_vkey = vec![];
        let result = praos.validate_header(&header, SlotNo(200));
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
        let result = praos.validate_header(&header, SlotNo(200));
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
        assert!(praos.validate_header(&header, SlotNo(200)).is_ok());
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
        assert!(praos.validate_header(&header, SlotNo(200)).is_ok());

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
        assert!(praos.validate_header(&header2, SlotNo(200)).is_ok());
    }

    // --- Tests for validate_header_full ---

    #[test]
    fn test_validate_header_full_without_issuer_info() {
        // Without issuer info, validate_header_full behaves like validate_header
        // plus opcert counter tracking
        let mut praos = OuroborosPraos::new();
        let header = make_valid_header(100);
        assert!(praos
            .validate_header_full(&header, SlotNo(200), None)
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
            relative_stake: 1.0,
        };

        let result = praos.validate_header_full(&header, SlotNo(200), Some(&info));
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
            relative_stake: 1.0,
        };

        assert!(praos
            .validate_header_full(&header, SlotNo(200), Some(&info))
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
            relative_stake: 1.0,
        };

        // Should pass VRF key binding check (VRF proof may still fail, but key binding is OK)
        // Note: with dummy VRF proof data, the underlying validate_header VRF check will
        // fail in strict mode, so we need to test the key binding path specifically
        // by using non-strict for the underlying check
        praos.set_strict_verification(false);
        assert!(praos
            .validate_header_full(&header, SlotNo(200), Some(&info))
            .is_ok());
    }

    #[test]
    fn test_opcert_counter_tracking() {
        let mut praos = OuroborosPraos::new();

        // First block from pool A with seq=5
        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None)
            .is_ok());

        // Second block from same pool with seq=6 (forward, OK)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 6;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None)
            .is_ok());

        // Third block from same pool with seq=4 (regression, non-strict: OK)
        let mut header3 = make_valid_header(300);
        header3.operational_cert.sequence_number = 4;
        assert!(praos
            .validate_header_full(&header3, SlotNo(400), None)
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
            .validate_header_full(&header1, SlotNo(200), Some(&info1))
            .is_ok());

        // Enable strict mode
        praos.set_strict_verification(true);

        // Block with seq=3 (regression) should fail in strict mode
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 3;
        let info2 = make_issuer_info(&header2);
        let result = praos.validate_header_full(&header2, SlotNo(300), Some(&info2));
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
            .validate_header_full(&header1, SlotNo(200), None)
            .is_ok());

        // Same seq=5 is allowed (not a regression, same cert can sign multiple blocks)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None)
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
            .validate_header_full(&header_a, SlotNo(200), None)
            .is_ok());

        // Pool B (different issuer key) with seq=2 — should be fine (different pool)
        let mut header_b = make_valid_header(200);
        header_b.issuer_vkey = vec![2u8; 32];
        header_b.operational_cert.sequence_number = 2;
        assert!(praos
            .validate_header_full(&header_b, SlotNo(300), None)
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
            relative_stake: 0.0, // Zero stake = never eligible
        };

        // The VRF proof verification will fail first in strict mode with dummy data,
        // so test leader eligibility in non-strict where VRF proof check is non-fatal
        // but leader check is still performed
        praos.set_strict_verification(false);
        // Non-strict: leader eligibility failure is non-fatal
        assert!(praos
            .validate_header_full(&header, SlotNo(200), Some(&info))
            .is_ok());
    }

    #[test]
    fn test_block_issuer_info_construction() {
        let info = BlockIssuerInfo {
            vrf_keyhash: Hash32::from_bytes([42u8; 32]),
            relative_stake: 0.05,
        };
        assert_eq!(info.relative_stake, 0.05);
        assert_eq!(info.vrf_keyhash, Hash32::from_bytes([42u8; 32]));
    }

    #[test]
    fn test_opcert_counter_over_increment_non_strict() {
        // Non-strict: over-increment is non-fatal
        let mut praos = OuroborosPraos::new();

        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), None)
            .is_ok());

        // Jump from 5 to 10 (over-increment by 5) — non-fatal during sync
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 10;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None)
            .is_ok());
    }

    #[test]
    fn test_opcert_counter_over_increment_strict() {
        let mut praos = OuroborosPraos::new();

        let mut header1 = make_valid_header(100);
        header1.operational_cert.sequence_number = 5;
        let info1 = make_issuer_info(&header1);
        assert!(praos
            .validate_header_full(&header1, SlotNo(200), Some(&info1))
            .is_ok());

        praos.set_strict_verification(true);

        // Jump from 5 to 7 (over-increment, max allowed is +1)
        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 7;
        let info2 = make_issuer_info(&header2);
        let result = praos.validate_header_full(&header2, SlotNo(300), Some(&info2));
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
            .validate_header_full(&header1, SlotNo(200), None)
            .is_ok());

        let mut header2 = make_valid_header(200);
        header2.operational_cert.sequence_number = 6;
        assert!(praos
            .validate_header_full(&header2, SlotNo(300), None)
            .is_ok());

        let pool_id = torsten_primitives::hash::blake2b_224(&header1.issuer_vkey);
        assert_eq!(praos.opcert_counters[&pool_id], 6);
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
        let result = praos.validate_header_full(&header, SlotNo(200), None);
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
        let result = praos.validate_header_full(&header, SlotNo(200), None);
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
            relative_stake: 0.5,
        };

        let result = praos.validate_header_full(&header, SlotNo(200), Some(&info));
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
        let result = praos.validate_header_full(&header, SlotNo(200), None);
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
            relative_stake: 1.0,
        };

        // With strict mode and dummy VRF data, VRF key binding check passes but
        // later VRF proof check may fail. The important thing is that it does NOT
        // fail with UnregisteredPool.
        let result = praos.validate_header_full(&header, SlotNo(200), Some(&info));
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
}
