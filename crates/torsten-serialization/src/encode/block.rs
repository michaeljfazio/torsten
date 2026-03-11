use crate::cbor::*;
use torsten_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::hash::{blake2b_256, Hash32};
use torsten_primitives::transaction::Transaction;

use super::transaction::{encode_auxiliary_data, encode_transaction_body, encode_witness_set};

/// Encode an operational certificate: [hot_vkey, sequence_number, kes_period, sigma]
pub fn encode_operational_cert(cert: &OperationalCert) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_bytes(&cert.hot_vkey));
    buf.extend(encode_uint(cert.sequence_number));
    buf.extend(encode_uint(cert.kes_period));
    buf.extend(encode_bytes(&cert.sigma));
    buf
}

/// Encode a VRF result: [output, proof]
pub fn encode_vrf_result(vrf: &VrfOutput) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_bytes(&vrf.output));
    buf.extend(encode_bytes(&vrf.proof));
    buf
}

/// Encode a protocol version: [major, minor]
pub fn encode_protocol_version(pv: &ProtocolVersion) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(pv.major));
    buf.extend(encode_uint(pv.minor));
    buf
}

/// Encode a block header body (the part that gets signed by KES).
///
/// [block_number, slot, prev_hash, issuer_vkey, vrf_vkey, vrf_result,
///  body_size, body_hash, operational_cert, protocol_version]
pub fn encode_block_header_body(header: &BlockHeader) -> Vec<u8> {
    let mut buf = encode_array_header(10);
    buf.extend(encode_uint(header.block_number.0));
    buf.extend(encode_uint(header.slot.0));
    buf.extend(encode_hash32(&header.prev_hash));
    buf.extend(encode_bytes(&header.issuer_vkey));
    buf.extend(encode_bytes(&header.vrf_vkey));
    buf.extend(encode_vrf_result(&header.vrf_result));
    buf.extend(encode_uint(header.body_size));
    buf.extend(encode_hash32(&header.body_hash));
    buf.extend(encode_operational_cert(&header.operational_cert));
    buf.extend(encode_protocol_version(&header.protocol_version));
    buf
}

/// Encode a complete block header: [header_body, body_signature]
///
/// The `kes_signature` parameter is the KES signature over the header body.
pub fn encode_block_header(header: &BlockHeader, kes_signature: &[u8]) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_block_header_body(header));
    buf.extend(encode_bytes(kes_signature));
    buf
}

/// Encode a complete Babbage/Conway era block.
///
/// Block = [era_tag, [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]]
///
/// For Babbage (era 6) and Conway (era 7), blocks are wrapped with era tag.
/// The `kes_signature` is the KES signature for the block header.
pub fn encode_block(block: &Block, kes_signature: &[u8]) -> Vec<u8> {
    let era_tag = match block.era {
        torsten_primitives::era::Era::Byron => 0u64,
        torsten_primitives::era::Era::Shelley => 2,
        torsten_primitives::era::Era::Allegra => 3,
        torsten_primitives::era::Era::Mary => 4,
        torsten_primitives::era::Era::Alonzo => 5,
        torsten_primitives::era::Era::Babbage => 6,
        torsten_primitives::era::Era::Conway => 7,
    };

    // Outer array: [era_tag, block_content]
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(era_tag));

    // Block content: [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]
    buf.extend(encode_array_header(5));

    // Header
    buf.extend(encode_block_header(&block.header, kes_signature));

    // Transaction bodies
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        buf.extend(encode_transaction_body(&tx.body));
    }

    // Transaction witness sets
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        buf.extend(encode_witness_set(&tx.witness_set));
    }

    // Auxiliary data map: {tx_index: aux_data}
    let aux_entries: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter_map(|(i, tx)| tx.auxiliary_data.as_ref().map(|aux| (i, aux)))
        .collect();
    buf.extend(encode_map_header(aux_entries.len()));
    for (idx, aux) in &aux_entries {
        buf.extend(encode_uint(*idx as u64));
        buf.extend(encode_auxiliary_data(aux));
    }

    // Invalid transactions (indices of txs with is_valid=false)
    let invalid_indices: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| !tx.is_valid)
        .map(|(i, _)| i)
        .collect();
    buf.extend(encode_array_header(invalid_indices.len()));
    for idx in &invalid_indices {
        buf.extend(encode_uint(*idx as u64));
    }

    buf
}

/// Compute the block body hash using the Alonzo+ segregated witness structure.
///
/// Per Haskell cardano-ledger, the block body hash is:
///   blake2b_256(h1 || h2 || h3 || h4)
/// where:
///   h1 = blake2b_256(CBOR array of transaction bodies)
///   h2 = blake2b_256(CBOR array of witness sets)
///   h3 = blake2b_256(CBOR map of {tx_index: auxiliary_data})
///   h4 = blake2b_256(CBOR array of invalid tx indices)
pub fn compute_block_body_hash(transactions: &[Transaction]) -> Hash32 {
    // 1. Transaction bodies
    let mut bodies_cbor = encode_array_header(transactions.len());
    for tx in transactions {
        bodies_cbor.extend(encode_transaction_body(&tx.body));
    }
    let h1 = blake2b_256(&bodies_cbor);

    // 2. Transaction witness sets
    let mut wits_cbor = encode_array_header(transactions.len());
    for tx in transactions {
        wits_cbor.extend(encode_witness_set(&tx.witness_set));
    }
    let h2 = blake2b_256(&wits_cbor);

    // 3. Auxiliary data map: {tx_index: aux_data} for txs that have auxiliary data
    let aux_entries: Vec<_> = transactions
        .iter()
        .enumerate()
        .filter_map(|(i, tx)| tx.auxiliary_data.as_ref().map(|aux| (i, aux)))
        .collect();
    let mut aux_cbor = encode_map_header(aux_entries.len());
    for (idx, aux) in &aux_entries {
        aux_cbor.extend(encode_uint(*idx as u64));
        aux_cbor.extend(encode_auxiliary_data(aux));
    }
    let h3 = blake2b_256(&aux_cbor);

    // 4. Invalid transaction indices (txs with is_valid=false)
    let invalid_indices: Vec<_> = transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| !tx.is_valid)
        .map(|(i, _)| i)
        .collect();
    let mut isvalid_cbor = encode_array_header(invalid_indices.len());
    for idx in &invalid_indices {
        isvalid_cbor.extend(encode_uint(*idx as u64));
    }
    let h4 = blake2b_256(&isvalid_cbor);

    // Combine: blake2b_256(h1 || h2 || h3 || h4)
    let mut combined = Vec::with_capacity(128);
    combined.extend_from_slice(h1.as_bytes());
    combined.extend_from_slice(h2.as_bytes());
    combined.extend_from_slice(h3.as_bytes());
    combined.extend_from_slice(h4.as_bytes());
    blake2b_256(&combined)
}
