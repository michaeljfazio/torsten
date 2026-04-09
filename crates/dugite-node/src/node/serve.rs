//! N2N and N2C server setup: connection-facing adapters that bridge the node's
//! internal state (ChainDB, LedgerState, Mempool) to the network protocol
//! implementations in dugite-network.

use std::sync::Arc;
use tokio::sync::RwLock;

use dugite_ledger::LedgerState;
use dugite_network::{
    BlockProvider, TipInfo, TxValidationError, TxValidator, UtxoQueryProvider, UtxoSnapshot,
};
use dugite_storage::ChainDB;

// ─── ChainDBBlockProvider ────────────────────────────────────────────────────

/// Provides block data from ChainDB for the N2N server.
pub(crate) struct ChainDBBlockProvider {
    pub chain_db: Arc<RwLock<ChainDB>>,
}

impl BlockProvider for ChainDBBlockProvider {
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let block_hash = dugite_primitives::hash::Hash32::from_bytes(*hash);
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            db.get_block(&block_hash).ok().flatten()
        })
    }

    fn has_block(&self, hash: &[u8; 32]) -> bool {
        let block_hash = dugite_primitives::hash::Hash32::from_bytes(*hash);
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            db.has_block(&block_hash)
        })
    }

    fn get_tip(&self) -> TipInfo {
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            let tip = db.get_tip();
            let slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
            let hash = tip
                .point
                .hash()
                .map(|h| {
                    let bytes: &[u8] = h.as_ref();
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(bytes);
                    arr
                })
                .unwrap_or([0u8; 32]);
            let block_no = tip.block_number.0;
            TipInfo {
                slot,
                hash,
                block_number: block_no,
            }
        })
    }

    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            let slot = dugite_primitives::time::SlotNo(after_slot);
            match db.get_next_block_after_slot(slot) {
                Ok(Some((s, hash, cbor))) => {
                    let mut hash_arr = [0u8; 32];
                    hash_arr.copy_from_slice(hash.as_bytes());
                    Some((s.0, hash_arr, cbor))
                }
                _ => None,
            }
        })
    }

    fn get_block_at_or_after_slot(&self, slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            let slot_no = dugite_primitives::time::SlotNo(slot);
            match db.get_block_at_or_after_slot(slot_no) {
                Ok(Some((s, hash, cbor))) => {
                    let mut hash_arr = [0u8; 32];
                    hash_arr.copy_from_slice(hash.as_bytes());
                    Some((s.0, hash_arr, cbor))
                }
                _ => None,
            }
        })
    }

    /// Collect blocks in [`from_slot`, `to_slot`] with chunked lock acquisition.
    ///
    /// # Why this override is critical
    ///
    /// The default trait implementation calls `get_next_block_after_slot()` in a
    /// loop, each of which does `block_in_place(|| chain_db.blocking_read())`.
    /// For a batch of N blocks that means N separate lock-acquire/release cycles —
    /// each one parks the calling tokio worker thread until the lock is available.
    /// When `ChainSelQueue` holds `chain_db.write()` during block storage, all N
    /// parked threads stack up and starve the async worker pool, freezing the
    /// metrics endpoint and slowing the main run loop.
    ///
    /// # Chunked locking strategy
    ///
    /// We do NOT hold the lock for the entire batch because
    /// `ImmutableDB::get_next_block_after_slot()` performs synchronous disk I/O
    /// (reads `.secondary` index + `.chunk` data files).  Holding the read lock
    /// for 2000 sequential disk reads would block `ChainSelQueue.write()` for
    /// seconds and stall the main sync loop.
    ///
    /// Instead, we acquire the read lock in chunks of `BATCH_CHUNK_SIZE` blocks.
    /// This reduces lock overhead by ~50× compared to per-block locking while
    /// keeping the critical section short enough (≈50 disk reads ≈ a few ms)
    /// for the writer to make progress between chunks.
    fn get_blocks_in_range(
        &self,
        from_slot: u64,
        to_slot: u64,
        limit: usize,
    ) -> Vec<(u64, [u8; 32], Vec<u8>)> {
        /// Number of blocks to collect per lock acquisition.  Each block read
        /// may hit disk (ImmutableDB), so keep this small enough that the
        /// ChainSelQueue writer is never starved for more than a few ms.
        const BATCH_CHUNK_SIZE: usize = 50;

        tokio::task::block_in_place(|| {
            let mut blocks = Vec::new();
            let mut current_slot = from_slot;
            let mut first = true;

            while current_slot <= to_slot && blocks.len() < limit {
                // Acquire the read lock for a chunk of blocks.
                let db = self.chain_db.blocking_read();
                let chunk_limit = BATCH_CHUNK_SIZE.min(limit - blocks.len());

                for _ in 0..chunk_limit {
                    if current_slot > to_slot {
                        break;
                    }
                    let slot_no = dugite_primitives::time::SlotNo(current_slot);
                    let result = if first {
                        first = false;
                        db.get_block_at_or_after_slot(slot_no)
                    } else {
                        db.get_next_block_after_slot(slot_no)
                    };
                    match result {
                        Ok(Some((s, hash, cbor))) if s.0 <= to_slot => {
                            let mut hash_arr = [0u8; 32];
                            hash_arr.copy_from_slice(hash.as_bytes());
                            current_slot = s.0;
                            blocks.push((s.0, hash_arr, cbor));
                        }
                        _ => return blocks, // No more blocks — done
                    }
                }
                // Read lock dropped here; ChainSelQueue writer can proceed.
            }
            blocks
        })
    }
}

