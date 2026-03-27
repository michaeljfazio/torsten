---
name: transaction build --change-address does not append change output
description: torsten-cli `transaction build` with --change-address computes change correctly but does NOT add a change output to the transaction body when change > 0
type: project
---

**Bug:** When `torsten-cli transaction build` computes a non-zero change amount (e.g. inputs=2000000, outputs=1500000, fee=165281, change=334719), the change output is NOT appended to the tx body. The tx body only contains the explicitly specified --tx-out outputs. The node correctly rejects this with `ValueNotConserved`.

**Workaround:** Set --tx-out to exactly `input - fee` so change=0. Example: 2000000 - 165281 = 1834719 lovelace.

**Root cause:** In `crates/torsten-cli/src/commands/transaction/build.rs`, after computing change, the code logs "change = N lovelace" but the change output construction and insertion into the tx body is either missing or has a bug.

**How to apply:** Before soak testing, always use exact amounts. The proper fix is to add a change TxOut to the outputs list when change > 0.
