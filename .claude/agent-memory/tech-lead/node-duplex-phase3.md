---
name: duplex-phase3-integration
description: How DuplexPeerConnection integrates into the sync loop (Phase 3 pattern)
type: project
---

Phase 3 integrates DuplexPeerConnection (full-duplex N2N, InitiatorAndResponder) into the primary sync connection.

**Why:** Outbound connections were initiator-only — the remote peer could not pull our mempool txs. DuplexPeerConnection adds a TxSubmission2 responder task so peers can request our mempool via the same TCP connection.

**Architecture of the conversion:**

The sync loop takes `Option<PipelinedPeerClient>`. Rather than change the loop signature, DuplexPeerConnection converts to PipelinedPeerClient via:
- `DuplexPeerConnection::into_pipelined()` — in `duplex.rs`, has private field access
- Calls `PipelinedPeerClient::from_duplex_parts()` — `pub(crate)` constructor in `pipelined.rs`
- Returns `(PipelinedPeerClient, JoinHandle<()>)` — caller must keep the JoinHandle alive

**Key invariant:** The `_txsub_responder_handle` local variable in `mod.rs` must live as long as `pipelined_client`. Both are declared in the same scope block before `chain_sync_loop()` is called. Dropping the handle aborts the TxSubmission2 responder task — correct behavior on disconnect.

**Fallback:** If DuplexPeerConnection::connect() fails (timeout, peer refuses InitiatorAndResponder), falls back to PipelinedPeerClient::connect() (InitiatorOnly) and spawns a TxSubmission2 CLIENT instead (so we still receive mempool txs from the peer).

**Metric updates:**
- `peers_duplex` is incremented via `pm.duplex_peer_count()` after successful duplex connect
- Refreshed after `peer_disconnected()` (clears duplex flag) and after `peer_failed()` (also clears duplex flag)

**PeerInfo.duplex field:** Added to `peer_manager.rs`. Cleared by both `peer_disconnected()` and `peer_failed()`. `PeerManager::duplex_peer_count()` counts active duplex connections for the metric.

**How to apply:** When adding features that need to run on the primary sync connection, follow this same pattern: add a method to DuplexPeerConnection, keep the JoinHandle in scope alongside pipelined_client.
