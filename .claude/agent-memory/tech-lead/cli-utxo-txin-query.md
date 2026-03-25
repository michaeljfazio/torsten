---
name: query utxo --tx-in support
description: Added GetUTxOByTxIn (tag 15) to n2c_client and CLI --tx-in flag to query utxo
type: project
---

`query utxo` now supports both `--address` and `--tx-in` flags.

**Why:** cardano-cli users often need to inspect specific UTxOs by tx reference (tx_hash#index) rather than scanning all UTxOs at an address. The server-side handler for GetUTxOByTxIn (tag 15) was already implemented in `query_handler/utxo.rs` — only the client call and CLI flag were missing.

**How to apply:** Use `--tx-in <txhash>#<index>` (repeatable). The flag is optional alongside `--address`; at least one of the two must be provided. Both can be provided simultaneously.

**Implementation details:**
- `N2CClient::query_utxo_by_txin(&[(Vec<u8>, u32)])` in `n2c_client.rs` encodes `tag(258) Set<[tx_hash, index]>` as the query argument for Shelley tag 15.
- CLI `QuerySubcommand::Utxo` changed `address: String` to `address: Option<String>` + `tx_in: Vec<String>`.
- UTxO result printing extracted into `print_utxo_result(&[u8])` helper to avoid duplication.
- The UTxO wire format response is identical to GetUTxOByAddress, so the same parser works for both.