// ─── LedgerUtxoProvider ──────────────────────────────────────────────────────

/// Provides UTxO lookups from the live ledger state.
pub(crate) struct LedgerUtxoProvider {
    pub ledger: Arc<RwLock<LedgerState>>,
}

impl UtxoQueryProvider for LedgerUtxoProvider {
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot> {
        let addr = match dugite_primitives::address::Address::from_bytes(addr_bytes) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(
                    "UTxO query: address decode failed: {e} (bytes len={})",
                    addr_bytes.len()
                );
                return vec![];
            }
        };
        // Use block_in_place + blocking_read so this works correctly even when
        // called from within a tokio async runtime (avoids "cannot block" panic).
        tokio::task::block_in_place(|| {
            let ledger = self.ledger.blocking_read();
            let results: Vec<_> = ledger
                .utxo_set
                .utxos_at_address(&addr)
                .into_iter()
                .map(|(input, output)| utxo_to_snapshot(&input, &output))
                .collect();
            tracing::debug!(
                addr_type = ?std::mem::discriminant(&addr),
                index_size = ledger.utxo_set.address_index_size(),
                utxos_found = results.len(),
                "UTxO query by address"
            );
            results
        })
    }

    fn utxos_by_tx_inputs(&self, inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        tokio::task::block_in_place(|| {
            let ledger = self.ledger.blocking_read();
            let mut results = Vec::new();
            for (tx_hash_bytes, idx) in inputs {
                if tx_hash_bytes.len() == 32 {
                    let mut hash_arr = [0u8; 32];
                    hash_arr.copy_from_slice(tx_hash_bytes);
                    let tx_input = dugite_primitives::transaction::TransactionInput {
                        transaction_id: dugite_primitives::hash::Hash32::from_bytes(hash_arr),
                        index: *idx,
                    };
                    if let Some(output) = ledger.utxo_set.lookup(&tx_input) {
                        results.push(utxo_to_snapshot(&tx_input, &output));
                    }
                }
            }
            results
        })
    }

    fn utxos_all(&self) -> Vec<UtxoSnapshot> {
        tokio::task::block_in_place(|| {
            let ledger = self.ledger.blocking_read();
            let results: Vec<_> = ledger
                .utxo_set
                .iter()
                .into_iter()
                .map(|(input, output)| utxo_to_snapshot(&input, &output))
                .collect();
            tracing::debug!(utxos_found = results.len(), "UTxO query: whole set");
            results
        })
    }
}

// ─── LedgerTxValidator ───────────────────────────────────────────────────────

/// Validates transactions against the live ledger state (Phase-1 + Phase-2 Plutus).
///
/// When `mempool` is provided, validation uses a `CompositeUtxoView` that
/// overlays mempool virtual UTxOs on top of the on-chain set.  This enables
/// chained/dependent transaction submission (spending outputs of unconfirmed
/// mempool txs).
pub(crate) struct LedgerTxValidator {
    pub ledger: Arc<RwLock<LedgerState>>,
    pub slot_config: dugite_ledger::plutus::SlotConfig,
    pub metrics: Arc<crate::metrics::NodeMetrics>,
    pub mempool: Option<Arc<dugite_mempool::Mempool>>,
}

