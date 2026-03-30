# Nightly Benchmark Results — 2026-03-30

Machine: GitHub Actions ubuntu-latest
Branch: main (590bc35)

## Storage Benchmarks
```
[1m[92m    Updating[0m crates.io index
[1m[92m    Updating[0m git repository `https://github.com/iquerejeta/curve25519-dalek`
[1m[92m    Updating[0m git repository `https://github.com/input-output-hk/vrf`
[1m[92m    Updating[0m git repository `https://github.com/aiken-lang/aiken.git`
[1m[92m Downloading[0m crates ...
[1m[92m  Downloaded[0m anes v0.1.6
[1m[92m  Downloaded[0m ciborium-io v0.2.2
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m cfg-if v1.0.4
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m cast v0.3.0
[1m[92m  Downloaded[0m arrayref v0.3.9
[1m[92m  Downloaded[0m parking_lot_core v0.9.12
[1m[92m  Downloaded[0m plotters-backend v0.3.7
[1m[92m  Downloaded[0m bech32 v0.9.1
[1m[92m  Downloaded[0m autocfg v1.5.0
[1m[92m  Downloaded[0m rand_chacha v0.3.1
[1m[92m  Downloaded[0m arrayvec v0.7.6
[1m[92m  Downloaded[0m anstyle v1.0.14
[1m[92m  Downloaded[0m plotters-svg v0.3.7
[1m[92m  Downloaded[0m block-buffer v0.9.0
[1m[92m  Downloaded[0m byteorder v1.5.0
[1m[92m  Downloaded[0m base58 v0.2.0
[1m[92m  Downloaded[0m opaque-debug v0.3.1
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m base64ct v1.8.3
[1m[92m  Downloaded[0m powerfmt v0.2.0
[1m[92m  Downloaded[0m crc v3.4.0
[1m[92m  Downloaded[0m pallas-addresses v1.0.0-alpha.5
[1m[92m  Downloaded[0m dyn-clone v1.0.20
[1m[92m  Downloaded[0m num-conv v0.2.0
[1m[92m  Downloaded[0m pin-project-lite v0.2.17
[1m[92m  Downloaded[0m thiserror-impl v2.0.18
[1m[92m  Downloaded[0m parking_lot v0.12.5
[1m[92m  Downloaded[0m darling_macro v0.23.0
[1m[92m  Downloaded[0m crypto-common v0.1.6
[1m[92m  Downloaded[0m itoa v1.0.17
[1m[92m  Downloaded[0m curve25519-dalek-derive v0.1.1
[1m[92m  Downloaded[0m rand_core v0.5.1
[1m[92m  Downloaded[0m scopeguard v1.2.0
[1m[92m  Downloaded[0m ref-cast-impl v1.0.25
[1m[92m  Downloaded[0m fastrand v2.3.0
[1m[92m  Downloaded[0m subtle v2.6.1
[1m[92m  Downloaded[0m digest v0.9.0
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m ed25519 v2.2.3
[1m[92m  Downloaded[0m pkcs8 v0.10.2
[1m[92m  Downloaded[0m num-modular v0.6.1
[1m[92m  Downloaded[0m num-rational v0.4.2
[1m[92m  Downloaded[0m strsim v0.11.1
[1m[92m  Downloaded[0m minicbor-derive v0.16.2
[1m[92m  Downloaded[0m generic-array v0.14.9
[1m[92m  Downloaded[0m signature v2.2.0
[1m[92m  Downloaded[0m num-integer v0.1.46
[1m[92m  Downloaded[0m ciborium v0.2.2
[1m[92m  Downloaded[0m unicode-xid v0.2.6
[1m[92m  Downloaded[0m minicbor v0.25.1
[1m[92m  Downloaded[0m version_check v0.9.5
[1m[92m  Downloaded[0m zmij v1.0.21
[1m[92m  Downloaded[0m crc32fast v1.5.0
[1m[92m  Downloaded[0m cpufeatures v0.2.17
[1m[92m  Downloaded[0m half v2.7.1
[1m[92m  Downloaded[0m walkdir v2.5.0
[1m[92m  Downloaded[0m tracing-attributes v0.1.31
[1m[92m  Downloaded[0m derive_more-impl v1.0.0
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m unicode-ident v1.0.24
[1m[92m  Downloaded[0m typenum v1.19.0
[1m[92m  Downloaded[0m zerocopy-derive v0.8.46
[1m[92m  Downloaded[0m hashbrown v0.16.1
[1m[92m  Downloaded[0m chrono v0.4.44
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m regex v1.12.3
[1m[92m  Downloaded[0m serde_with v3.18.0
[1m[92m  Downloaded[0m time v0.3.47
[1m[92m  Downloaded[0m curve25519-dalek v4.1.3
[1m[92m  Downloaded[0m rayon v1.11.0
[1m[92m  Downloaded[0m cryptoxide v0.4.4
[1m[92m  Downloaded[0m zerocopy v0.8.46
[1m[92m  Downloaded[0m syn v2.0.117
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m clap_builder v4.6.0
[1m[92m  Downloaded[0m aho-corasick v1.1.4
[1m[92m  Downloaded[0m rustix v1.1.4
[1m[92m  Downloaded[0m curve25519-dalek v3.2.0
[1m[92m  Downloaded[0m tracing v0.1.44
[1m[92m  Downloaded[0m rayon-core v1.13.0
[1m[92m  Downloaded[0m plotters v0.3.7
[1m[92m  Downloaded[0m dashu-int v0.4.1
[1m[92m  Downloaded[0m serde v1.0.228
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m schemars v1.2.1
[1m[92m  Downloaded[0m libc v0.2.183
[1m[92m  Downloaded[0m itertools v0.13.0
[1m[92m  Downloaded[0m ed25519-dalek v2.2.0
[1m[92m  Downloaded[0m tracing-core v0.1.36
[1m[92m  Downloaded[0m tokio v1.50.0
[1m[92m  Downloaded[0m tempfile v3.27.0
[1m[92m  Downloaded[0m pallas-traverse v1.0.0-alpha.5
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m serde_derive v1.0.228
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m schemars v0.9.0
[1m[92m  Downloaded[0m rand v0.9.2
[1m[92m  Downloaded[0m num-bigint v0.4.6
[1m[92m  Downloaded[0m mio v1.1.1
[1m[92m  Downloaded[0m itertools v0.10.5
[1m[92m  Downloaded[0m indexmap v2.13.0
[1m[92m  Downloaded[0m hashbrown v0.12.3
[1m[92m  Downloaded[0m der v0.7.10
[1m[92m  Downloaded[0m criterion v0.5.1
[1m[92m  Downloaded[0m smallvec v1.15.1
[1m[92m  Downloaded[0m serde_with_macros v3.18.0
[1m[92m  Downloaded[0m semver v1.0.27
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m num-traits v0.2.19
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m darling_core v0.23.0
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m rand v0.8.5
[1m[92m  Downloaded[0m indexmap v1.9.3
[1m[92m  Downloaded[0m clap v4.6.0
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m sha2 v0.10.9
[1m[92m  Downloaded[0m sha2 v0.9.9
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m derive_more v1.0.0
[1m[92m  Downloaded[0m crossbeam-utils v0.8.21
[1m[92m  Downloaded[0m bytes v1.11.1
[1m[92m  Downloaded[0m zeroize v1.8.2
[1m[92m  Downloaded[0m tokio-macros v2.6.1
[1m[92m  Downloaded[0m time-macros v0.2.27
[1m[92m  Downloaded[0m static_assertions v1.1.0
[1m[92m  Downloaded[0m spki v0.7.3
[1m[92m  Downloaded[0m same-file v1.0.6
[1m[92m  Downloaded[0m rustversion v1.0.22
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m num-order v1.2.0
[1m[92m  Downloaded[0m minicbor v0.26.5
[1m[92m  Downloaded[0m memmap2 v0.9.10
[1m[92m  Downloaded[0m iana-time-zone v0.1.65
[1m[92m  Downloaded[0m getrandom v0.4.2
[1m[92m  Downloaded[0m tinytemplate v1.2.1
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m ref-cast v1.0.25
[1m[92m  Downloaded[0m crossbeam-epoch v0.9.18
[1m[92m  Downloaded[0m crossbeam-deque v0.8.6
[1m[92m  Downloaded[0m const-oid v0.9.6
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m getrandom v0.1.16
[1m[92m  Downloaded[0m darling v0.23.0
[1m[92m  Downloaded[0m bech32 v0.11.1
[1m[92m  Downloaded[0m time-core v0.1.8
[1m[92m  Downloaded[0m minicbor-derive v0.15.3
[1m[92m  Downloaded[0m lock_api v0.4.14
[1m[92m  Downloaded[0m either v1.15.0
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m deranged v0.5.8
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
[1m[92m  Downloaded[0m rustc_version v0.4.1
[1m[92m  Downloaded[0m pallas-primitives v1.0.0-alpha.5
[1m[92m  Downloaded[0m constant_time_eq v0.4.2
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m pallas-crypto v1.0.0-alpha.5
[1m[92m  Downloaded[0m pallas-codec v1.0.0-alpha.5
[1m[92m  Downloaded[0m oorandom v11.1.5
[1m[92m  Downloaded[0m criterion-plot v0.5.0
[1m[92m  Downloaded[0m bitflags v2.11.0
[1m[92m  Downloaded[0m dashu-base v0.4.1
[1m[92m  Downloaded[0m ciborium-ll v0.2.2
[1m[92m  Downloaded[0m blake2b_simd v1.0.4
[1m[92m  Downloaded[0m blake2 v0.10.6
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m is-terminal v0.4.17
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m clap_lex v1.1.0
[1m[92m  Downloaded[0m ident_case v1.0.1
[1m[92m  Downloaded[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m proc-macro2 v1.0.106
[1m[92m   Compiling[0m quote v1.0.45
[1m[92m   Compiling[0m unicode-ident v1.0.24
[1m[92m   Compiling[0m cfg-if v1.0.4
[1m[92m   Compiling[0m libc v0.2.183
[1m[92m   Compiling[0m typenum v1.19.0
[1m[92m   Compiling[0m version_check v0.9.5
[1m[92m   Compiling[0m generic-array v0.14.9
[1m[92m   Compiling[0m zerocopy v0.8.46
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m subtle v2.6.1
[1m[92m   Compiling[0m block-buffer v0.10.4
[1m[92m   Compiling[0m crypto-common v0.1.6
[1m[92m   Compiling[0m autocfg v1.5.0
[1m[92m   Compiling[0m digest v0.10.7
[1m[92m   Compiling[0m semver v1.0.27
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m strsim v0.11.1
[1m[92m   Compiling[0m cpufeatures v0.2.17
[1m[92m   Compiling[0m ident_case v1.0.1
[1m[92m   Compiling[0m getrandom v0.3.4
[1m[92m   Compiling[0m rustc_version v0.4.1
[1m[92m   Compiling[0m getrandom v0.2.17
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m rand_core v0.6.4
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m   Compiling[0m minicbor v0.26.5
[1m[92m   Compiling[0m rand_core v0.9.5
[1m[92m   Compiling[0m hex v0.4.3
[1m[92m   Compiling[0m signature v2.2.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m sha2 v0.10.9
[1m[92m   Compiling[0m memchr v2.8.0
[1m[92m   Compiling[0m getrandom v0.1.16
[1m[92m   Compiling[0m either v1.15.0
[1m[92m   Compiling[0m itoa v1.0.17
[1m[92m   Compiling[0m darling_core v0.23.0
[1m[92m   Compiling[0m crossbeam-utils v0.8.21
[1m[92m   Compiling[0m cryptoxide v0.4.4
[1m[92m   Compiling[0m zerocopy-derive v0.8.46
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m darling_macro v0.23.0
[1m[92m   Compiling[0m thiserror-impl v1.0.69
[1m[92m   Compiling[0m darling v0.23.0
[1m[92m   Compiling[0m curve25519-dalek-derive v0.1.1
[1m[92m   Compiling[0m minicbor-derive v0.16.2
[1m[92m   Compiling[0m ed25519 v2.2.3
[1m[92m   Compiling[0m serde_with_macros v3.18.0
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m digest v0.9.0
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m unicode-xid v0.2.6
[1m[92m   Compiling[0m derive_more-impl v1.0.0
[1m[92m   Compiling[0m serde_with v3.18.0
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m rand_core v0.5.1
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m minicbor-derive v0.15.3
[1m[92m   Compiling[0m pallas-codec v1.0.0-alpha.5
[1m[92m   Compiling[0m bytes v1.11.1
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.5
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m arrayvec v0.7.6
[1m[92m   Compiling[0m iana-time-zone v0.1.65
[1m[92m   Compiling[0m arrayref v0.3.9
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m   Compiling[0m rustversion v1.0.22
[1m[92m   Compiling[0m parking_lot_core v0.9.12
[1m[92m   Compiling[0m constant_time_eq v0.4.2
[1m[92m   Compiling[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m blake2b_simd v1.0.4
[1m[92m   Compiling[0m crc v3.4.0
[1m[92m   Compiling[0m chrono v0.4.44
[1m[92m   Compiling[0m crossbeam-epoch v0.9.18
[1m[92m   Compiling[0m derive_more v1.0.0
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m blake2 v0.10.6
[1m[92m   Compiling[0m block-buffer v0.9.0
[1m[92m   Compiling[0m bech32 v0.9.1
[1m[92m   Compiling[0m bech32 v0.11.1
[1m[92m   Compiling[0m rayon-core v1.13.0
[1m[92m   Compiling[0m pin-project-lite v0.2.17
[1m[92m   Compiling[0m base58 v0.2.0
[1m[92m   Compiling[0m num-modular v0.6.1
[1m[92m   Compiling[0m opaque-debug v0.3.1
[1m[92m   Compiling[0m scopeguard v1.2.0
[1m[92m   Compiling[0m smallvec v1.15.1
[1m[92m   Compiling[0m lock_api v0.4.14
[1m[92m   Compiling[0m sha2 v0.9.9
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m num-order v1.2.0
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m crossbeam-deque v0.8.6
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.5
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m rand_chacha v0.3.1
[1m[92m   Compiling[0m tracing-core v0.1.36
[1m[92m   Compiling[0m tracing-attributes v0.1.31
[1m[92m   Compiling[0m itertools v0.13.0
[1m[92m   Compiling[0m errno v0.3.14
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m   Compiling[0m ciborium-io v0.2.2
[1m[92m   Compiling[0m clap_lex v1.1.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m dashu-base v0.4.1
[1m[92m   Compiling[0m anstyle v1.0.14
[1m[92m   Compiling[0m plotters-backend v0.3.7
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m   Compiling[0m crc32fast v1.5.0
[1m[92m   Compiling[0m static_assertions v1.1.0
[1m[92m   Compiling[0m dashu-int v0.4.1
[1m[92m   Compiling[0m plotters-svg v0.3.7
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m clap_builder v4.6.0
[1m[92m   Compiling[0m tracing v0.1.44
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m signal-hook-registry v1.4.8
[1m[92m   Compiling[0m rand v0.8.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m parking_lot v0.12.5
[1m[92m   Compiling[0m tokio-macros v2.6.1
[1m[92m   Compiling[0m itertools v0.10.5
[1m[92m   Compiling[0m socket2 v0.6.3
[1m[92m   Compiling[0m mio v1.1.1
[1m[92m   Compiling[0m bitflags v2.11.0
[1m[92m   Compiling[0m same-file v1.0.6
[1m[92m   Compiling[0m cast v0.3.0
[1m[92m   Compiling[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m walkdir v2.5.0
[1m[92m   Compiling[0m criterion-plot v0.5.0
[1m[92m   Compiling[0m tokio v1.50.0
[1m[92m   Compiling[0m rayon v1.11.0
[1m[92m   Compiling[0m torsten-crypto v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-crypto)
[1m[92m   Compiling[0m clap v4.6.0
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m torsten-serialization v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-serialization)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m is-terminal v0.4.17
[1m[92m   Compiling[0m memmap2 v0.9.10
[1m[92m   Compiling[0m oorandom v11.1.5
[1m[92m   Compiling[0m anes v0.1.6
[1m[92m   Compiling[0m fastrand v2.3.0
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m torsten-storage v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 08s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-51833895afe17d60)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 41.3s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 41.307 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [3.7278 s 3.8778 s 4.1194 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 371.0s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 370.97 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [29.939 ms 30.055 ms 30.174 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 4168.4s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 4168.4 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [348.47 ms 354.40 ms 360.72 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 391.6s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 391.65 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [29.144 ms 29.245 ms 29.348 ms]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) low mild
  1 (1.00%) high mild

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 350.6s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 350.56 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [28.916 ms 29.014 ms 29.113 ms]
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) low mild
  4 (4.00%) high mild

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 369.8s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 369.78 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [30.171 ms 30.281 ms 30.419 ms]
Found 3 outliers among 100 measurements (3.00%)
  1 (1.00%) high mild
  2 (2.00%) high severe

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.7s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 7.7080 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [6.3102 ms 6.4434 ms 6.5858 ms]

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.9s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 35.931 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.5705 s 3.6518 s 3.7296 s]
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 36.9s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 36.855 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [3.6881 s 3.7509 s 3.8148 s]
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.7s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 37.707 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [29.127 ms 29.375 ms 29.678 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 36.9s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 36.903 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [29.173 ms 29.471 ms 29.721 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.1s, or reduce sample count to 50.
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 9.0821 s (100 iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [90.677 ms 90.803 ms 90.949 ms]
Found 3 outliers among 100 measurements (3.00%)
  1 (1.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.1636 s (25k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [204.75 µs 205.00 µs 205.25 µs]
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.0058 s (1700 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [2.9428 ms 3.0723 ms 3.3212 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high severe
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 113.7s, or reduce sample count to 10.
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 113.65 s (100 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [862.37 ms 863.28 ms 864.40 ms]
Found 6 outliers among 100 measurements (6.00%)
  3 (3.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.8s, enable flat sampling, or reduce sample count to 50.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 8.8340 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.7396 ms 1.7433 ms 1.7471 ms]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.1037 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [25.405 ms 25.454 ms 25.506 ms]
Found 4 outliers among 100 measurements (4.00%)
  4 (4.00%) high mild

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.2919 s (500 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [10.651 ms 10.675 ms 10.700 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.2760 s (500 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [10.538 ms 10.560 ms 10.582 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.0696 s (146k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [34.518 µs 34.543 µs 34.580 µs]
Found 8 outliers among 100 measurements (8.00%)
  3 (3.00%) high mild
  5 (5.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.0559 s (146k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [34.513 µs 34.546 µs 34.600 µs]
Found 6 outliers among 100 measurements (6.00%)
  3 (3.00%) high mild
  3 (3.00%) high severe

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 5.8610 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [14.576 ms 14.599 ms 14.624 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.0843 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [15.166 ms 15.195 ms 15.226 ms]
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 6.2620 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [311.25 µs 313.02 µs 315.89 µs]
Found 20 outliers among 100 measurements (20.00%)
  11 (11.00%) low severe
  4 (4.00%) low mild
  3 (3.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 6.3206 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [313.59 µs 314.22 µs 314.82 µs]
Found 18 outliers among 100 measurements (18.00%)
  6 (6.00%) low severe
  7 (7.00%) low mild
  4 (4.00%) high mild
  1 (1.00%) high severe

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 8.0770 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [800.58 µs 801.44 µs 802.34 µs]
Found 5 outliers among 100 measurements (5.00%)
  4 (4.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.4424 s (900 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.0598 ms 6.1008 ms 6.1535 ms]
Found 10 outliers among 100 measurements (10.00%)
  4 (4.00%) high mild
  6 (6.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2096 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.4715 ms 3.4758 ms 3.4824 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high severe
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 8.7353 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [43.851 ms 44.121 ms 44.460 ms]
Found 11 outliers among 100 measurements (11.00%)
  4 (4.00%) high mild
  7 (7.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.6543 s (800 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.0636 ms 7.0663 ms 7.0691 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.5s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 8.5293 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [84.818 ms 84.932 ms 85.064 ms]
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0342 s (323k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [17.458 µs 17.463 µs 17.470 µs]
Found 10 outliers among 100 measurements (10.00%)
  5 (5.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.1238 s (182k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [29.302 µs 29.413 µs 29.495 µs]
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0468 s (318k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [17.954 µs 17.968 µs 17.984 µs]
Found 8 outliers among 100 measurements (8.00%)
  2 (2.00%) low mild
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0480 s (247k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [20.207 µs 20.220 µs 20.233 µs]
Found 9 outliers among 100 measurements (9.00%)
  1 (1.00%) low severe
  5 (5.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0185 s (313k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [18.390 µs 18.403 µs 18.420 µs]
Found 12 outliers among 100 measurements (12.00%)
  1 (1.00%) low mild
  6 (6.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0188 s (252k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [19.687 µs 19.699 µs 19.714 µs]
Found 5 outliers among 100 measurements (5.00%)
  3 (3.00%) high mild
  2 (2.00%) high severe

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0222 s (454k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.039 µs 11.045 µs 11.052 µs]
Found 10 outliers among 100 measurements (10.00%)
  4 (4.00%) high mild
  6 (6.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.1522 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [42.460 µs 42.490 µs 42.526 µs]
Found 10 outliers among 100 measurements (10.00%)
  5 (5.00%) low severe
  1 (1.00%) low mild
  1 (1.00%) high mild
  3 (3.00%) high severe

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0404 s (6270 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [797.40 µs 799.56 µs 800.85 µs]
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.0424 s (825 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.0927 ms 6.1241 ms 6.1497 ms]
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.0440 s (1430 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.5057 ms 3.5358 ms 3.5892 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 7.2256 s (165 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [43.715 ms 43.765 ms 43.833 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.0781 s (715 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.0861 ms 7.0953 ms 7.1095 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 9.3382 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [84.412 ms 84.611 ms 84.759 ms]
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 6.2316 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [27.398 ms 27.680 ms 27.949 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 9.5s or enable flat sampling.
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 9.4996 s (55 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [171.72 ms 171.98 ms 172.30 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.2418 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [65.740 ms 66.033 ms 66.471 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 6.0459 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [301.32 ms 302.29 ms 303.51 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.2s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 8.2019 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [148.92 ms 149.05 ms 149.25 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.5s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 5.4962 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [558.25 ms 571.37 ms 585.35 ms]

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0007 s (324k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [15.422 µs 15.429 µs 15.432 µs]
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0005 s (165k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [29.946 µs 29.963 µs 30.008 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0006 s (317k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [15.809 µs 15.820 µs 15.830 µs]
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0005 s (244k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.349 µs 20.363 µs 20.376 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0000 s (314k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [15.927 µs 15.944 µs 15.979 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0001 s (250k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [19.881 µs 19.884 µs 19.888 µs]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0003 s (313k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [15.963 µs 15.971 µs 15.979 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0002 s (251k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [19.680 µs 19.736 µs 19.774 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0008 s (312k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [15.962 µs 15.997 µs 16.033 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0006 s (254k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.464 µs 19.480 µs 19.496 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0001 s (310k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [16.062 µs 16.166 µs 16.363 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0002 s (256k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.396 µs 19.401 µs 19.411 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 9.9944 s (110 iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [90.637 ms 90.770 ms 90.893 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0021 s (25k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [199.66 µs 202.48 µs 203.73 µs]
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 8.7128 s (20 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [432.90 ms 433.43 ms 434.03 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0318 s (6215 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [774.04 µs 775.22 µs 778.61 µs]
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.3s.
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 11.342 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [862.12 ms 863.07 ms 864.06 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0281 s (2915 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [1.7266 ms 1.7290 ms 1.7314 ms]
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 40.9s.
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 40.916 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [2.1484 s 2.1500 s 2.1517 s]
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.2759 s (1045 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [5.0669 ms 5.1268 ms 5.2155 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 76.4s.
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 76.443 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [23.052 s 23.277 s 23.591 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.3528 s (550 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [9.5811 ms 9.6045 ms 9.6311 ms]

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 42.4s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 42.435 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [4.3851 s 4.4519 s 4.5267 s]
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 233.3s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 233.31 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [22.629 s 22.932 s 23.215 s]
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 343.9s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 343.90 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [41.347 s 41.995 s 42.721 s]
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 1057.8s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 1057.8 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [101.24 s 102.96 s 104.62 s]

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m zeroize_derive v1.4.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m shlex v1.3.0
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m cc v1.2.57
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-codec v0.33.0
[1m[92m   Compiling[0m pallas-crypto v0.33.0
[1m[92m   Compiling[0m secp256k1-sys v0.8.2
[1m[92m   Compiling[0m log v0.4.29
[1m[92m   Compiling[0m radium v0.7.0
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m blst v0.3.16
[1m[92m   Compiling[0m indexmap v1.9.3
[1m[92m   Compiling[0m num_cpus v1.17.0
[1m[92m   Compiling[0m peg-runtime v0.8.5
[1m[92m   Compiling[0m tap v1.0.1
[1m[92m   Compiling[0m heck v0.5.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m strum_macros v0.26.4
[1m[92m   Compiling[0m wyz v0.5.1
[1m[92m   Compiling[0m peg-macros v0.8.5
[1m[92m   Compiling[0m threadpool v1.8.1
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-primitives v0.33.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m pallas-addresses v0.33.0
[1m[92m   Compiling[0m miette-derive v5.10.0
[1m[92m   Compiling[0m hashbrown v0.12.3
[1m[92m   Compiling[0m arrayvec v0.5.2
[1m[92m   Compiling[0m funty v2.0.0
[1m[92m   Compiling[0m unicode-width v0.1.14
[1m[92m   Compiling[0m unicode-segmentation v1.12.0
[1m[92m   Compiling[0m typed-arena v2.0.2
[1m[92m   Compiling[0m miette v5.10.0
[1m[92m   Compiling[0m bitvec v1.0.1
[1m[92m   Compiling[0m pretty v0.11.3
[1m[92m   Compiling[0m pallas-traverse v0.33.0
[1m[92m   Compiling[0m peg v0.8.5
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m secp256k1 v0.26.0
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m strum v0.26.3
[1m[92m   Compiling[0m wait-timeout v0.2.1
[1m[92m   Compiling[0m fs2 v0.4.3
[1m[92m   Compiling[0m fnv v1.0.7
[1m[92m   Compiling[0m bit-vec v0.8.0
[1m[92m   Compiling[0m hamming v0.1.3
[1m[92m   Compiling[0m quick-error v1.2.3
[1m[92m   Compiling[0m rusty-fork v0.3.1
[1m[92m   Compiling[0m uplc v1.1.21 (https://github.com/aiken-lang/aiken.git?rev=6806d52211fbb469ae13aa0e0290aeb6b9b3e8cf#6806d522)
[1m[92m   Compiling[0m bit-set v0.8.0
[1m[92m   Compiling[0m torsten-lsm v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-lsm)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m torsten-crypto v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-crypto)
[1m[92m   Compiling[0m torsten-serialization v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-serialization)
[1m[92m   Compiling[0m rand_xorshift v0.4.0
[1m[92m   Compiling[0m bincode v1.3.3
[1m[92m   Compiling[0m unarray v0.1.4
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m torsten-ledger v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 03s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-4b37a42019cfa4eb)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.9s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 25.876 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.5249 s 2.5362 s 2.5480 s]

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.6395 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [575.90 µs 576.42 µs 577.06 µs]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) high mild
  3 (3.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 5.0385 s (15k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [331.39 µs 331.64 µs 331.99 µs]
Found 7 outliers among 100 measurements (7.00%)
  3 (3.00%) high mild
  4 (4.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 6.0226 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [396.77 µs 397.21 µs 397.80 µs]
Found 5 outliers among 100 measurements (5.00%)
  5 (5.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 6.0908 s (20k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [299.86 µs 300.11 µs 300.38 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 48.8s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 48.824 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.5628 s 2.5764 s 2.5891 s]

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 25.161 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [257.00 ms 262.57 ms 267.87 ms]
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.1s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 25.085 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [250.79 ms 262.73 ms 270.98 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 32.8s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 32.836 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.3020 s 3.3165 s 3.3309 s]
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 33.5s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 33.540 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [114.78 ms 122.33 ms 129.64 ms]

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 29.4s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 29.429 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [294.26 ms 295.10 ms 295.94 ms]
Found 19 outliers among 100 measurements (19.00%)
  1 (1.00%) low severe
  2 (2.00%) low mild
  5 (5.00%) high mild
  11 (11.00%) high severe

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.4s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 35.388 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [579.00 ms 582.11 ms 585.40 ms]

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 25.198 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.5060 s 2.5153 s 2.5267 s]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high severe
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.1s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 25.128 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.5154 s 2.5219 s 2.5287 s]
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 25.158 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.5065 s 2.5178 s 2.5285 s]
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.3s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 25.260 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.4982 s 2.5077 s 2.5171 s]
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.0s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 24.987 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.4924 s 2.5104 s 2.5283 s]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 6.7976 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [445.75 µs 446.35 µs 446.89 µs]
Found 19 outliers among 100 measurements (19.00%)
  9 (9.00%) low severe
  4 (4.00%) low mild
  4 (4.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 6.7364 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [443.29 µs 443.79 µs 444.41 µs]
Found 16 outliers among 100 measurements (16.00%)
  5 (5.00%) low severe
  3 (3.00%) low mild
  4 (4.00%) high mild
  4 (4.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 6.7281 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [441.82 µs 442.30 µs 442.77 µs]
Found 11 outliers among 100 measurements (11.00%)
  2 (2.00%) low severe
  5 (5.00%) low mild
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 6.7140 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [440.72 µs 441.59 µs 442.43 µs]
Found 11 outliers among 100 measurements (11.00%)
  5 (5.00%) low severe
  1 (1.00%) low mild
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 6.7924 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [446.62 µs 446.92 µs 447.20 µs]
Found 14 outliers among 100 measurements (14.00%)
  2 (2.00%) low severe
  6 (6.00%) low mild
  2 (2.00%) high mild
  4 (4.00%) high severe

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 6.1409 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [201.03 ms 202.22 ms 203.35 ms]
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.8s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 11.808 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.1581 s 1.1684 s 1.1774 s]
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.1s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 25.143 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.4858 s 2.4913 s 2.4966 s]

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0169 s (13k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [358.65 µs 361.19 µs 363.04 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0030 s (12k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [406.40 µs 407.19 µs 408.12 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0111 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [440.31 µs 440.99 µs 442.24 µs]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 6.1586 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [11.007 ms 11.257 ms 11.552 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.8s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 11.773 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [109.08 ms 114.46 ms 119.58 ms]
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.8s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 24.816 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [253.21 ms 261.58 ms 268.38 ms]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low severe
  1 (10.00%) high severe

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.3825 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [28.947 ms 29.022 ms 29.136 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.9s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 7.8680 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [142.84 ms 143.09 ms 143.28 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 6.0110 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [296.19 ms 299.39 ms 301.93 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 150.8s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 150.82 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [15.068 s 15.106 s 15.143 s]
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 333.5s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 333.52 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [31.885 s 31.957 s 32.035 s]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low mild
  2 (20.00%) high mild

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0342 s (4125 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.2290 ms 1.2378 ms 1.2427 ms]
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0022 s (3300 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.5016 ms 1.5081 ms 1.5148 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) low mild

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 14.9s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 14.928 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.5205 s 1.5240 s 1.5273 s]
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 30.2s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 30.211 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.0483 s 3.0718 s 3.0981 s]

```

## LSM Stress Tests
```
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m zerocopy-derive v0.8.46
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m torsten-lsm v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-lsm)
[1m[92m   Compiling[0m zerocopy v0.8.46
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 27.59s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-e9460fbb829b3490)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 7.87s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
