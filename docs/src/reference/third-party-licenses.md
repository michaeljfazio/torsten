# Third-Party Licenses

Dugite depends on a number of open-source Rust crates. This page documents
all third-party dependencies and their license terms.

**Total dependencies:** 393

## License Summary

| License | Count |
|---------|-------|
| MIT OR Apache-2.0 | 205 |
| MIT | 71 |
| Apache-2.0 OR MIT | 33 |
| Unicode-3.0 | 18 |
| Apache-2.0 | 17 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 14 |
| Unlicense OR MIT | 6 |
| BSD-3-Clause | 4 |
| Apache-2.0 OR ISC OR MIT | 2 |
| MIT OR Apache-2.0 OR Zlib | 2 |
| BlueOak-1.0.0 | 2 |
| ISC | 2 |
| CC0-1.0 | 2 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 |
| BSD-2-Clause | 1 |
| CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | 1 |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 |
| MIT OR Apache-2.0 OR BSD-1-Clause | 1 |
| Apache-2.0  OR  MIT | 1 |
| Zlib | 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 1 |
| Apache-2.0 AND ISC | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| Zlib OR Apache-2.0 OR MIT | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |
| Unknown | 1 |
| CDLA-Permissive-2.0 | 1 |

## Key Dependencies

These are the primary libraries that Dugite directly depends on:

