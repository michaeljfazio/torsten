# Contributing to Dugite

Thank you for your interest in contributing to Dugite! This document provides guidelines for contributing.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone git@github.com:YOUR_USERNAME/dugite.git`
3. Create a branch: `git checkout -b feature/your-feature`
4. Make your changes
5. Run the checks: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo nextest run --workspace`
6. Commit and push
7. Open a pull request

## Development Requirements

- **Rust stable** (latest)
- **Zero warnings**: `RUSTFLAGS="-D warnings"` is enforced in CI
- **Clippy clean**: `cargo clippy --all-targets -- -D warnings` must pass
- **Formatted**: `cargo fmt --all -- --check` must pass
- **Tests pass**: `cargo nextest run --workspace` must pass (or `cargo test --all` if nextest is not installed)

## Code Style

- Follow standard Rust conventions (rustfmt handles formatting)
- Add comments where logic isn't self-evident
- Include unit tests for new functionality
- Use `thiserror` for error types in library crates
- Use `anyhow` for error handling in binary crates (node, cli)
- Prefer `Result` propagation over `unwrap()` in non-test code

## Architecture

Dugite is a 14-crate Cargo workspace. See [Architecture Overview](https://michaeljfazio.github.io/dugite/architecture/overview.html) for details.

Key constraints:
- **Dependency DAG**: No circular dependencies between crates
- **Trait boundaries**: Cross-crate interactions via traits, not concrete types
- **No unsafe**: All unsafe confined to `memmap2` I/O (storage crate only)
- **Cardano wire format**: All encoding/decoding via pallas crates

## Pull Request Process

1. Ensure CI passes (build, test, clippy, fmt)
2. Update documentation if you changed user-facing behavior
3. Add tests for new functionality
4. Keep PRs focused — one logical change per PR
5. Reference any related issues in the PR description

## Reporting Bugs

Use [GitHub Issues](https://github.com/michaeljfazio/dugite/issues) for bug reports. Include:
- Steps to reproduce
- Expected vs actual behavior
- Dugite version / commit hash
- Network (mainnet/preview/preprod)
- Relevant log output

## Security Vulnerabilities

**Do not report security vulnerabilities through public issues.** See [SECURITY.md](SECURITY.md) for responsible disclosure instructions.

## Discussions

For questions, ideas, and general discussion, use [GitHub Discussions](https://github.com/michaeljfazio/dugite/discussions).

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
