# Torsten Fuzz Targets

Fuzz testing for Torsten's untrusted-input parsers using
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer backend).

## Prerequisites

```bash
# Install cargo-fuzz (requires nightly Rust)
cargo install cargo-fuzz
rustup install nightly
```

## Available Targets

| Target | Description | Input |
|--------|-------------|-------|
| `fuzz_decode_block` | CBOR block deserialization | Raw bytes fed to `decode_block()` |
| `fuzz_decode_transaction` | CBOR transaction deserialization (all eras) | Raw bytes fed to `decode_transaction()` |
| `fuzz_mux_segment` | Ouroboros multiplexer segment parsing | Raw bytes fed to `Segment::decode()` |
| `fuzz_nonce_update` | Evolving nonce blake2b computation | Arbitrary-length bytes simulating VRF output |

## Running a Target

```bash
# From the repository root:
cd fuzz

# Run a specific target for 5 minutes
cargo +nightly fuzz run fuzz_decode_block -- -max_total_time=300

# Run with a corpus directory (seeds are auto-saved)
cargo +nightly fuzz run fuzz_decode_block corpus/decode_block

# Run all targets sequentially (10 min each)
for target in fuzz_decode_block fuzz_decode_transaction fuzz_mux_segment fuzz_nonce_update; do
  echo "=== Fuzzing $target ==="
  cargo +nightly fuzz run $target -- -max_total_time=600
done
```

## Corpus Management

Seed corpora live in `corpus/<target>/`. The fuzzer automatically saves
interesting inputs (new coverage) to this directory. Commit interesting
corpus files to the repository so future runs start from better seeds.

For `fuzz_decode_block`, the corpus is seeded with real block CBOR from
Cardano eras (Alonzo, Babbage, Conway).

## Coverage-Guided Fuzzing Tips

- **Duration**: 10-60 minutes per target is a reasonable starting point.
  Longer runs explore deeper paths.
- **Parallelism**: Use `-fork=N` to run N fuzzer processes in parallel:
  ```bash
  cargo +nightly fuzz run fuzz_decode_block -- -fork=4 -max_total_time=600
  ```
- **Memory limit**: Default is 2 GB. Increase with `-rss_limit_mb=4096`
  if the target legitimately needs more.
- **Artifact analysis**: When a crash is found, the input is saved to
  `fuzz/artifacts/<target>/`. Reproduce with:
  ```bash
  cargo +nightly fuzz run fuzz_decode_block fuzz/artifacts/fuzz_decode_block/<crash_file>
  ```

## Adding New Targets

1. Create `fuzz/fuzz_targets/<name>.rs` with a `fuzz_target!` macro.
2. Add the `[[bin]]` entry to `fuzz/Cargo.toml`.
3. Add any new crate dependencies to `[dependencies]` in `fuzz/Cargo.toml`.
4. Optionally seed `fuzz/corpus/<name>/` with representative inputs.
