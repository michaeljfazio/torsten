# Installation

Torsten can be installed from pre-built binaries, container images, or built from source.

## Pre-built Binaries

Download the latest release from [GitHub Releases](https://github.com/michaeljfazio/torsten/releases):

| Platform | Architecture | Download |
|----------|-------------|----------|
| Linux | x86_64 | `torsten-x86_64-linux.tar.gz` |
| Linux | aarch64 | `torsten-aarch64-linux.tar.gz` |
| macOS | x86_64 (Intel) | `torsten-x86_64-macos.tar.gz` |
| macOS | Apple Silicon | `torsten-aarch64-macos.tar.gz` |

```bash
# Example: download and extract for Linux x86_64
curl -LO https://github.com/michaeljfazio/torsten/releases/latest/download/torsten-x86_64-linux.tar.gz
tar xzf torsten-x86_64-linux.tar.gz
sudo mv torsten-node torsten-cli torsten-monitor torsten-config /usr/local/bin/
```

Verify checksums:

```bash
curl -LO https://github.com/michaeljfazio/torsten/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt
```

## Container Image

Multi-architecture container images (amd64 and arm64) are published to GitHub Container Registry:

```bash
docker pull ghcr.io/michaeljfazio/torsten:latest
```

The image uses a [distroless](https://github.com/GoogleContainerTools/distroless) base (`gcr.io/distroless/cc-debian12:nonroot`) for minimal attack surface — no shell, no package manager, runs as nonroot (UID 65532).

Run the node:

```bash
docker run -d \
  --name torsten \
  -p 3001:3001 \
  -p 12798:12798 \
  -v torsten-data:/opt/torsten/db \
  ghcr.io/michaeljfazio/torsten:latest
```

See [Kubernetes Deployment](./running/kubernetes.md) for production container deployments.

## Building from Source

### Prerequisites

#### Rust Toolchain

Install the latest stable Rust toolchain via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Verify the installation:

```bash
rustc --version
cargo --version
```

Torsten requires **Rust 1.75 or later** (edition 2021).

#### System Dependencies

Torsten's storage layer is pure Rust with **no system dependencies** beyond the Rust toolchain. Block storage uses append-only chunk files, and the UTxO set uses `torsten-lsm`, a pure Rust LSM tree. On all platforms, `cargo build` works out of the box.

### Build

Clone the repository:

```bash
git clone https://github.com/michaeljfazio/torsten.git
cd torsten
```

Build in release mode:

```bash
cargo build --release
```

On Linux with kernel 5.1+, you can enable io_uring for improved disk I/O in the UTxO LSM tree:

```bash
cargo build --release --features io-uring
```

This produces four binaries in `target/release/`:

| Binary | Description |
|--------|-------------|
| `torsten-node` | The Cardano node |
| `torsten-cli` | The cardano-cli compatible command-line interface |
| `torsten-monitor` | Terminal monitoring dashboard (ratatui-based, real-time metrics via Prometheus polling) |
| `torsten-config` | Interactive TUI configuration editor with tree navigation, inline editing, and diff view |

#### Install Binaries

To install the binaries into your `$CARGO_HOME/bin` (typically `~/.cargo/bin/`):

```bash
cargo install --path crates/torsten-node
cargo install --path crates/torsten-cli
cargo install --path crates/torsten-monitor
cargo install --path crates/torsten-config
```

## Running Tests

Verify everything is working:

```bash
cargo test --all
```

The project enforces a zero-warning policy. You can run the full CI check locally:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

## Development Build

For faster compilation during development, use the debug profile (the default):

```bash
cargo build
```

Debug builds are significantly faster to compile but produce slower binaries. Always use `--release` for running a node against a live network.