| Crate | Version | License | Description |
|-------|---------|---------|-------------|
| [pallas-codec](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Pallas common CBOR encoding interface and utilities |
| [pallas-crypto](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Cryptographic primitives for Cardano |
| [pallas-primitives](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Ledger primitives and cbor codec for the different Cardano eras |
| [pallas-traverse](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Utilities to traverse over multi-era block data |
| [pallas-addresses](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Ergonomic library to work with different Cardano addresses |
| [pallas-network](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 | Ouroboros networking stack using async IO |
| [uplc](https://github.com/aiken-lang/aiken) | 1.1.21 | Apache-2.0 | Utilities for working with Untyped Plutus Core |
| [tokio](https://github.com/tokio-rs/tokio) | 1.50.0 | MIT | An event-driven, non-blocking I/O platform for writing asynchronous I/O
backe... |
| [hyper](https://github.com/hyperium/hyper) | 1.8.1 | MIT | A protective and efficient HTTP library for all. |
| [reqwest](https://github.com/seanmonstar/reqwest) | 0.12.28 | MIT OR Apache-2.0 | higher level HTTP client library |
| [clap](https://github.com/clap-rs/clap) | 4.6.0 | MIT OR Apache-2.0 | A simple to use, efficient, and full-featured Command Line Argument Parser |
| [serde](https://github.com/serde-rs/serde) | 1.0.228 | MIT OR Apache-2.0 | A generic serialization/deserialization framework |
| [serde_json](https://github.com/serde-rs/json) | 1.0.149 | MIT OR Apache-2.0 | A JSON serialization file format |
| [bincode](https://github.com/servo/bincode) | 1.3.3 | MIT | A binary serialization / deserialization strategy that uses Serde for transfo... |
| [blake2b_simd](https://github.com/oconnor663/blake2_simd) | 1.0.4 | MIT | a pure Rust BLAKE2b implementation with dynamic SIMD |
| [sha2](https://github.com/RustCrypto/hashes) | 0.9.9 | MIT OR Apache-2.0 | Pure Rust implementation of the SHA-2 hash function family
including SHA-224,... |
| [ed25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek) | 2.2.0 | BSD-3-Clause | Fast and efficient ed25519 EdDSA key generations, signing, and verification i... |
| [curve25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/tree/main/curve25519-dalek) | 4.1.3 | BSD-3-Clause | A pure-Rust implementation of group operations on ristretto255 and Curve25519 |
| [blst](https://github.com/supranational/blst) | 0.3.16 | Apache-2.0 | Bindings for blst BLS12-381 library |
| [k256](https://github.com/RustCrypto/elliptic-curves/tree/master/k256) | 0.13.4 | Apache-2.0 OR MIT | secp256k1 elliptic curve library written in pure Rust with support for ECDSA
... |
| [minicbor](https://github.com/twittner/minicbor) | 0.26.5 | BlueOak-1.0.0 | A small CBOR codec suitable for no_std environments. |
| [tracing](https://github.com/tokio-rs/tracing) | 0.1.44 | MIT | Application-level tracing for Rust. |
| [tracing-subscriber](https://github.com/tokio-rs/tracing) | 0.3.22 | MIT | Utilities for implementing and composing `tracing` subscribers. |
| [dashmap](https://github.com/xacrimon/dashmap) | 6.1.0 | MIT | Blazing fast concurrent HashMap for Rust. |
| [crossbeam](https://github.com/crossbeam-rs/crossbeam) | 0.8.4 | MIT OR Apache-2.0 | Tools for concurrent programming |
| [dashu-int](https://github.com/cmpute/dashu) | 0.4.1 | MIT OR Apache-2.0 | A big integer library with good performance |
| [memmap2](https://github.com/RazrFalcon/memmap2-rs) | 0.9.10 | MIT OR Apache-2.0 | Cross-platform Rust API for memory-mapped file IO |
| [lz4](https://github.com/10xGenomics/lz4-rs) | 1.28.1 | MIT | Rust LZ4 bindings library. |
| [zstd](https://github.com/gyscos/zstd-rs) | 0.13.3 | MIT | Binding for the zstd compression library. |
| [tar](https://github.com/alexcrichton/tar-rs) | 0.4.44 | MIT OR Apache-2.0 | A Rust implementation of a TAR file reader and writer. This library does not
... |
| [crc32fast](https://github.com/srijs/rust-crc32fast) | 1.5.0 | MIT OR Apache-2.0 | Fast, SIMD-accelerated CRC32 (IEEE) checksum computation |
| [hex](https://github.com/KokaKiwi/rust-hex) | 0.4.3 | MIT OR Apache-2.0 | Encoding and decoding data into/from hexadecimal representation. |
| [bs58](https://github.com/Nullus157/bs58-rs) | 0.5.1 | MIT/Apache-2.0 | Another Base58 codec implementation. |
| [bech32](https://github.com/rust-bitcoin/rust-bech32) | 0.9.1 | MIT | Encodes and decodes the Bech32 format |
| [base64](https://github.com/marshallpierce/rust-base64) | 0.22.1 | MIT OR Apache-2.0 | encodes and decodes base64 as bytes or utf8 |
| [rand](https://github.com/rust-random/rand) | 0.9.2 | MIT OR Apache-2.0 | Random number generators and other randomness functionality. |
| [chrono](https://github.com/chronotope/chrono) | 0.4.44 | MIT OR Apache-2.0 | Date and time library for Rust |
| [uuid](https://github.com/uuid-rs/uuid) | 1.22.0 | Apache-2.0 OR MIT | A library to generate and parse UUIDs. |
| [indicatif](https://github.com/console-rs/indicatif) | 0.17.11 | MIT | A progress bar and cli reporting library for Rust |
| vrf_dalek | 0.1.0 | Unknown |  |

## All Dependencies

Complete list of all third-party crates used by Dugite, sorted alphabetically.

| Crate | Version | License |
|-------|---------|---------|
| [aho-corasick](https://github.com/BurntSushi/aho-corasick) | 1.1.4 | Unlicense OR MIT |
| [android_system_properties](https://github.com/nical/android_system_properties) | 0.1.5 | MIT/Apache-2.0 |
| [anes](https://github.com/zrzka/anes-rs) | 0.1.6 | MIT OR Apache-2.0 |
| [anstream](https://github.com/rust-cli/anstyle.git) | 1.0.0 | MIT OR Apache-2.0 |
| [anstyle](https://github.com/rust-cli/anstyle.git) | 1.0.13 | MIT OR Apache-2.0 |
| [anstyle-parse](https://github.com/rust-cli/anstyle.git) | 1.0.0 | MIT OR Apache-2.0 |
| [anstyle-query](https://github.com/rust-cli/anstyle.git) | 1.1.5 | MIT OR Apache-2.0 |
| [anstyle-wincon](https://github.com/rust-cli/anstyle.git) | 3.0.11 | MIT OR Apache-2.0 |
| [anyhow](https://github.com/dtolnay/anyhow) | 1.0.102 | MIT OR Apache-2.0 |
| [arrayref](https://github.com/droundy/arrayref) | 0.3.9 | BSD-2-Clause |
| [arrayvec](https://github.com/bluss/arrayvec) | 0.7.6 | MIT OR Apache-2.0 |
| [async-trait](https://github.com/dtolnay/async-trait) | 0.1.89 | MIT OR Apache-2.0 |
| [atomic-waker](https://github.com/smol-rs/atomic-waker) | 1.1.2 | Apache-2.0 OR MIT |
| [autocfg](https://github.com/cuviper/autocfg) | 1.5.0 | Apache-2.0 OR MIT |
| [base16ct](https://github.com/RustCrypto/formats/tree/master/base16ct) | 0.2.0 | Apache-2.0 OR MIT |
| [base58](https://github.com/debris/base58) | 0.2.0 | MIT |
| [base64](https://github.com/marshallpierce/rust-base64) | 0.22.1 | MIT OR Apache-2.0 |
| [base64ct](https://github.com/RustCrypto/formats) | 1.8.3 | Apache-2.0 OR MIT |
| [bech32](https://github.com/rust-bitcoin/rust-bech32) | 0.9.1 | MIT |
| [bincode](https://github.com/servo/bincode) | 1.3.3 | MIT |
| [bit-set](https://github.com/contain-rs/bit-set) | 0.8.0 | Apache-2.0 OR MIT |
| [bit-vec](https://github.com/contain-rs/bit-vec) | 0.8.0 | Apache-2.0 OR MIT |
| [bitflags](https://github.com/bitflags/bitflags) | 2.11.0 | MIT OR Apache-2.0 |
| [bitvec](https://github.com/bitvecto-rs/bitvec) | 1.0.1 | MIT |
| [blake2](https://github.com/RustCrypto/hashes) | 0.10.6 | MIT OR Apache-2.0 |
| [blake2b_simd](https://github.com/oconnor663/blake2_simd) | 1.0.4 | MIT |
| [blake3](https://github.com/BLAKE3-team/BLAKE3) | 1.8.3 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| [block-buffer](https://github.com/RustCrypto/utils) | 0.9.0 | MIT OR Apache-2.0 |
| [blst](https://github.com/supranational/blst) | 0.3.16 | Apache-2.0 |
| [bs58](https://github.com/Nullus157/bs58-rs) | 0.5.1 | MIT/Apache-2.0 |
| [bumpalo](https://github.com/fitzgen/bumpalo) | 3.20.2 | MIT OR Apache-2.0 |
| [byteorder](https://github.com/BurntSushi/byteorder) | 1.5.0 | Unlicense OR MIT |
| [bytes](https://github.com/tokio-rs/bytes) | 1.11.1 | MIT |
| [cast](https://github.com/japaric/cast.rs) | 0.3.0 | MIT OR Apache-2.0 |
| [cc](https://github.com/rust-lang/cc-rs) | 1.2.56 | MIT OR Apache-2.0 |
| [cfg-if](https://github.com/rust-lang/cfg-if) | 1.0.4 | MIT OR Apache-2.0 |
| [cfg_aliases](https://github.com/katharostech/cfg_aliases) | 0.2.1 | MIT |
| [chrono](https://github.com/chronotope/chrono) | 0.4.44 | MIT OR Apache-2.0 |
| [ciborium](https://github.com/enarx/ciborium) | 0.2.2 | Apache-2.0 |
| [ciborium-io](https://github.com/enarx/ciborium) | 0.2.2 | Apache-2.0 |
| [ciborium-ll](https://github.com/enarx/ciborium) | 0.2.2 | Apache-2.0 |
| [clap](https://github.com/clap-rs/clap) | 4.6.0 | MIT OR Apache-2.0 |
| [clap_builder](https://github.com/clap-rs/clap) | 4.6.0 | MIT OR Apache-2.0 |
| [clap_derive](https://github.com/clap-rs/clap) | 4.6.0 | MIT OR Apache-2.0 |
| [clap_lex](https://github.com/clap-rs/clap) | 1.1.0 | MIT OR Apache-2.0 |
| [colorchoice](https://github.com/rust-cli/anstyle.git) | 1.0.4 | MIT OR Apache-2.0 |
| [console](https://github.com/console-rs/console) | 0.15.11 | MIT |
| [const-oid](https://github.com/RustCrypto/formats/tree/master/const-oid) | 0.9.6 | Apache-2.0 OR MIT |
| [constant_time_eq](https://github.com/cesarb/constant_time_eq) | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| [core-foundation-sys](https://github.com/servo/core-foundation-rs) | 0.8.7 | MIT OR Apache-2.0 |
| [cpufeatures](https://github.com/RustCrypto/utils) | 0.2.17 | MIT OR Apache-2.0 |
| [crc](https://github.com/mrhooray/crc-rs.git) | 3.4.0 | MIT OR Apache-2.0 |
| [crc-catalog](https://github.com/akhilles/crc-catalog.git) | 2.4.0 | MIT OR Apache-2.0 |
| [crc32fast](https://github.com/srijs/rust-crc32fast) | 1.5.0 | MIT OR Apache-2.0 |
| [criterion](https://github.com/bheisler/criterion.rs) | 0.5.1 | Apache-2.0 OR MIT |
| [criterion-plot](https://github.com/bheisler/criterion.rs) | 0.5.0 | MIT/Apache-2.0 |
| [crossbeam](https://github.com/crossbeam-rs/crossbeam) | 0.8.4 | MIT OR Apache-2.0 |
| [crossbeam-channel](https://github.com/crossbeam-rs/crossbeam) | 0.5.15 | MIT OR Apache-2.0 |
| [crossbeam-deque](https://github.com/crossbeam-rs/crossbeam) | 0.8.6 | MIT OR Apache-2.0 |
| [crossbeam-epoch](https://github.com/crossbeam-rs/crossbeam) | 0.9.18 | MIT OR Apache-2.0 |
| [crossbeam-queue](https://github.com/crossbeam-rs/crossbeam) | 0.3.12 | MIT OR Apache-2.0 |
| [crossbeam-utils](https://github.com/crossbeam-rs/crossbeam) | 0.8.21 | MIT OR Apache-2.0 |
| [crunchy](https://github.com/eira-fransham/crunchy) | 0.2.4 | MIT |
| [crypto-bigint](https://github.com/RustCrypto/crypto-bigint) | 0.5.5 | Apache-2.0 OR MIT |
| [crypto-common](https://github.com/RustCrypto/traits) | 0.1.7 | MIT OR Apache-2.0 |
| [cryptoxide](https://github.com/typed-io/cryptoxide/) | 0.4.4 | MIT/Apache-2.0 |
| [curve25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/tree/main/curve25519-dalek) | 4.1.3 | BSD-3-Clause |
| [curve25519-dalek-derive](https://github.com/dalek-cryptography/curve25519-dalek) | 0.1.1 | MIT/Apache-2.0 |
| [darling](https://github.com/TedDriggs/darling) | 0.21.3 | MIT |
| [darling_core](https://github.com/TedDriggs/darling) | 0.21.3 | MIT |
| [darling_macro](https://github.com/TedDriggs/darling) | 0.21.3 | MIT |
| [dashmap](https://github.com/xacrimon/dashmap) | 6.1.0 | MIT |
| [dashu-base](https://github.com/cmpute/dashu) | 0.4.1 | MIT OR Apache-2.0 |
| [dashu-int](https://github.com/cmpute/dashu) | 0.4.1 | MIT OR Apache-2.0 |
| [der](https://github.com/RustCrypto/formats/tree/master/der) | 0.7.10 | Apache-2.0 OR MIT |
| [deranged](https://github.com/jhpratt/deranged) | 0.5.8 | MIT OR Apache-2.0 |
| [derive_more](https://github.com/JelteF/derive_more) | 1.0.0 | MIT |
| [derive_more-impl](https://github.com/JelteF/derive_more) | 1.0.0 | MIT |
| [digest](https://github.com/RustCrypto/traits) | 0.9.0 | MIT OR Apache-2.0 |
| [displaydoc](https://github.com/yaahc/displaydoc) | 0.2.5 | MIT OR Apache-2.0 |
| [dyn-clone](https://github.com/dtolnay/dyn-clone) | 1.0.20 | MIT OR Apache-2.0 |
| [ecdsa](https://github.com/RustCrypto/signatures/tree/master/ecdsa) | 0.16.9 | Apache-2.0 OR MIT |
| [ed25519](https://github.com/RustCrypto/signatures/tree/master/ed25519) | 2.2.3 | Apache-2.0 OR MIT |
| [ed25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek) | 2.2.0 | BSD-3-Clause |
| [either](https://github.com/rayon-rs/either) | 1.15.0 | MIT OR Apache-2.0 |
| [elliptic-curve](https://github.com/RustCrypto/traits/tree/master/elliptic-curve) | 0.13.8 | Apache-2.0 OR MIT |
| [encode_unicode](https://github.com/tormol/encode_unicode) | 1.0.0 | Apache-2.0 OR MIT |
| [equivalent](https://github.com/indexmap-rs/equivalent) | 1.0.2 | Apache-2.0 OR MIT |
| [errno](https://github.com/lambda-fairy/rust-errno) | 0.3.14 | MIT OR Apache-2.0 |
| [fastrand](https://github.com/smol-rs/fastrand) | 2.3.0 | Apache-2.0 OR MIT |
| [ff](https://github.com/zkcrypto/ff) | 0.13.1 | MIT/Apache-2.0 |
| [fiat-crypto](https://github.com/mit-plv/fiat-crypto) | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause |
| [filetime](https://github.com/alexcrichton/filetime) | 0.2.27 | MIT/Apache-2.0 |
| [find-msvc-tools](https://github.com/rust-lang/cc-rs) | 0.1.9 | MIT OR Apache-2.0 |
| [fnv](https://github.com/servo/rust-fnv) | 1.0.7 | Apache-2.0 / MIT |
| [foldhash](https://github.com/orlp/foldhash) | 0.1.5 | Zlib |
| [form_urlencoded](https://github.com/servo/rust-url) | 1.2.2 | MIT OR Apache-2.0 |
| [fs2](https://github.com/danburkert/fs2-rs) | 0.4.3 | MIT/Apache-2.0 |
| [funty](https://github.com/myrrlyn/funty) | 2.0.0 | MIT |
| [futures](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-channel](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-core](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-executor](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-io](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-macro](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-sink](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-task](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [futures-util](https://github.com/rust-lang/futures-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [generic-array](https://github.com/fizyk20/generic-array.git) | 0.14.7 | MIT |
| [getrandom](https://github.com/rust-random/getrandom) | 0.4.2 | MIT OR Apache-2.0 |
| [glob](https://github.com/rust-lang/glob) | 0.3.3 | MIT OR Apache-2.0 |
| [group](https://github.com/zkcrypto/group) | 0.13.0 | MIT/Apache-2.0 |
| [half](https://github.com/VoidStarKat/half-rs) | 2.7.1 | MIT OR Apache-2.0 |
| [hamming](https://github.com/huonw/hamming) | 0.1.3 | MIT/Apache-2.0 |
| [hashbrown](https://github.com/rust-lang/hashbrown) | 0.16.1 | MIT OR Apache-2.0 |
| [heck](https://github.com/withoutboats/heck) | 0.5.0 | MIT OR Apache-2.0 |
| [hermit-abi](https://github.com/hermit-os/hermit-rs) | 0.5.2 | MIT OR Apache-2.0 |
| [hex](https://github.com/KokaKiwi/rust-hex) | 0.4.3 | MIT OR Apache-2.0 |
| [hmac](https://github.com/RustCrypto/MACs) | 0.12.1 | MIT OR Apache-2.0 |
| [hostname](https://github.com/svartalf/hostname) | 0.3.1 | MIT |
| [http](https://github.com/hyperium/http) | 1.4.0 | MIT OR Apache-2.0 |
| [http-body](https://github.com/hyperium/http-body) | 1.0.1 | MIT |
| [http-body-util](https://github.com/hyperium/http-body) | 0.1.3 | MIT |
| [httparse](https://github.com/seanmonstar/httparse) | 1.10.1 | MIT OR Apache-2.0 |
| [hyper](https://github.com/hyperium/hyper) | 1.8.1 | MIT |
| [hyper-rustls](https://github.com/rustls/hyper-rustls) | 0.27.7 | Apache-2.0 OR ISC OR MIT |
| [hyper-util](https://github.com/hyperium/hyper-util) | 0.1.20 | MIT |
| [iana-time-zone](https://github.com/strawlab/iana-time-zone) | 0.1.65 | MIT OR Apache-2.0 |
| [iana-time-zone-haiku](https://github.com/strawlab/iana-time-zone) | 0.1.2 | MIT OR Apache-2.0 |
| [icu_collections](https://github.com/unicode-org/icu4x) | 2.1.1 | Unicode-3.0 |
| [icu_locale_core](https://github.com/unicode-org/icu4x) | 2.1.1 | Unicode-3.0 |
| [icu_normalizer](https://github.com/unicode-org/icu4x) | 2.1.1 | Unicode-3.0 |
| [icu_normalizer_data](https://github.com/unicode-org/icu4x) | 2.1.1 | Unicode-3.0 |
| [icu_properties](https://github.com/unicode-org/icu4x) | 2.1.2 | Unicode-3.0 |
| [icu_properties_data](https://github.com/unicode-org/icu4x) | 2.1.2 | Unicode-3.0 |
| [icu_provider](https://github.com/unicode-org/icu4x) | 2.1.1 | Unicode-3.0 |
| [id-arena](https://github.com/fitzgen/id-arena) | 2.3.0 | MIT/Apache-2.0 |
| [ident_case](https://github.com/TedDriggs/ident_case) | 1.0.1 | MIT/Apache-2.0 |
| [idna](https://github.com/servo/rust-url/) | 1.1.0 | MIT OR Apache-2.0 |
| [idna_adapter](https://github.com/hsivonen/idna_adapter) | 1.2.1 | Apache-2.0 OR MIT |
| [indexmap](https://github.com/indexmap-rs/indexmap) | 2.13.0 | Apache-2.0 OR MIT |
| [indicatif](https://github.com/console-rs/indicatif) | 0.17.11 | MIT |
| [ipnet](https://github.com/krisprice/ipnet) | 2.12.0 | MIT OR Apache-2.0 |
| [iri-string](https://github.com/lo48576/iri-string) | 0.7.10 | MIT OR Apache-2.0 |
| [is-terminal](https://github.com/sunfishcode/is-terminal) | 0.4.17 | MIT |
| [is_terminal_polyfill](https://github.com/polyfill-rs/is_terminal_polyfill) | 1.70.2 | MIT OR Apache-2.0 |
| [itertools](https://github.com/rust-itertools/itertools) | 0.13.0 | MIT OR Apache-2.0 |
| [itoa](https://github.com/dtolnay/itoa) | 1.0.17 | MIT OR Apache-2.0 |
| [jobserver](https://github.com/rust-lang/jobserver-rs) | 0.1.34 | MIT OR Apache-2.0 |
| [js-sys](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys) | 0.3.91 | MIT OR Apache-2.0 |
| [k256](https://github.com/RustCrypto/elliptic-curves/tree/master/k256) | 0.13.4 | Apache-2.0 OR MIT |
| [lazy_static](https://github.com/rust-lang-nursery/lazy-static.rs) | 1.5.0 | MIT OR Apache-2.0 |
| [leb128fmt](https://github.com/bluk/leb128fmt) | 0.1.0 | MIT OR Apache-2.0 |
| [libc](https://github.com/rust-lang/libc) | 0.2.183 | MIT OR Apache-2.0 |
| [libredox](https://gitlab.redox-os.org/redox-os/libredox.git) | 0.1.14 | MIT |
| [linux-raw-sys](https://github.com/sunfishcode/linux-raw-sys) | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [litemap](https://github.com/unicode-org/icu4x) | 0.8.1 | Unicode-3.0 |
| [lock_api](https://github.com/Amanieu/parking_lot) | 0.4.14 | MIT OR Apache-2.0 |
| [log](https://github.com/rust-lang/log) | 0.4.29 | MIT OR Apache-2.0 |
| [lru-slab](https://github.com/Ralith/lru-slab) | 0.1.2 | MIT OR Apache-2.0 OR Zlib |
| [lz4](https://github.com/10xGenomics/lz4-rs) | 1.28.1 | MIT |
| [lz4-sys](https://github.com/10xGenomics/lz4-rs) | 1.11.1+lz4-1.10.0 | MIT |
| [match_cfg](https://github.com/gnzlbg/match_cfg) | 0.1.0 | MIT/Apache-2.0 |
| [matchers](https://github.com/hawkw/matchers) | 0.2.0 | MIT |
| [memchr](https://github.com/BurntSushi/memchr) | 2.8.0 | Unlicense OR MIT |
| [memmap2](https://github.com/RazrFalcon/memmap2-rs) | 0.9.10 | MIT OR Apache-2.0 |
| [miette](https://github.com/zkat/miette) | 5.10.0 | Apache-2.0 |
| [miette-derive](https://github.com/zkat/miette) | 5.10.0 | Apache-2.0 |
| [minicbor](https://github.com/twittner/minicbor) | 0.26.5 | BlueOak-1.0.0 |
| [minicbor-derive](https://github.com/twittner/minicbor) | 0.16.2 | BlueOak-1.0.0 |
| [mio](https://github.com/tokio-rs/mio) | 1.1.1 | MIT |
| [nu-ansi-term](https://github.com/nushell/nu-ansi-term) | 0.50.3 | MIT |
| [num-bigint](https://github.com/rust-num/num-bigint) | 0.4.6 | MIT OR Apache-2.0 |
| [num-conv](https://github.com/jhpratt/num-conv) | 0.2.0 | MIT OR Apache-2.0 |
| [num-integer](https://github.com/rust-num/num-integer) | 0.1.46 | MIT OR Apache-2.0 |
| [num-modular](https://github.com/cmpute/num-modular) | 0.6.1 | Apache-2.0 |
| [num-order](https://github.com/cmpute/num-order) | 1.2.0 | Apache-2.0 |
| [num-rational](https://github.com/rust-num/num-rational) | 0.4.2 | MIT OR Apache-2.0 |
| [num-traits](https://github.com/rust-num/num-traits) | 0.2.19 | MIT OR Apache-2.0 |
| [num_cpus](https://github.com/seanmonstar/num_cpus) | 1.17.0 | MIT OR Apache-2.0 |
| [number_prefix](https://github.com/ogham/rust-number-prefix) | 0.4.0 | MIT |
| [once_cell](https://github.com/matklad/once_cell) | 1.21.4 | MIT OR Apache-2.0 |
| [once_cell_polyfill](https://github.com/polyfill-rs/once_cell_polyfill) | 1.70.2 | MIT OR Apache-2.0 |
| [oorandom](https://hg.sr.ht/~icefox/oorandom) | 11.1.5 | MIT |
| [opaque-debug](https://github.com/RustCrypto/utils) | 0.3.1 | MIT OR Apache-2.0 |
| [pallas-addresses](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [pallas-codec](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [pallas-crypto](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [pallas-network](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [pallas-primitives](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [pallas-traverse](https://github.com/txpipe/pallas) | 1.0.0-alpha.5 | Apache-2.0 |
| [parking_lot](https://github.com/Amanieu/parking_lot) | 0.12.5 | MIT OR Apache-2.0 |
| [parking_lot_core](https://github.com/Amanieu/parking_lot) | 0.9.12 | MIT OR Apache-2.0 |
| [paste](https://github.com/dtolnay/paste) | 1.0.15 | MIT OR Apache-2.0 |
| [peg](https://github.com/kevinmehall/rust-peg) | 0.8.5 | MIT |
| [peg-macros](https://github.com/kevinmehall/rust-peg) | 0.8.5 | MIT |
| [peg-runtime](https://github.com/kevinmehall/rust-peg) | 0.8.5 | MIT |
| [percent-encoding](https://github.com/servo/rust-url/) | 2.3.2 | MIT OR Apache-2.0 |
| [pin-project-lite](https://github.com/taiki-e/pin-project-lite) | 0.2.17 | Apache-2.0 OR MIT |
| [pin-utils](https://github.com/rust-lang-nursery/pin-utils) | 0.1.0 | MIT OR Apache-2.0 |
| [pkcs8](https://github.com/RustCrypto/formats/tree/master/pkcs8) | 0.10.2 | Apache-2.0 OR MIT |
| [pkg-config](https://github.com/rust-lang/pkg-config-rs) | 0.3.32 | MIT OR Apache-2.0 |
| [plain](https://github.com/randomites/plain) | 0.2.3 | MIT/Apache-2.0 |
| [plotters](https://github.com/plotters-rs/plotters) | 0.3.7 | MIT |
| [plotters-backend](https://github.com/plotters-rs/plotters) | 0.3.7 | MIT |
| [plotters-svg](https://github.com/plotters-rs/plotters.git) | 0.3.7 | MIT |
| [portable-atomic](https://github.com/taiki-e/portable-atomic) | 1.13.1 | Apache-2.0 OR MIT |
| [potential_utf](https://github.com/unicode-org/icu4x) | 0.1.4 | Unicode-3.0 |
| [powerfmt](https://github.com/jhpratt/powerfmt) | 0.2.0 | MIT OR Apache-2.0 |
| [ppv-lite86](https://github.com/cryptocorrosion/cryptocorrosion) | 0.2.21 | MIT OR Apache-2.0 |
| [pretty](https://github.com/Marwes/pretty.rs) | 0.11.3 | MIT |
| [prettyplease](https://github.com/dtolnay/prettyplease) | 0.2.37 | MIT OR Apache-2.0 |
| [proc-macro2](https://github.com/dtolnay/proc-macro2) | 1.0.106 | MIT OR Apache-2.0 |
| [proptest](https://github.com/proptest-rs/proptest) | 1.10.0 | MIT OR Apache-2.0 |
| [quick-error](http://github.com/tailhook/quick-error) | 1.2.3 | MIT/Apache-2.0 |
| [quinn](https://github.com/quinn-rs/quinn) | 0.11.9 | MIT OR Apache-2.0 |
| [quinn-proto](https://github.com/quinn-rs/quinn) | 0.11.14 | MIT OR Apache-2.0 |
| [quinn-udp](https://github.com/quinn-rs/quinn) | 0.5.14 | MIT OR Apache-2.0 |
| [quote](https://github.com/dtolnay/quote) | 1.0.45 | MIT OR Apache-2.0 |
| [r-efi](https://github.com/r-efi/r-efi) | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| [radium](https://github.com/bitvecto-rs/radium) | 0.7.0 | MIT |
| [rand](https://github.com/rust-random/rand) | 0.9.2 | MIT OR Apache-2.0 |
| [rand_chacha](https://github.com/rust-random/rand) | 0.9.0 | MIT OR Apache-2.0 |
| [rand_core](https://github.com/rust-random/rand) | 0.9.5 | MIT OR Apache-2.0 |
| [rand_xorshift](https://github.com/rust-random/rngs) | 0.4.0 | MIT OR Apache-2.0 |
| [rayon](https://github.com/rayon-rs/rayon) | 1.11.0 | MIT OR Apache-2.0 |
| [rayon-core](https://github.com/rayon-rs/rayon) | 1.13.0 | MIT OR Apache-2.0 |
| [redox_syscall](https://gitlab.redox-os.org/redox-os/syscall) | 0.7.3 | MIT |
| [ref-cast](https://github.com/dtolnay/ref-cast) | 1.0.25 | MIT OR Apache-2.0 |
| [ref-cast-impl](https://github.com/dtolnay/ref-cast) | 1.0.25 | MIT OR Apache-2.0 |
| [regex](https://github.com/rust-lang/regex) | 1.12.3 | MIT OR Apache-2.0 |
| [regex-automata](https://github.com/rust-lang/regex) | 0.4.14 | MIT OR Apache-2.0 |
| [regex-syntax](https://github.com/rust-lang/regex) | 0.8.10 | MIT OR Apache-2.0 |
| [reqwest](https://github.com/seanmonstar/reqwest) | 0.12.28 | MIT OR Apache-2.0 |
| [rfc6979](https://github.com/RustCrypto/signatures/tree/master/rfc6979) | 0.4.0 | Apache-2.0 OR MIT |
| [ring](https://github.com/briansmith/ring) | 0.17.14 | Apache-2.0 AND ISC |
| [rustc-hash](https://github.com/rust-lang/rustc-hash) | 2.1.1 | Apache-2.0 OR MIT |
| [rustc_version](https://github.com/djc/rustc-version-rs) | 0.4.1 | MIT OR Apache-2.0 |
| [rustix](https://github.com/bytecodealliance/rustix) | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [rustls](https://github.com/rustls/rustls) | 0.23.37 | Apache-2.0 OR ISC OR MIT |
| [rustls-pki-types](https://github.com/rustls/pki-types) | 1.14.0 | MIT OR Apache-2.0 |
| [rustls-webpki](https://github.com/rustls/webpki) | 0.103.9 | ISC |
| [rustversion](https://github.com/dtolnay/rustversion) | 1.0.22 | MIT OR Apache-2.0 |
| [rusty-fork](https://github.com/altsysrq/rusty-fork) | 0.3.1 | MIT/Apache-2.0 |
| [ryu](https://github.com/dtolnay/ryu) | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| [same-file](https://github.com/BurntSushi/same-file) | 1.0.6 | Unlicense/MIT |
| [schemars](https://github.com/GREsau/schemars) | 1.2.1 | MIT |
| [scopeguard](https://github.com/bluss/scopeguard) | 1.2.0 | MIT OR Apache-2.0 |
| [sec1](https://github.com/RustCrypto/formats/tree/master/sec1) | 0.7.3 | Apache-2.0 OR MIT |
| [secp256k1](https://github.com/rust-bitcoin/rust-secp256k1/) | 0.26.0 | CC0-1.0 |
| [secp256k1-sys](https://github.com/rust-bitcoin/rust-secp256k1/) | 0.8.2 | CC0-1.0 |
| [semver](https://github.com/dtolnay/semver) | 1.0.27 | MIT OR Apache-2.0 |
| [serde](https://github.com/serde-rs/serde) | 1.0.228 | MIT OR Apache-2.0 |
| [serde_core](https://github.com/serde-rs/serde) | 1.0.228 | MIT OR Apache-2.0 |
| [serde_derive](https://github.com/serde-rs/serde) | 1.0.228 | MIT OR Apache-2.0 |
| [serde_json](https://github.com/serde-rs/json) | 1.0.149 | MIT OR Apache-2.0 |
| [serde_spanned](https://github.com/toml-rs/toml) | 0.6.9 | MIT OR Apache-2.0 |
| [serde_urlencoded](https://github.com/nox/serde_urlencoded) | 0.7.1 | MIT/Apache-2.0 |
| [serde_with](https://github.com/jonasbb/serde_with/) | 3.17.0 | MIT OR Apache-2.0 |
| [serde_with_macros](https://github.com/jonasbb/serde_with/) | 3.17.0 | MIT OR Apache-2.0 |
| [sha2](https://github.com/RustCrypto/hashes) | 0.9.9 | MIT OR Apache-2.0 |
| [sharded-slab](https://github.com/hawkw/sharded-slab) | 0.1.7 | MIT |
| [shlex](https://github.com/comex/rust-shlex) | 1.3.0 | MIT OR Apache-2.0 |
| [signal-hook-registry](https://github.com/vorner/signal-hook) | 1.4.8 | MIT OR Apache-2.0 |
| [signature](https://github.com/RustCrypto/traits/tree/master/signature) | 2.2.0 | Apache-2.0 OR MIT |
| [slab](https://github.com/tokio-rs/slab) | 0.4.12 | MIT |
| [smallvec](https://github.com/servo/rust-smallvec) | 1.15.1 | MIT OR Apache-2.0 |
| [snap](https://github.com/BurntSushi/rust-snappy) | 1.1.1 | BSD-3-Clause |
| [socket2](https://github.com/rust-lang/socket2) | 0.6.3 | MIT OR Apache-2.0 |
| [spki](https://github.com/RustCrypto/formats/tree/master/spki) | 0.7.3 | Apache-2.0 OR MIT |
| [stable_deref_trait](https://github.com/storyyeller/stable_deref_trait) | 1.2.1 | MIT OR Apache-2.0 |
| [static_assertions](https://github.com/nvzqz/static-assertions-rs) | 1.1.0 | MIT OR Apache-2.0 |
| [strsim](https://github.com/rapidfuzz/strsim-rs) | 0.11.1 | MIT |
| [strum](https://github.com/Peternator7/strum) | 0.26.3 | MIT |
| [strum_macros](https://github.com/Peternator7/strum) | 0.26.4 | MIT |
| [subtle](https://github.com/dalek-cryptography/subtle) | 2.6.1 | BSD-3-Clause |
| [syn](https://github.com/dtolnay/syn) | 2.0.117 | MIT OR Apache-2.0 |
| [sync_wrapper](https://github.com/Actyx/sync_wrapper) | 1.0.2 | Apache-2.0 |
| [synstructure](https://github.com/mystor/synstructure) | 0.13.2 | MIT |
| [tap](https://github.com/myrrlyn/tap) | 1.0.1 | MIT |
| [tar](https://github.com/alexcrichton/tar-rs) | 0.4.44 | MIT OR Apache-2.0 |
| [tempfile](https://github.com/Stebalien/tempfile) | 3.27.0 | MIT OR Apache-2.0 |
| [thiserror](https://github.com/dtolnay/thiserror) | 2.0.18 | MIT OR Apache-2.0 |
| [thiserror-impl](https://github.com/dtolnay/thiserror) | 2.0.18 | MIT OR Apache-2.0 |
| [thread_local](https://github.com/Amanieu/thread_local-rs) | 1.1.9 | MIT OR Apache-2.0 |
| [threadpool](https://github.com/rust-threadpool/rust-threadpool) | 1.8.1 | MIT/Apache-2.0 |
| [time](https://github.com/time-rs/time) | 0.3.47 | MIT OR Apache-2.0 |
| [time-core](https://github.com/time-rs/time) | 0.1.8 | MIT OR Apache-2.0 |
| [time-macros](https://github.com/time-rs/time) | 0.2.27 | MIT OR Apache-2.0 |
| [tinystr](https://github.com/unicode-org/icu4x) | 0.8.2 | Unicode-3.0 |
| [tinytemplate](https://github.com/bheisler/TinyTemplate) | 1.2.1 | Apache-2.0 OR MIT |
| [tinyvec](https://github.com/Lokathor/tinyvec) | 1.10.0 | Zlib OR Apache-2.0 OR MIT |
| [tinyvec_macros](https://github.com/Soveu/tinyvec_macros) | 0.1.1 | MIT OR Apache-2.0 OR Zlib |
| [tokio](https://github.com/tokio-rs/tokio) | 1.50.0 | MIT |
| [tokio-macros](https://github.com/tokio-rs/tokio) | 2.6.1 | MIT |
| [tokio-rustls](https://github.com/rustls/tokio-rustls) | 0.26.4 | MIT OR Apache-2.0 |
| [tokio-util](https://github.com/tokio-rs/tokio) | 0.7.18 | MIT |
| [toml](https://github.com/toml-rs/toml) | 0.8.23 | MIT OR Apache-2.0 |
| [toml_datetime](https://github.com/toml-rs/toml) | 0.6.11 | MIT OR Apache-2.0 |
| [toml_edit](https://github.com/toml-rs/toml) | 0.22.27 | MIT OR Apache-2.0 |
| [toml_write](https://github.com/toml-rs/toml) | 0.1.2 | MIT OR Apache-2.0 |
| [tower](https://github.com/tower-rs/tower) | 0.5.3 | MIT |
| [tower-http](https://github.com/tower-rs/tower-http) | 0.6.8 | MIT |
| [tower-layer](https://github.com/tower-rs/tower) | 0.3.3 | MIT |
| [tower-service](https://github.com/tower-rs/tower) | 0.3.3 | MIT |
| [tracing](https://github.com/tokio-rs/tracing) | 0.1.44 | MIT |
| [tracing-appender](https://github.com/tokio-rs/tracing) | 0.2.4 | MIT |
| [tracing-attributes](https://github.com/tokio-rs/tracing) | 0.1.31 | MIT |
| [tracing-core](https://github.com/tokio-rs/tracing) | 0.1.36 | MIT |
| [tracing-log](https://github.com/tokio-rs/tracing) | 0.2.0 | MIT |
| [tracing-serde](https://github.com/tokio-rs/tracing) | 0.2.0 | MIT |
| [tracing-subscriber](https://github.com/tokio-rs/tracing) | 0.3.22 | MIT |
| [try-lock](https://github.com/seanmonstar/try-lock) | 0.2.5 | MIT |
| [typed-arena](https://github.com/SimonSapin/rust-typed-arena) | 2.0.2 | MIT |
| [typenum](https://github.com/paholg/typenum) | 1.19.0 | MIT OR Apache-2.0 |
| [unarray](https://github.com/cameron1024/unarray) | 0.1.4 | MIT OR Apache-2.0 |
| [unicode-ident](https://github.com/dtolnay/unicode-ident) | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| [unicode-segmentation](https://github.com/unicode-rs/unicode-segmentation) | 1.12.0 | MIT OR Apache-2.0 |
| [unicode-width](https://github.com/unicode-rs/unicode-width) | 0.2.2 | MIT OR Apache-2.0 |
| [unicode-xid](https://github.com/unicode-rs/unicode-xid) | 0.2.6 | MIT OR Apache-2.0 |
| [untrusted](https://github.com/briansmith/untrusted) | 0.9.0 | ISC |
| [uplc](https://github.com/aiken-lang/aiken) | 1.1.21 | Apache-2.0 |
| [url](https://github.com/servo/rust-url) | 2.5.8 | MIT OR Apache-2.0 |
| [utf8_iter](https://github.com/hsivonen/utf8_iter) | 1.0.4 | Apache-2.0 OR MIT |
| [utf8parse](https://github.com/alacritty/vte) | 0.2.2 | Apache-2.0 OR MIT |
| [uuid](https://github.com/uuid-rs/uuid) | 1.22.0 | Apache-2.0 OR MIT |
| [valuable](https://github.com/tokio-rs/valuable) | 0.1.1 | MIT |
| [version_check](https://github.com/SergioBenitez/version_check) | 0.9.5 | MIT/Apache-2.0 |
| vrf_dalek | 0.1.0 | Unknown |
| [wait-timeout](https://github.com/alexcrichton/wait-timeout) | 0.2.1 | MIT/Apache-2.0 |
| [walkdir](https://github.com/BurntSushi/walkdir) | 2.5.0 | Unlicense/MIT |
| [want](https://github.com/seanmonstar/want) | 0.3.1 | MIT |
| [wasi](https://github.com/bytecodealliance/wasi) | 0.9.0+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wasip2](https://github.com/bytecodealliance/wasi-rs) | 1.0.2+wasi-0.2.9 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wasip3](https://github.com/bytecodealliance/wasi-rs) | 0.4.0+wasi-0.3.0-rc-2026-01-06 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wasm-bindgen](https://github.com/wasm-bindgen/wasm-bindgen) | 0.2.114 | MIT OR Apache-2.0 |
| [wasm-bindgen-futures](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/futures) | 0.4.64 | MIT OR Apache-2.0 |
| [wasm-bindgen-macro](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro) | 0.2.114 | MIT OR Apache-2.0 |
| [wasm-bindgen-macro-support](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro-support) | 0.2.114 | MIT OR Apache-2.0 |
| [wasm-bindgen-shared](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared) | 0.2.114 | MIT OR Apache-2.0 |
| [wasm-encoder](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-encoder) | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wasm-metadata](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-metadata) | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wasm-streams](https://github.com/MattiasBuelens/wasm-streams/) | 0.4.2 | MIT OR Apache-2.0 |
| [wasmparser](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasmparser) | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [web-sys](https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/web-sys) | 0.3.91 | MIT OR Apache-2.0 |
| [web-time](https://github.com/daxpedda/web-time) | 1.1.0 | MIT OR Apache-2.0 |
| [webpki-roots](https://github.com/rustls/webpki-roots) | 1.0.6 | CDLA-Permissive-2.0 |
| [winapi](https://github.com/retep998/winapi-rs) | 0.3.9 | MIT/Apache-2.0 |
| [winapi-i686-pc-windows-gnu](https://github.com/retep998/winapi-rs) | 0.4.0 | MIT/Apache-2.0 |
| [winapi-util](https://github.com/BurntSushi/winapi-util) | 0.1.11 | Unlicense OR MIT |
| [winapi-x86_64-pc-windows-gnu](https://github.com/retep998/winapi-rs) | 0.4.0 | MIT/Apache-2.0 |
| [windows-core](https://github.com/microsoft/windows-rs) | 0.62.2 | MIT OR Apache-2.0 |
| [windows-implement](https://github.com/microsoft/windows-rs) | 0.60.2 | MIT OR Apache-2.0 |
| [windows-interface](https://github.com/microsoft/windows-rs) | 0.59.3 | MIT OR Apache-2.0 |
| [windows-link](https://github.com/microsoft/windows-rs) | 0.2.1 | MIT OR Apache-2.0 |
| [windows-result](https://github.com/microsoft/windows-rs) | 0.4.1 | MIT OR Apache-2.0 |
| [windows-strings](https://github.com/microsoft/windows-rs) | 0.5.1 | MIT OR Apache-2.0 |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.61.2 | MIT OR Apache-2.0 |
| [windows-targets](https://github.com/microsoft/windows-rs) | 0.53.5 | MIT OR Apache-2.0 |
| [windows_aarch64_gnullvm](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_aarch64_msvc](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_i686_gnu](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_i686_gnullvm](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_i686_msvc](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_x86_64_gnu](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_x86_64_gnullvm](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [windows_x86_64_msvc](https://github.com/microsoft/windows-rs) | 0.53.1 | MIT OR Apache-2.0 |
| [winnow](https://github.com/winnow-rs/winnow) | 0.7.15 | MIT |
| [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen) | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wit-bindgen-core](https://github.com/bytecodealliance/wit-bindgen) | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wit-bindgen-rust](https://github.com/bytecodealliance/wit-bindgen) | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wit-bindgen-rust-macro](https://github.com/bytecodealliance/wit-bindgen) | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wit-component](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wit-component) | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [wit-parser](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wit-parser) | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| [writeable](https://github.com/unicode-org/icu4x) | 0.6.2 | Unicode-3.0 |
| [wyz](https://github.com/myrrlyn/wyz) | 0.5.1 | MIT |
| [xattr](https://github.com/Stebalien/xattr) | 1.6.1 | MIT OR Apache-2.0 |
| [yoke](https://github.com/unicode-org/icu4x) | 0.8.1 | Unicode-3.0 |
| [yoke-derive](https://github.com/unicode-org/icu4x) | 0.8.1 | Unicode-3.0 |
| [zerocopy](https://github.com/google/zerocopy) | 0.8.42 | BSD-2-Clause OR Apache-2.0 OR MIT |
| [zerocopy-derive](https://github.com/google/zerocopy) | 0.8.42 | BSD-2-Clause OR Apache-2.0 OR MIT |
| [zerofrom](https://github.com/unicode-org/icu4x) | 0.1.6 | Unicode-3.0 |
| [zerofrom-derive](https://github.com/unicode-org/icu4x) | 0.1.6 | Unicode-3.0 |
| [zeroize](https://github.com/RustCrypto/utils) | 1.8.2 | Apache-2.0 OR MIT |
| [zeroize_derive](https://github.com/RustCrypto/utils/tree/master/zeroize/derive) | 1.4.3 | Apache-2.0 OR MIT |
| [zerotrie](https://github.com/unicode-org/icu4x) | 0.2.3 | Unicode-3.0 |
| [zerovec](https://github.com/unicode-org/icu4x) | 0.11.5 | Unicode-3.0 |
| [zerovec-derive](https://github.com/unicode-org/icu4x) | 0.11.2 | Unicode-3.0 |
| [zmij](https://github.com/dtolnay/zmij) | 1.0.21 | MIT |
| [zstd](https://github.com/gyscos/zstd-rs) | 0.13.3 | MIT |
| [zstd-safe](https://github.com/gyscos/zstd-rs) | 7.2.4 | MIT OR Apache-2.0 |
| [zstd-sys](https://github.com/gyscos/zstd-rs) | 2.0.16+zstd.1.5.7 | MIT/Apache-2.0 |

## Regenerating This Page

This page is generated from `Cargo.lock` metadata. To regenerate after dependency changes:

```bash
python3 scripts/generate-licenses.py > docs/src/reference/third-party-licenses.md
```
