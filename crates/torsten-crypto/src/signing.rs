use torsten_primitives::hash::{blake2b_256, Hash32, TransactionHash};

/// Compute the transaction hash (hash of the serialized transaction body)
pub fn hash_transaction(tx_body_cbor: &[u8]) -> TransactionHash {
    blake2b_256(tx_body_cbor)
}

/// Compute the hash of a block header
pub fn hash_block_header(header_cbor: &[u8]) -> Hash32 {
    blake2b_256(header_cbor)
}
