---
name: transaction build-raw alias
description: build-raw added as a cardano-cli compatible alias for transaction build
type: project
---

`transaction build-raw` was added as a separate subcommand in `TxSubcommand` that delegates to the same `cmd_build()` handler as `transaction build`.

**Why:** Many downstream scripts and tools (especially those originally written against cardano-cli) call `cardano-cli transaction build-raw` rather than `build`. Without this alias, those scripts fail.

**How to apply:** When adding future command aliases, follow the same pattern: extract args into a shared `*Args` struct, add a second variant to the `*Subcommand` enum, share the handler via an `or` pattern in the match (`Variant1(args) | Variant2(args) => handler(args)`).

Implementation location: `crates/dugite-cli/src/commands/transaction.rs`
- `BuildArgs` struct: shared clap `Args` for all build fields
- `TxSubcommand::Build(BuildArgs)` and `TxSubcommand::BuildRaw(BuildArgs)`: the two variants
- `cmd_build(args: BuildArgs) -> Result<()>`: the shared handler function
- Match arm: `TxSubcommand::Build(args) | TxSubcommand::BuildRaw(args) => cmd_build(args)`