impl TxValidator for LedgerTxValidator {
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError> {
        let tx = dugite_serialization::decode_transaction(era_id, tx_bytes).map_err(|e| {
            TxValidationError::DecodeFailed {
                reason: e.to_string(),
            }
        })?;

        let ledger = self
            .ledger
            .try_read()
            .map_err(|_| TxValidationError::LedgerStateUnavailable)?;
        let tx_size = tx_bytes.len() as u64;
        let current_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);

        // Build the UTxO view: on-chain set + optional mempool virtual overlay.
        // This enables chained tx submission (spending unconfirmed mempool outputs).
        let virtual_utxos = self
            .mempool
            .as_ref()
            .map(|mp| mp.virtual_utxo_snapshot())
            .unwrap_or_default();
        let utxo_view =
            dugite_ledger::utxo::CompositeUtxoView::new(&ledger.utxo_set, virtual_utxos);

        dugite_ledger::validation::validate_transaction(
            &tx,
            &utxo_view,
            &ledger.protocol_params,
            current_slot,
            tx_size,
            Some(&self.slot_config),
        )
        .map_err(|errors| {
            // Increment the rejection counter so the TUI and Prometheus show it.
            self.metrics
                .transactions_rejected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            for err in &errors {
                self.metrics.record_validation_error(&format!("{:?}", err));
            }
            let mut mapped: Vec<TxValidationError> =
                errors.into_iter().map(convert_validation_error).collect();
            if mapped.len() == 1 {
                mapped.pop().expect("vec has exactly one element")
            } else {
                TxValidationError::Multiple(mapped)
            }
        })
    }
}

