//! Debug query handlers: epoch/chain state dumps and GetCBOR wrapper.
//!
//! Tags handled (via `QueryHandler::handle_query_cbor`):
//!   - 8  DebugEpochState        → `QueryResult::DebugEpochState`
//!   - 9  GetCBOR                → `QueryResult::WrappedCbor` (tag(24) wrapper)
//!   - 12 DebugNewEpochState     → `QueryResult::DebugNewEpochState`
//!   - 13 DebugChainDepState     → `QueryResult::DebugChainDepState`
//!
//! The actual CBOR serialization lives in `encoding.rs`; this module
//! documents which `QueryResult` variants correspond to which protocol tags.
//!
//! ## GetCBOR (tag 9)
//!
//! GetCBOR wraps an inner query in `tag(24)` (embedded CBOR).  The inner
//! query is evaluated first, its result value is encoded to bytes, and
//! those bytes are then encoded as a CBOR byte-string tagged with 24.
//!
//! The HFC wrapper still applies to the outer result — only the inner
//! value (without MsgResult or HFC) is serialized as the embedded CBOR.
