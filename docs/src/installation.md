# Installation

Dugite can be installed from pre-built binaries, container images, or built from source.

## Pre-built Binaries

Download the latest release from [GitHub Releases](https://github.com/michaeljfazio/dugite/releases):

| Platform | Architecture | Download |
|----------|-------------|----------|
| Linux | x86_64 | `dugite-x86_64-linux.tar.gz` |
| Linux | aarch64 | `dugite-aarch64-linux.tar.gz` |
| macOS | x86_64 (Intel) | `dugite-x86_64-macos.tar.gz` |
| macOS | Apple Silicon | `dugite-aarch64-macos.tar.gz` |

```bash
# Example: download and extract for Linux x86_64
curl -LO https://github.com/michaeljfazio/dugite/releases/latest/download/dugite-x86_64-linux.tar.gz
tar xzf dugite-x86_64-linux.tar.gz
sudo mv dugite-node dugite-cli dugite-monitor dugite-config /usr/local/bin/
```

Verify checksums:

```bash
curl -LO https://github.com/michaeljfazio/dugite/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt
```

## Container Image

Multi-architecture container images (amd64 and arm64) are published to GitHub Container Registry:

```bash
docker pull ghcr.io/michaeljfazio/dugite:latest
```

The image uses a [distroless](https://github.com/GoogleContainerTools/distroless) base (`gcr.io/distroless/cc-debian12:nonroot`) for minimal attack surface — no shell, no package manager, runs as nonroot (UID 65532).

Run the node:

```bash
docker run -d \
  --name dugite \
  -p 3001:3001 \
  -p 12798:12798 \
  -v dugite-data:/opt/dugite/db \
  ghcr.io/michaeljfazio/dugite:latest
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

Dugite requires **Rust 1.75 or later** (edition 2021).

#### System Dependencies

Dugite's storage layer is pure Rust with **no system dependencies** beyond the Rust toolchain. Block storage uses append-only chunk files, and the UTxO set uses `dugite-lsm`, a pure Rust LSM tree. On all platforms, `cargo build` works out of the box.

### Build

Clone the repository:

```bash
git clone https://github.com/michaeljfazio/dugite.git
cd dugite
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
| `dugite-node` | The Cardano node |
| `dugite-cli` | The cardano-cli compatible command-line interface |
| `dugite-monitor` | Terminal monitoring dashboard (ratatui-based, real-time metrics via Prometheus polling) |
| `dugite-config` | Interactive TUI configuration editor with tree navigation, inline editing, and diff view |

#### Install Binaries

To install the binaries into your `$CARGO_HOME/bin` (typically `~/.cargo/bin/`):

```bash
cargo install --path crates/dugite-node
cargo install --path crates/dugite-cli
cargo install --path crates/dugite-monitor
cargo install --path crates/dugite-config
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
