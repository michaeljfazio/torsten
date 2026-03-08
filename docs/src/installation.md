# Installation

Torsten is built from source using the standard Rust toolchain. Pre-built binaries are not yet distributed.

## Prerequisites

### Rust Toolchain

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

### System Dependencies

Torsten uses RocksDB for persistent storage, which requires `libclang` for compilation.

**Ubuntu / Debian:**

```bash
sudo apt-get update
sudo apt-get install -y libclang-dev build-essential pkg-config
```

**macOS (Homebrew):**

```bash
brew install llvm
```

The `llvm` package includes `libclang`. Homebrew typically configures the paths automatically. If not, you may need to set:

```bash
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
```

**Fedora / RHEL:**

```bash
sudo dnf install clang-devel
```

**Arch Linux:**

```bash
sudo pacman -S clang
```

## Building from Source

Clone the repository:

```bash
git clone https://github.com/michaeljfazio/torsten.git
cd torsten
```

Build in release mode:

```bash
cargo build --release
```

This produces two binaries in `target/release/`:

| Binary | Description |
|--------|-------------|
| `torsten-node` | The Cardano node |
| `torsten-cli` | The cardano-cli compatible command-line interface |

### Install Binaries

To install the binaries into your `$CARGO_HOME/bin` (typically `~/.cargo/bin/`):

```bash
cargo install --path crates/torsten-node
cargo install --path crates/torsten-cli
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
