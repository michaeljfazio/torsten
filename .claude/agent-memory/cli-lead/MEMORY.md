# CLI Lead Agent Memory

## TUI
- [TUI polish](project_tui_polish.md) — layout.rs/ui.rs rewrite: Wide mode, aligned kv rows, Monokai default, colored RTT bar, no info duplication

## Command Coverage
- [transaction build-raw alias](project_build_raw_alias.md) — `build-raw` added as a cardano-cli compatible alias for `transaction build`
- [query utxo --tx-in support](project_utxo_txin.md) — `query utxo` now accepts `--tx-in tx_hash#index` for GetUTxOByTxIn (tag 15)
- [stake-address-info credential filtering](project_stake_address_info.md) — `query stake-address-info` now passes credential server-side; reward balance already returned
