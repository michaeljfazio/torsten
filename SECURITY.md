# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| main    | :white_check_mark: |

Dugite is in active development. Security fixes are applied to the `main` branch.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, use [GitHub's private vulnerability reporting](https://github.com/michaeljfazio/dugite/security/advisories/new) to report security issues confidentially.

You should receive a response within 48 hours. If the issue is confirmed, we will:
1. Work on a fix in a private branch
2. Release the fix and publish a security advisory
3. Credit the reporter (unless anonymity is requested)

## Scope

The following are in scope for security reports:

- **Consensus vulnerabilities**: Anything that could cause chain divergence, invalid block acceptance, or denial of consensus
- **Ledger vulnerabilities**: UTxO manipulation, reward miscalculation, governance voting exploitation
- **Network vulnerabilities**: Peer-to-peer protocol abuse, eclipse attacks, amplification attacks
- **Cryptographic issues**: VRF/KES/Ed25519 implementation flaws, nonce prediction
- **Storage corruption**: Data loss, silent corruption, crash recovery failures
- **Denial of service**: Resource exhaustion, unbounded allocation, panic-inducing inputs
- **Information disclosure**: Private key leakage, mempool content leakage to unauthorized parties

## Out of Scope

- Issues in upstream dependencies (pallas, uplc, vrf_dalek) — report these to their respective maintainers
- Performance issues that do not constitute denial of service
- Issues requiring physical access to the host machine

## Security Practices

- All code is compiled with `RUSTFLAGS="-D warnings"` (zero tolerance for compiler warnings)
- No `unsafe` code blocks in the Dugite codebase
- Dependabot security alerts and automated fixes are enabled
- Secret scanning with push protection is enabled
- All cryptographic operations use established libraries (ed25519-dalek, blake2, pallas-crypto)
- CRC32 checksums on WAL entries and SSTable pages for data integrity
- Exclusive session locks prevent concurrent database access corruption

## Acknowledgments

We appreciate the security research community's efforts in responsibly disclosing vulnerabilities. Contributors will be acknowledged in release notes and security advisories.