/// Convert a ledger `ValidationError` into the network-facing `TxValidationError`.
pub(crate) fn convert_validation_error(
    e: dugite_ledger::validation::ValidationError,
) -> TxValidationError {
    use dugite_ledger::validation::ValidationError as VE;
    match e {
        VE::NoInputs => TxValidationError::NoInputs,
        VE::InputNotFound(input) => TxValidationError::InputNotFound { input },
        VE::ValueNotConserved {
            inputs,
            outputs,
            fee,
        } => TxValidationError::ValueNotConserved {
            inputs,
            outputs,
            fee,
        },
        VE::FeeTooSmall { minimum, actual } => TxValidationError::FeeTooSmall { minimum, actual },
        VE::OutputTooSmall { minimum, actual } => {
            TxValidationError::OutputTooSmall { minimum, actual }
        }
        VE::TxTooLarge { maximum, actual } => TxValidationError::TxTooLarge { maximum, actual },
        VE::MissingRequiredSigner(signer) => TxValidationError::MissingRequiredSigner { signer },
        VE::MissingWitness(input) => TxValidationError::MissingWitness { input },
        VE::TtlExpired { current_slot, ttl } => TxValidationError::TtlExpired { current_slot, ttl },
        VE::NotYetValid {
            current_slot,
            valid_from,
        } => TxValidationError::NotYetValid {
            current_slot,
            valid_from,
        },
        VE::ScriptFailed(reason) => TxValidationError::ScriptFailed { reason },
        VE::InsufficientCollateral => TxValidationError::InsufficientCollateral,
        VE::TooManyCollateralInputs { max, actual } => {
            TxValidationError::TooManyCollateralInputs { max, actual }
        }
        VE::CollateralNotFound(input) => TxValidationError::CollateralNotFound { input },
        VE::CollateralHasTokens(input) => TxValidationError::CollateralHasTokens { input },
        VE::CollateralMismatch { declared, computed } => {
            TxValidationError::CollateralMismatch { declared, computed }
        }
        VE::ReferenceInputNotFound(input) => TxValidationError::ReferenceInputNotFound { input },
        VE::ReferenceInputOverlapsInput(input) => {
            TxValidationError::ReferenceInputOverlapsInput { input }
        }
        VE::MultiAssetNotConserved {
            policy,
            input_side,
            output_side,
        } => TxValidationError::MultiAssetNotConserved {
            policy,
            input_side,
            output_side,
        },
        VE::InvalidMint => TxValidationError::InvalidMint,
        VE::ExUnitsExceeded => TxValidationError::ExUnitsExceeded,
        VE::ScriptDataHashMismatch { expected, actual } => {
            TxValidationError::ScriptDataHashMismatch { expected, actual }
        }
        VE::UnexpectedScriptDataHash => TxValidationError::UnexpectedScriptDataHash,
        VE::MissingScriptDataHash => TxValidationError::MissingScriptDataHash,
        VE::DuplicateInput(input) => TxValidationError::DuplicateInput { input },
        VE::NativeScriptFailed => TxValidationError::NativeScriptFailed,
        VE::InvalidWitnessSignature(vkey) => TxValidationError::InvalidWitnessSignature { vkey },
        VE::NetworkMismatch { expected, actual } => TxValidationError::NetworkMismatch {
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        },
        VE::AuxiliaryDataHashWithoutData => TxValidationError::AuxiliaryDataHashWithoutData,
        VE::AuxiliaryDataWithoutHash => TxValidationError::AuxiliaryDataWithoutHash,
        VE::BlockExUnitsExceeded {
            resource,
            limit,
            total,
        } => TxValidationError::BlockExUnitsExceeded {
            resource,
            limit,
            total,
        },
        VE::OutputValueTooLarge { maximum, actual } => {
            TxValidationError::OutputValueTooLarge { maximum, actual }
        }
        VE::MissingRawCbor => TxValidationError::MissingRawCbor,
        VE::MissingSlotConfig => TxValidationError::MissingSlotConfig,
        VE::MissingSpendRedeemer { index } => TxValidationError::MissingSpendRedeemer { index },
        VE::RedeemerIndexOutOfRange { tag, index, max } => {
            TxValidationError::RedeemerIndexOutOfRange { tag, index, max: max as u32 }
        }
        VE::MissingInputWitness(credential) => {
            TxValidationError::MissingInputWitness { credential }
        }
        VE::MissingScriptWitness(credential) => {
            TxValidationError::MissingScriptWitness { credential }
        }
        VE::MissingWithdrawalWitness(credential) => {
            TxValidationError::MissingWithdrawalWitness { credential }
        }
        VE::MissingWithdrawalScriptWitness(credential) => {
            TxValidationError::MissingWithdrawalScriptWitness { credential }
        }
        VE::MissingCertificateWitness(credential) => {
            TxValidationError::MissingCertificateWitness { credential }
        }
        VE::ValueOverflow => TxValidationError::ValueOverflow,
        VE::EraGatingViolation {
            certificate_type,
            required_era,
            current_era,
        } => TxValidationError::ScriptFailed {
            reason: format!(
                "Era gating violation: {certificate_type} requires {required_era}, current era is {current_era}"
            ),
        },
        VE::GovernancePreConway { current_version } => TxValidationError::ScriptFailed {
            reason: format!(
                "Governance features not available pre-Conway (current protocol version: {current_version})"
            ),
        },
        VE::TreasuryValueMismatch { declared, actual } => TxValidationError::ScriptFailed {
            reason: format!("Treasury value mismatch: declared {declared}, actual {actual}"),
        },
        VE::UnelectedCommitteeMember { cold_credential_hash } => TxValidationError::ScriptFailed {
            reason: format!("Unelected committee member: {cold_credential_hash}"),
        },
        VE::MissingRedeemer { tag, index } => TxValidationError::ScriptFailed {
            reason: format!("Missing redeemer for {tag} at index {index}"),
        },
        VE::MissingDatumWitness(datum_hash) => TxValidationError::ScriptFailed {
            reason: format!("Missing datum witness for script-locked input: datum hash {datum_hash}"),
        },
        VE::ExtraDatumWitness(datum_hash) => TxValidationError::ScriptFailed {
            reason: format!("Extra (unreferenced) datum witness in transaction: datum hash {datum_hash}"),
        },
        VE::TxRefScriptSizeTooLarge { actual, limit } => TxValidationError::TxTooLarge {
            // Map to TxTooLarge — closest semantic match for a transaction that
            // exceeds a size-based limit (ppMaxRefScriptSizePerTxG, Conway+).
            maximum: limit,
            actual,
        },
        VE::ZeroWithdrawal { account } => TxValidationError::ScriptFailed {
            reason: format!("Zero withdrawal amount for reward account: {account}"),
        },
        VE::IncorrectWithdrawalAmount {
            account,
            declared,
            actual,
        } => TxValidationError::ScriptFailed {
            reason: format!(
                "Incorrect withdrawal amount for {account}: declared={declared}, actual={actual}"
            ),
        },
        VE::PoolRetirementTooLate {
            retirement_epoch,
            current_epoch,
            e_max,
            ..
        } => TxValidationError::ScriptFailed {
            reason: format!(
                "Pool retirement epoch {retirement_epoch} exceeds max (current {current_epoch} + e_max {e_max})"
            ),
        },
        VE::StakeRegistrationDepositMismatch { declared, expected } => {
            TxValidationError::ScriptFailed {
                reason: format!(
                    "Conway stake registration deposit mismatch: declared={declared}, expected={expected}"
                ),
            }
        }
        VE::StakeKeyHasNonZeroBalance {
            credential_hash,
            balance,
        } => TxValidationError::ScriptFailed {
            reason: format!(
                "Stake deregistration rejected: credential {credential_hash} has non-zero balance ({balance} lovelace)"
            ),
        },
        VE::StakeDeregistrationRefundMismatch { declared, expected } => {
            TxValidationError::ScriptFailed {
                reason: format!(
                    "Conway stake deregistration refund mismatch: declared={declared}, expected={expected}"
                ),
            }
        }
        VE::StakeKeyAlreadyRegistered { credential_hash } => TxValidationError::ScriptFailed {
            reason: format!(
                "Stake registration rejected: credential {credential_hash} is already registered"
            ),
        },
        VE::DelegateePoolNotRegistered { pool_id } => TxValidationError::ScriptFailed {
            reason: format!(
                "Stake delegation rejected: target pool {pool_id} is not registered"
            ),
        },
        VE::DRepAlreadyRegistered { credential_hash } => TxValidationError::ScriptFailed {
            reason: format!(
                "DRep registration rejected: credential {credential_hash} is already registered"
            ),
        },
        VE::DRepIncorrectDeposit { declared, expected } => TxValidationError::ScriptFailed {
            reason: format!(
                "DRep registration rejected: declared deposit {declared} does not match \
                 drep_deposit parameter {expected} (ConwayDRepIncorrectDeposit)"
            ),
        },
        VE::ProposalDepositIncorrect { declared, expected } => TxValidationError::ScriptFailed {
            reason: format!(
                "Governance proposal rejected: declared deposit {declared} does not match \
                 gov_action_deposit parameter {expected} (ProposalDepositIncorrect)"
            ),
        },
        VE::CommitteeHasPreviouslyResigned { cold_credential_hash } => {
            TxValidationError::ScriptFailed {
                reason: format!(
                    "CommitteeHotAuth rejected: cold credential {cold_credential_hash} has previously resigned \
                     (ConwayCommitteeHasPreviouslyResigned)"
                ),
            }
        }
        VE::VrfKeyHashAlreadyRegistered {
            vrf_keyhash,
            existing_pool_id,
        } => TxValidationError::ScriptFailed {
            reason: format!(
                "VRF key {vrf_keyhash} is already registered to pool {existing_pool_id}"
            ),
        },
        VE::StakePoolCostTooLow { actual, minimum } => TxValidationError::ScriptFailed {
            reason: format!(
                "Pool registration rejected: cost {actual} is below minimum pool cost {minimum} \
                 (StakePoolCostTooLowPOOL)"
            ),
        },
        VE::PoolRewardAccountWrongNetwork { expected, actual } => TxValidationError::ScriptFailed {
            reason: format!(
                "Pool registration rejected: reward account network {actual:?} does not match \
                 transaction network {expected:?} (WrongNetworkInTxBody)"
            ),
        },
        VE::AuxiliaryDataHashMismatch => TxValidationError::ScriptFailed {
            reason: "Auxiliary data hash mismatch: declared hash does not match blake2b_256 of \
                     aux data bytes (AuxDataHashMismatch)"
                .to_string(),
        },
        VE::WrongNetworkInOutput { expected, actual } => TxValidationError::ScriptFailed {
            reason: format!(
                "Output address network {actual:?} does not match node network {expected:?} \
                 (WrongNetworkInOutput)"
            ),
        },
        VE::WrongNetworkWithdrawal { expected, actual } => TxValidationError::ScriptFailed {
            reason: format!(
                "Withdrawal reward address network {actual:?} does not match node network \
                 {expected:?} (WrongNetworkWithdrawal)"
            ),
        },
        VE::ConstitutionPolicyMismatch { expected, actual } => TxValidationError::ScriptFailed {
            reason: format!(
                "Governance proposal policy_hash mismatch: constitution requires {expected}, \
                 proposal has {actual} (ConstitutionPolicyMismatch)"
            ),
        },
        VE::UnspendableUTxONoDatumHash { input, language } => TxValidationError::ScriptFailed {
            reason: format!(
                "Script-locked input {input} has no datum hash but uses {language} \
                 (UnspendableUTxONoDatumHash)"
            ),
        },
        VE::WdrlNotDelegatedToDRep { credential_hash } => TxValidationError::ScriptFailed {
            reason: format!(
                "Withdrawal rejected: KeyHash reward account {credential_hash} has no DRep \
                 delegation (ConwayWdrlNotDelegatedToDRep)"
            ),
        },
        VE::MalformedProposal { reason } => TxValidationError::ScriptFailed {
            reason: format!("Governance proposal rejected: malformed PParamsUpdate ({reason})"),
        },
        VE::ExtraRedeemer { tag, index } => TxValidationError::ScriptFailed {
            reason: format!(
                "Extra redeemer with no matching script purpose: tag={tag}, index={index}"
            ),
        },
        VE::ScriptLockedCollateral { inputs } => TxValidationError::ScriptFailed {
            reason: format!("Collateral input(s) at script-locked addresses: {inputs:?}"),
        },
    }
}

