---
name: dependency_status
description: Key dependency versions and update status as of 2026-03-14
type: project
---

# Dependency Status — 2026-03-14

**Why:** Track which dependencies are pre-release or on non-canonical branches.

**How to apply:** When checking for dependency updates, compare against this baseline.

## Pre-Release Dependencies (Require Action)

| Dependency | Current | Status |
|---|---|---|
| pallas-* (6 crates) | 1.0.0-alpha.5 | Pre-release alpha. Watch for stable 1.0.0 release. |
| cardano-lsm | REPLACED | Replaced by torsten-lsm (in-house pure Rust LSM). No longer a dependency. |
| vrf_dalek | git main branch | No tagged release. Monitor for stability. |

## Stable Dependencies (Up to Date)

| Dependency | Version |
|---|---|
| uplc | 1.1 |
| dashu-int | 0.4.1 |
| dashu-base | 0.4.1 |
| tokio | 1.x |
| ed25519-dalek | 2.x |
| minicbor | 0.25 |
| serde | 1.x |

## Notes
- pallas 1.0.0-alpha.5 is the latest pre-release; API changes between alphas could be breaking
- When pallas reaches stable, check: DatumOption (was PseudoDatumOption), Option<T> (was Nullable<T>), 28-byte hash padding changes
