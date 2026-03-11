//! UTxO-related query handlers (tags 6, 7, 15).

use std::sync::Arc;
use tracing::debug;

use super::types::{NodeStateSnapshot, QueryResult, UtxoQueryProvider};

/// Handle GetUTxOByAddress (tag 6).
///
/// Argument: tag(258) Set<Address> or single address bytes
pub(crate) fn handle_utxo_by_address(
    _state: &NodeStateSnapshot,
    utxo_provider: &Option<Arc<dyn UtxoQueryProvider>>,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetUTxOByAddress");
    let mut addresses: Vec<Vec<u8>> = Vec::new();
    let pos = decoder.position();
    // Try single bare address bytes first (most common case)
    if let Ok(bytes) = decoder.bytes() {
        addresses.push(bytes.to_vec());
    } else {
        // Try tag(258) Set<Address>
        decoder.set_position(pos);
        let _ = decoder.tag(); // consume tag(258)
        if let Ok(Some(n)) = decoder.array() {
            for _ in 0..n {
                if let Ok(bytes) = decoder.bytes() {
                    addresses.push(bytes.to_vec());
                }
            }
        }
    }
    // Fallback: use remaining decoder bytes as raw address
    if addresses.is_empty() {
        decoder.set_position(pos);
        let remaining = &decoder.input()[pos..];
        if !remaining.is_empty() {
            addresses.push(remaining.to_vec());
        }
    }
    if let Some(provider) = utxo_provider {
        let mut all_utxos = Vec::new();
        for addr in &addresses {
            all_utxos.extend(provider.utxos_at_address_bytes(addr));
        }
        QueryResult::UtxoByAddress(all_utxos)
    } else {
        QueryResult::UtxoByAddress(vec![])
    }
}

/// Handle GetUTxOWhole (tag 7).
///
/// Too large to serve in practice -- returns empty.
pub(crate) fn handle_utxo_whole() -> QueryResult {
    debug!("Query: GetUTxOWhole (returning empty)");
    QueryResult::UtxoByAddress(vec![])
}

/// Handle GetUTxOByTxIn (tag 15).
///
/// Argument: Set<TxIn> where TxIn = [tx_hash, output_index]
pub(crate) fn handle_utxo_by_txin(
    utxo_provider: &Option<Arc<dyn UtxoQueryProvider>>,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetUTxOByTxIn");
    let mut inputs = Vec::new();
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            if let Ok(Some(_)) = decoder.array() {
                let tx_hash = decoder.bytes().unwrap_or(&[]).to_vec();
                let idx = decoder.u32().unwrap_or(0);
                inputs.push((tx_hash, idx));
            }
        }
    }
    if let Some(provider) = utxo_provider {
        QueryResult::UtxoByAddress(provider.utxos_by_tx_inputs(&inputs))
    } else {
        QueryResult::UtxoByAddress(vec![])
    }
}
