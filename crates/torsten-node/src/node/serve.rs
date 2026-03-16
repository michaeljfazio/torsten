//! N2N and N2C server setup: connection-facing adapters that bridge the node's
//! internal state (ChainDB, LedgerState, Mempool) to the network protocol
//! implementations in torsten-network.

use std::sync::Arc;
use tokio::sync::RwLock;

use torsten_ledger::LedgerState;
use torsten_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use torsten_network::{BlockProvider, TipInfo, TxValidationError, TxValidator};
use torsten_storage::ChainDB;

// ─── ChainDBBlockProvider ────────────────────────────────────────────────────

/// Provides block data from ChainDB for the N2N server.
pub(crate) struct ChainDBBlockProvider {
    pub chain_db: Arc<RwLock<ChainDB>>,
}

impl BlockProvider for ChainDBBlockProvider {
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
        tokio::task::block_in_place(|| {
            let db = self.chain_db.blocking_read();
            db.get_block(&block_hash).ok().flatten()
        })
    }

    fn has_block(&self, hash: &[u8; 32]) -> bool {
        let block_hash = torsten_primitives::hash::Hash32::from_bytes(*hash);
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
            let slot = torsten_primitives::time::SlotNo(after_slot);
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
}

// ─── LedgerUtxoProvider ──────────────────────────────────────────────────────

/// Provides UTxO lookups from the live ledger state.
pub(crate) struct LedgerUtxoProvider {
    pub ledger: Arc<RwLock<LedgerState>>,
}

impl UtxoQueryProvider for LedgerUtxoProvider {
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot> {
        let addr = match torsten_primitives::address::Address::from_bytes(addr_bytes) {
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
                    let tx_input = torsten_primitives::transaction::TransactionInput {
                        transaction_id: torsten_primitives::hash::Hash32::from_bytes(hash_arr),
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
}

// ─── LedgerTxValidator ───────────────────────────────────────────────────────

/// Validates transactions against the live ledger state (Phase-1 + Phase-2 Plutus).
pub(crate) struct LedgerTxValidator {
    pub ledger: Arc<RwLock<LedgerState>>,
    pub slot_config: torsten_ledger::plutus::SlotConfig,
    pub metrics: Arc<crate::metrics::NodeMetrics>,
}

impl TxValidator for LedgerTxValidator {
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError> {
        let tx = torsten_serialization::decode_transaction(era_id, tx_bytes).map_err(|e| {
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

        torsten_ledger::validation::validate_transaction(
            &tx,
            &ledger.utxo_set,
            &ledger.protocol_params,
            current_slot,
            tx_size,
            Some(&self.slot_config),
        )
        .map_err(|errors| {
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
    e: torsten_ledger::validation::ValidationError,
) -> TxValidationError {
    use torsten_ledger::validation::ValidationError as VE;
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
            TxValidationError::RedeemerIndexOutOfRange { tag, index, max }
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
    }
}

// ─── Connection metrics bridges ──────────────────────────────────────────────

/// Bridges N2N server connection events to the node metrics system.
pub(crate) struct N2NConnectionMetrics {
    pub metrics: Arc<crate::metrics::NodeMetrics>,
}

impl torsten_network::ConnectionMetrics for N2NConnectionMetrics {
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
pub(crate) struct N2CConnectionMetrics {
    pub metrics: Arc<crate::metrics::NodeMetrics>,
}

impl torsten_network::ConnectionMetrics for N2CConnectionMetrics {
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
    input: &torsten_primitives::transaction::TransactionInput,
    output: &torsten_primitives::transaction::TransactionOutput,
) -> UtxoSnapshot {
    let multi_asset: torsten_network::query_handler::MultiAssetSnapshot = output
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
        torsten_primitives::transaction::OutputDatum::DatumHash(h) => Some(h.as_ref().to_vec()),
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
