# Introduction

**Torsten** is a Cardano node implementation written in Rust, aiming for 100% compatibility with [cardano-node](https://github.com/IntersectMBO/cardano-node) (Haskell).

Built by [Sandstone Pool](https://www.sandstone.io/).

[![CI](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml/badge.svg)](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml)

## Why Torsten?

The Cardano ecosystem benefits from client diversity. Running multiple independent node implementations strengthens the network by:

- **Resilience** — A bug in one implementation does not bring down the entire network.
- **Performance** — Rust's zero-cost abstractions and memory safety without garbage collection enable high-throughput block processing.
- **Verification** — An independent implementation validates the Cardano specification against the reference Haskell node, catching ambiguities and edge cases.
- **Accessibility** — A Rust codebase broadens the pool of developers who can contribute to Cardano infrastructure.

## Key Features

- **Full Ouroboros Praos consensus** — Slot leader checks, VRF validation, KES period tracking, epoch nonce computation.
- **Multi-era support** — Byron, Shelley, Allegra, Mary, Alonzo, Babbage, and Conway eras.
- **Conway governance (CIP-1694)** — DRep registration, voting, proposals, constitutional committee, treasury withdrawals.
- **Pipelined multi-peer sync** — Header collection from a primary peer with parallel block fetching from multiple peers.
- **Plutus script execution** — Plutus V1/V2/V3 evaluation via the uplc CEK machine.
- **Node-to-Node (N2N) protocol** — Full Ouroboros mini-protocol suite: ChainSync, BlockFetch, TxSubmission2, KeepAlive, PeerSharing.
- **Node-to-Client (N2C) protocol** — Unix domain socket server with LocalChainSync, LocalStateQuery, LocalTxSubmission, and LocalTxMonitor.
- **cardano-cli compatible CLI** — Key generation, transaction building, signing, submission, queries, and governance commands.
- **Prometheus metrics** — Real-time node metrics on port 12798.
- **P2P networking** — Peer manager with cold/warm/hot lifecycle, DNS multi-resolution, ledger-based peer discovery, and inbound rate limiting.
- **Mithril snapshot import** — Fast initial sync by importing a Mithril-certified snapshot.
- **SIGHUP topology reload** — Update peer configuration without restarting the node.

## Project Status

Torsten is under active development. It can sync against both the Cardano mainnet and preview/preprod testnets. The node implements the full N2N and N2C protocol stacks, ledger validation, epoch transitions with stake snapshots and reward distribution, and Conway-era governance.

See the [Feature Status](https://github.com/michaeljfazio/torsten#feature-status) section in the repository README for a detailed checklist of implemented and pending features.

## License

Torsten is released under the [MIT License](https://github.com/michaeljfazio/torsten/blob/main/LICENSE).
