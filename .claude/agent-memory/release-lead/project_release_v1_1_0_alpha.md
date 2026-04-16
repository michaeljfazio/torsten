---
name: v1.1.0-alpha release
description: Release details and blockers for v1.1.0-alpha (2026-04-13)
type: project
---

Released v1.1.0-alpha from commit `155bb3090` (version bump commit) on 2026-04-13.

The milestone commit in this release is `2cb2d152b` (fix(forge,chainsync): four fixes for Haskell peer block acceptance) — first confirmed live block acceptance by a Haskell cardano-node peer (610 blocks initial + 190 across DB-preserving restart, zero HeaderError/UnexpectedBlockNo).

**Why:** Four root-cause block forging/chainsync bugs were fixed enabling Haskell peer acceptance on private testnet. Sufficient scope for a MINOR alpha bump per semver.

**How to apply:** When generating the next changelog, note that v1.0.3-alpha was the previous tag and v1.1.0-alpha covers ~140 commits including the milestone forging fixes, ledger correctness (cstreamer divergences resolved, 10+ validation checks added), EraRules trait dispatch refactor, P2P governor overhaul (#369), and 140+ new tests.

## Known CI issue at release time
The `release-binaries (macos-latest, x86_64-apple-darwin)` job fails with:
> Cross compilation from aarch64-apple-darwin to x86_64-apple-darwin not supported! Use the `force-cross` feature to cross compile anyway.
This is pre-existing (present since v1.0.3-alpha era) — GitHub's macOS runners are now aarch64, but the CI matrix targets x86_64. The fix is to add `force-cross` feature flag to `gmp-mpfr-sys` in the workflow. Does NOT block releases: build-and-test, integration-offline, coverage all pass.

## Release process notes
- `gh release create` uses `--notes` flag (not `--body`) in this gh version
- All crates use `version.workspace = true` — only root `Cargo.toml [workspace.package] version` needs bumping
- `cargo generate-lockfile` then `cargo build --all-targets` to verify before committing
- Prior releases had no binary artifacts attached manually (CI uploads them)
