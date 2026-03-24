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
    // Try tag(258) Set wrapper first, fall back to bare array
    let pos = decoder.position();
    if decoder.tag().is_err() {
        decoder.set_position(pos);
    }
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            if let Ok(Some(_)) = decoder.array() {
                if let (Ok(tx_hash), Ok(idx)) = (decoder.bytes(), decoder.u32()) {
                    inputs.push((tx_hash.to_vec(), idx));
                } else {
                    debug!("Skipping malformed TxIn entry in GetUTxOByTxIn");
                }
            }
        }
    }
    if let Some(provider) = utxo_provider {
        QueryResult::UtxoByAddress(provider.utxos_by_tx_inputs(&inputs))
    } else {
        QueryResult::UtxoByAddress(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockUtxoProvider {
        utxos: Vec<super::super::types::UtxoSnapshot>,
    }

    impl UtxoQueryProvider for MockUtxoProvider {
        fn utxos_at_address_bytes(
            &self,
            addr_bytes: &[u8],
        ) -> Vec<super::super::types::UtxoSnapshot> {
            self.utxos
                .iter()
                .filter(|u| u.address_bytes == addr_bytes)
                .cloned()
                .collect()
        }

        fn utxos_by_tx_inputs(
            &self,
            inputs: &[(Vec<u8>, u32)],
        ) -> Vec<super::super::types::UtxoSnapshot> {
            self.utxos
                .iter()
                .filter(|u| {
                    inputs
                        .iter()
                        .any(|(h, i)| h == &u.tx_hash && *i == u.output_index)
                })
                .cloned()
                .collect()
        }
    }

    fn make_utxo(
        tx_hash: Vec<u8>,
        index: u32,
        addr: Vec<u8>,
        lovelace: u64,
    ) -> super::super::types::UtxoSnapshot {
        super::super::types::UtxoSnapshot {
            tx_hash,
            output_index: index,
            address_bytes: addr,
            lovelace,
            multi_asset: vec![],
            datum_hash: None,
            raw_cbor: None,
        }
    }

    fn make_provider(
        utxos: Vec<super::super::types::UtxoSnapshot>,
    ) -> Option<Arc<dyn UtxoQueryProvider>> {
        Some(Arc::new(MockUtxoProvider { utxos }))
    }

    #[test]
    fn test_utxo_by_address_single() {
        let addr = vec![0x61; 29]; // enterprise address
        let provider = make_provider(vec![
            make_utxo(vec![1u8; 32], 0, addr.clone(), 5_000_000),
            make_utxo(vec![2u8; 32], 1, vec![0x62; 29], 3_000_000),
        ]);
        // Encode single address bytes
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).bytes(&addr).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let state = super::super::types::NodeStateSnapshot::default();
        let result = handle_utxo_by_address(&state, &provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => {
                assert_eq!(utxos.len(), 1);
                assert_eq!(utxos[0].lovelace, 5_000_000);
            }
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_address_set() {
        let addr1 = vec![0x61; 29];
        let addr2 = vec![0x62; 29];
        let provider = make_provider(vec![
            make_utxo(vec![1u8; 32], 0, addr1.clone(), 5_000_000),
            make_utxo(vec![2u8; 32], 0, addr2.clone(), 3_000_000),
        ]);
        // Encode tag(258) Set<Address>
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.tag(minicbor::data::Tag::new(258)).ok();
        enc.array(2).ok();
        enc.bytes(&addr1).ok();
        enc.bytes(&addr2).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let state = super::super::types::NodeStateSnapshot::default();
        let result = handle_utxo_by_address(&state, &provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => {
                assert_eq!(utxos.len(), 2);
            }
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_address_no_provider() {
        let addr = vec![0x61; 29];
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).bytes(&addr).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let state = super::super::types::NodeStateSnapshot::default();
        let result = handle_utxo_by_address(&state, &None, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => assert!(utxos.is_empty()),
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_address_empty_result() {
        let addr = vec![0xFF; 29]; // address not in set
        let provider = make_provider(vec![make_utxo(vec![1u8; 32], 0, vec![0x61; 29], 5_000_000)]);
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).bytes(&addr).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let state = super::super::types::NodeStateSnapshot::default();
        let result = handle_utxo_by_address(&state, &provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => assert!(utxos.is_empty()),
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_whole_returns_empty() {
        let result = handle_utxo_whole();
        match result {
            QueryResult::UtxoByAddress(utxos) => assert!(utxos.is_empty()),
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_txin_single() {
        let tx_hash = vec![0xAA; 32];
        let provider = make_provider(vec![
            make_utxo(tx_hash.clone(), 0, vec![0x61; 29], 5_000_000),
            make_utxo(vec![0xBB; 32], 1, vec![0x62; 29], 3_000_000),
        ]);
        // Encode array(1) [ [tx_hash, 0] ]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.array(2).ok();
        enc.bytes(&tx_hash).ok();
        enc.u32(0).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_utxo_by_txin(&provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => {
                assert_eq!(utxos.len(), 1);
                assert_eq!(utxos[0].tx_hash, tx_hash);
            }
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_txin_no_provider() {
        let mut buf = Vec::new();
        minicbor::Encoder::new(&mut buf).array(0).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_utxo_by_txin(&None, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => assert!(utxos.is_empty()),
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_txin_not_found() {
        let provider = make_provider(vec![make_utxo(
            vec![0xAA; 32],
            0,
            vec![0x61; 29],
            5_000_000,
        )]);
        // Query for a TxIn that doesn't exist
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).ok();
        enc.array(2).ok();
        enc.bytes(&[0xFF; 32]).ok();
        enc.u32(99).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_utxo_by_txin(&provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => assert!(utxos.is_empty()),
            _ => panic!("Expected UtxoByAddress"),
        }
    }

    #[test]
    fn test_utxo_by_txin_multiple() {
        let tx1 = vec![0xAA; 32];
        let tx2 = vec![0xBB; 32];
        let provider = make_provider(vec![
            make_utxo(tx1.clone(), 0, vec![0x61; 29], 5_000_000),
            make_utxo(tx2.clone(), 1, vec![0x62; 29], 3_000_000),
            make_utxo(vec![0xCC; 32], 2, vec![0x63; 29], 1_000_000),
        ]);
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).ok();
        enc.array(2).ok();
        enc.bytes(&tx1).ok();
        enc.u32(0).ok();
        enc.array(2).ok();
        enc.bytes(&tx2).ok();
        enc.u32(1).ok();
        let mut dec = minicbor::Decoder::new(&buf);

        let result = handle_utxo_by_txin(&provider, &mut dec);
        match result {
            QueryResult::UtxoByAddress(utxos) => {
                assert_eq!(utxos.len(), 2);
            }
            _ => panic!("Expected UtxoByAddress"),
        }
    }
}