// ─── Connection metrics bridges ──────────────────────────────────────────────

/// Bridges N2N server connection events to the node metrics system.
#[allow(dead_code)] // used by networking rewrite
pub(crate) struct N2NConnectionMetrics {
    pub metrics: Arc<crate::metrics::NodeMetrics>,
}

impl dugite_network::ConnectionMetrics for N2NConnectionMetrics {
    fn on_connect(&self) {
        self.metrics
            .n2n_connections_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .n2n_connections_active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_disconnect(&self) {
        self.metrics
            .n2n_connections_active
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_error(&self, label: &str) {
        self.metrics.record_protocol_error(label);
    }
}

/// Bridges N2C server connection events to the node metrics system.
#[allow(dead_code)] // used by networking rewrite
pub(crate) struct N2CConnectionMetrics {
    pub metrics: Arc<crate::metrics::NodeMetrics>,
}

impl dugite_network::ConnectionMetrics for N2CConnectionMetrics {
    fn on_connect(&self) {
        self.metrics
            .n2c_connections_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .n2c_connections_active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_disconnect(&self) {
        self.metrics
            .n2c_connections_active
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn on_error(&self, label: &str) {
        self.metrics.record_protocol_error(label);
    }
}

// ─── UTxO snapshot helper ────────────────────────────────────────────────────

/// Convert a UTxO entry to a snapshot for N2C queries.
pub(crate) fn utxo_to_snapshot(
    input: &dugite_primitives::transaction::TransactionInput,
    output: &dugite_primitives::transaction::TransactionOutput,
) -> UtxoSnapshot {
    let multi_asset: dugite_network::MultiAssetSnapshot = output
        .value
        .multi_asset
        .iter()
        .map(|(policy, assets)| {
            let assets_vec: Vec<(Vec<u8>, u64)> = assets
                .iter()
                .map(|(name, qty)| (name.0.clone(), *qty))
                .collect();
            (policy.as_ref().to_vec(), assets_vec)
        })
        .collect();

    let datum_hash = match &output.datum {
        dugite_primitives::transaction::OutputDatum::DatumHash(h) => Some(h.as_ref().to_vec()),
        _ => None,
    };

    UtxoSnapshot {
        tx_hash: input.transaction_id.as_ref().to_vec(),
        output_index: input.index,
        address_bytes: output.address.to_bytes(),
        lovelace: output.value.coin.0,
        multi_asset,
        datum_hash,
        raw_cbor: output.raw_cbor.clone(),
    }
}
