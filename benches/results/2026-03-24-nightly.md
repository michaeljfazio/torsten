# Nightly Benchmark Results — 2026-03-24

Machine: GitHub Actions ubuntu-latest
Branch: main (f6d1682)

## Storage Benchmarks
```
[1m[92m    Updating[0m crates.io index
[1m[92m    Updating[0m git repository `https://github.com/iquerejeta/curve25519-dalek`
[1m[92m    Updating[0m git repository `https://github.com/input-output-hk/vrf`
[1m[92m    Updating[0m git repository `https://github.com/aiken-lang/aiken.git`
[1m[92m Downloading[0m crates ...
[1m[92m  Downloaded[0m arrayref v0.3.9
[1m[92m  Downloaded[0m tokio-macros v2.6.1
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m time-macros v0.2.27
[1m[92m  Downloaded[0m rand v0.8.5
[1m[92m  Downloaded[0m schemars v0.9.0
[1m[92m  Downloaded[0m rayon v1.11.0
[1m[92m  Downloaded[0m version_check v0.9.5
[1m[92m  Downloaded[0m subtle v2.6.1
[1m[92m  Downloaded[0m strsim v0.11.1
[1m[92m  Downloaded[0m unicode-xid v0.2.6
[1m[92m  Downloaded[0m plotters-svg v0.3.7
[1m[92m  Downloaded[0m zeroize v1.8.2
[1m[92m  Downloaded[0m tracing-attributes v0.1.31
[1m[92m  Downloaded[0m zmij v1.0.21
[1m[92m  Downloaded[0m unicode-ident v1.0.24
[1m[92m  Downloaded[0m zerocopy-derive v0.8.46
[1m[92m  Downloaded[0m minicbor v0.26.5
[1m[92m  Downloaded[0m serde v1.0.228
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m serde_with v3.18.0
[1m[92m  Downloaded[0m plotters v0.3.7
[1m[92m  Downloaded[0m itertools v0.13.0
[1m[92m  Downloaded[0m regex v1.12.3
[1m[92m  Downloaded[0m time v0.3.47
[1m[92m  Downloaded[0m mio v1.1.1
[1m[92m  Downloaded[0m zerocopy v0.8.46
[1m[92m  Downloaded[0m syn v2.0.117
[1m[92m  Downloaded[0m chrono v0.4.44
[1m[92m  Downloaded[0m num-bigint v0.4.6
[1m[92m  Downloaded[0m rustix v1.1.4
[1m[92m  Downloaded[0m criterion v0.5.1
[1m[92m  Downloaded[0m hashbrown v0.12.3
[1m[92m  Downloaded[0m rand v0.9.2
[1m[92m  Downloaded[0m hashbrown v0.16.1
[1m[92m  Downloaded[0m tracing v0.1.44
[1m[92m  Downloaded[0m getrandom v0.4.2
[1m[92m  Downloaded[0m bytes v1.11.1
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m schemars v1.2.1
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m rayon-core v1.13.0
[1m[92m  Downloaded[0m curve25519-dalek v3.2.0
[1m[92m  Downloaded[0m memmap2 v0.9.10
[1m[92m  Downloaded[0m indexmap v1.9.3
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m criterion-plot v0.5.0
[1m[92m  Downloaded[0m serde_derive v1.0.228
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m parking_lot_core v0.9.12
[1m[92m  Downloaded[0m pallas-crypto v1.0.0-alpha.5
[1m[92m  Downloaded[0m either v1.15.0
[1m[92m  Downloaded[0m ed25519 v2.2.3
[1m[92m  Downloaded[0m crypto-common v0.1.6
[1m[92m  Downloaded[0m crossbeam-deque v0.8.6
[1m[92m  Downloaded[0m crc-catalog v2.4.0
[1m[92m  Downloaded[0m walkdir v2.5.0
[1m[92m  Downloaded[0m typenum v1.19.0
[1m[92m  Downloaded[0m tracing-core v0.1.36
[1m[92m  Downloaded[0m libc v0.2.183
[1m[92m  Downloaded[0m tempfile v3.27.0
[1m[92m  Downloaded[0m smallvec v1.15.1
[1m[92m  Downloaded[0m tokio v1.50.0
[1m[92m  Downloaded[0m sha2 v0.10.9
[1m[92m  Downloaded[0m serde_with_macros v3.18.0
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m rand_core v0.5.1
[1m[92m  Downloaded[0m powerfmt v0.2.0
[1m[92m  Downloaded[0m pkcs8 v0.10.2
[1m[92m  Downloaded[0m pin-project-lite v0.2.17
[1m[92m  Downloaded[0m parking_lot v0.12.5
[1m[92m  Downloaded[0m pallas-traverse v1.0.0-alpha.5
[1m[92m  Downloaded[0m pallas-primitives v1.0.0-alpha.5
[1m[92m  Downloaded[0m num-rational v0.4.2
[1m[92m  Downloaded[0m num-integer v0.1.46
[1m[92m  Downloaded[0m num-conv v0.2.0
[1m[92m  Downloaded[0m minicbor-derive v0.15.3
[1m[92m  Downloaded[0m minicbor v0.25.1
[1m[92m  Downloaded[0m lock_api v0.4.14
[1m[92m  Downloaded[0m iana-time-zone v0.1.65
[1m[92m  Downloaded[0m getrandom v0.1.16
[1m[92m  Downloaded[0m tinytemplate v1.2.1
[1m[92m  Downloaded[0m thiserror-impl v2.0.18
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m static_assertions v1.1.0
[1m[92m  Downloaded[0m spki v0.7.3
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m sha2 v0.9.9
[1m[92m  Downloaded[0m semver v1.0.27
[1m[92m  Downloaded[0m rustversion v1.0.22
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m ciborium-ll v0.2.2
[1m[92m  Downloaded[0m rand_chacha v0.3.1
[1m[92m  Downloaded[0m plotters-backend v0.3.7
[1m[92m  Downloaded[0m minicbor-derive v0.16.2
[1m[92m  Downloaded[0m ciborium-io v0.2.2
[1m[92m  Downloaded[0m signature v2.2.0
[1m[92m  Downloaded[0m ref-cast-impl v1.0.25
[1m[92m  Downloaded[0m dyn-clone v1.0.20
[1m[92m  Downloaded[0m digest v0.9.0
[1m[92m  Downloaded[0m curve25519-dalek-derive v0.1.1
[1m[92m  Downloaded[0m clap_builder v4.6.0
[1m[92m  Downloaded[0m indexmap v2.13.0
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m fastrand v2.3.0
[1m[92m  Downloaded[0m derive_more v1.0.0
[1m[92m  Downloaded[0m clap v4.6.0
[1m[92m  Downloaded[0m cast v0.3.0
[1m[92m  Downloaded[0m byteorder v1.5.0
[1m[92m  Downloaded[0m blake2b_simd v1.0.4
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m num-traits v0.2.19
[1m[92m  Downloaded[0m itoa v1.0.17
[1m[92m  Downloaded[0m is-terminal v0.4.17
[1m[92m  Downloaded[0m ident_case v1.0.1
[1m[92m  Downloaded[0m generic-array v0.14.9
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m derive_more-impl v1.0.0
[1m[92m  Downloaded[0m deranged v0.5.8
[1m[92m  Downloaded[0m constant_time_eq v0.4.2
[1m[92m  Downloaded[0m const-oid v0.9.6
[1m[92m  Downloaded[0m cfg-if v1.0.4
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m bitflags v2.11.0
[1m[92m  Downloaded[0m bech32 v0.11.1
[1m[92m  Downloaded[0m time-core v0.1.8
[1m[92m  Downloaded[0m scopeguard v1.2.0
[1m[92m  Downloaded[0m same-file v1.0.6
[1m[92m  Downloaded[0m rustc_version v0.4.1
[1m[92m  Downloaded[0m ref-cast v1.0.25
[1m[92m  Downloaded[0m pallas-codec v1.0.0-alpha.5
[1m[92m  Downloaded[0m pallas-addresses v1.0.0-alpha.5
[1m[92m  Downloaded[0m num-order v1.2.0
[1m[92m  Downloaded[0m itertools v0.10.5
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m ed25519-dalek v2.2.0
[1m[92m  Downloaded[0m dashu-base v0.4.1
[1m[92m  Downloaded[0m darling_macro v0.23.0
[1m[92m  Downloaded[0m darling_core v0.23.0
[1m[92m  Downloaded[0m crc v3.4.0
[1m[92m  Downloaded[0m cpufeatures v0.2.17
[1m[92m  Downloaded[0m ciborium v0.2.2
[1m[92m  Downloaded[0m blake2 v0.10.6
[1m[92m  Downloaded[0m bech32 v0.9.1
[1m[92m  Downloaded[0m oorandom v11.1.5
[1m[92m  Downloaded[0m num-modular v0.6.1
[1m[92m  Downloaded[0m der v0.7.10
[1m[92m  Downloaded[0m crossbeam-utils v0.8.21
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m curve25519-dalek v4.1.3
[1m[92m  Downloaded[0m cryptoxide v0.4.4
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m aho-corasick v1.1.4
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m half v2.7.1
[1m[92m  Downloaded[0m dashu-int v0.4.1
[1m[92m  Downloaded[0m crossbeam-epoch v0.9.18
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m darling v0.23.0
[1m[92m  Downloaded[0m crc32fast v1.5.0
[1m[92m  Downloaded[0m base64ct v1.8.3
[1m[92m  Downloaded[0m autocfg v1.5.0
[1m[92m  Downloaded[0m arrayvec v0.7.6
[1m[92m  Downloaded[0m anstyle v1.0.14
[1m[92m  Downloaded[0m anes v0.1.6
[1m[92m  Downloaded[0m opaque-debug v0.3.1
[1m[92m  Downloaded[0m clap_lex v1.1.0
[1m[92m  Downloaded[0m block-buffer v0.9.0
[1m[92m  Downloaded[0m base58 v0.2.0
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
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
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m semver v1.0.27
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m getrandom v0.3.4
[1m[92m   Compiling[0m strsim v0.11.1
[1m[92m   Compiling[0m cpufeatures v0.2.17
[1m[92m   Compiling[0m ident_case v1.0.1
[1m[92m   Compiling[0m rustc_version v0.4.1
[1m[92m   Compiling[0m getrandom v0.2.17
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m rand_core v0.6.4
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m minicbor v0.26.5
[1m[92m   Compiling[0m rand_core v0.9.5
[1m[92m   Compiling[0m hex v0.4.3
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m   Compiling[0m signature v2.2.0
[1m[92m   Compiling[0m sha2 v0.10.9
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m getrandom v0.1.16
[1m[92m   Compiling[0m either v1.15.0
[1m[92m   Compiling[0m itoa v1.0.17
[1m[92m   Compiling[0m memchr v2.8.0
[1m[92m   Compiling[0m darling_core v0.23.0
[1m[92m   Compiling[0m cryptoxide v0.4.4
[1m[92m   Compiling[0m crossbeam-utils v0.8.21
[1m[92m   Compiling[0m zerocopy-derive v0.8.46
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m darling_macro v0.23.0
[1m[92m   Compiling[0m thiserror-impl v1.0.69
[1m[92m   Compiling[0m darling v0.23.0
[1m[92m   Compiling[0m curve25519-dalek-derive v0.1.1
[1m[92m   Compiling[0m minicbor-derive v0.16.2
[1m[92m   Compiling[0m serde_with_macros v3.18.0
[1m[92m   Compiling[0m ed25519 v2.2.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m serde_with v3.18.0
[1m[92m   Compiling[0m digest v0.9.0
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m unicode-xid v0.2.6
[1m[92m   Compiling[0m derive_more-impl v1.0.0
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m rand_core v0.5.1
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m minicbor-derive v0.15.3
[1m[92m   Compiling[0m pallas-codec v1.0.0-alpha.5
[1m[92m   Compiling[0m rustversion v1.0.22
[1m[92m   Compiling[0m parking_lot_core v0.9.12
[1m[92m   Compiling[0m bytes v1.11.1
[1m[92m   Compiling[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m constant_time_eq v0.4.2
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.5
[1m[92m   Compiling[0m iana-time-zone v0.1.65
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m   Compiling[0m arrayvec v0.7.6
[1m[92m   Compiling[0m arrayref v0.3.9
[1m[92m   Compiling[0m chrono v0.4.44
[1m[92m   Compiling[0m blake2b_simd v1.0.4
[1m[92m   Compiling[0m crc v3.4.0
[1m[92m   Compiling[0m crossbeam-epoch v0.9.18
[1m[92m   Compiling[0m derive_more v1.0.0
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m blake2 v0.10.6
[1m[92m   Compiling[0m block-buffer v0.9.0
[1m[92m   Compiling[0m base58 v0.2.0
[1m[92m   Compiling[0m scopeguard v1.2.0
[1m[92m   Compiling[0m bech32 v0.11.1
[1m[92m   Compiling[0m num-modular v0.6.1
[1m[92m   Compiling[0m rayon-core v1.13.0
[1m[92m   Compiling[0m smallvec v1.15.1
[1m[92m   Compiling[0m pin-project-lite v0.2.17
[1m[92m   Compiling[0m opaque-debug v0.3.1
[1m[92m   Compiling[0m bech32 v0.9.1
[1m[92m   Compiling[0m sha2 v0.9.9
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m num-order v1.2.0
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m lock_api v0.4.14
[1m[92m   Compiling[0m crossbeam-deque v0.8.6
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.5
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m rand_chacha v0.3.1
[1m[92m   Compiling[0m tracing-core v0.1.36
[1m[92m   Compiling[0m tracing-attributes v0.1.31
[1m[92m   Compiling[0m itertools v0.13.0
[1m[92m   Compiling[0m errno v0.3.14
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m clap_lex v1.1.0
[1m[92m   Compiling[0m ciborium-io v0.2.2
[1m[92m   Compiling[0m crc32fast v1.5.0
[1m[92m   Compiling[0m dashu-base v0.4.1
[1m[92m   Compiling[0m plotters-backend v0.3.7
[1m[92m   Compiling[0m anstyle v1.0.14
[1m[92m   Compiling[0m static_assertions v1.1.0
[1m[92m   Compiling[0m dashu-int v0.4.1
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m plotters-svg v0.3.7
[1m[92m   Compiling[0m clap_builder v4.6.0
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m tracing v0.1.44
[1m[92m   Compiling[0m signal-hook-registry v1.4.8
[1m[92m   Compiling[0m rand v0.8.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m parking_lot v0.12.5
[1m[92m   Compiling[0m tokio-macros v2.6.1
[1m[92m   Compiling[0m itertools v0.10.5
[1m[92m   Compiling[0m socket2 v0.6.3
[1m[92m   Compiling[0m mio v1.1.1
[1m[92m   Compiling[0m same-file v1.0.6
[1m[92m   Compiling[0m bitflags v2.11.0
[1m[92m   Compiling[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m cast v0.3.0
[1m[92m   Compiling[0m walkdir v2.5.0
[1m[92m   Compiling[0m criterion-plot v0.5.0
[1m[92m   Compiling[0m tokio v1.50.0
[1m[92m   Compiling[0m rayon v1.11.0
[1m[92m   Compiling[0m torsten-crypto v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-crypto)
[1m[92m   Compiling[0m clap v4.6.0
[1m[92m   Compiling[0m torsten-serialization v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-serialization)
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m is-terminal v0.4.17
[1m[92m   Compiling[0m memmap2 v0.9.10
[1m[92m   Compiling[0m fastrand v2.3.0
[1m[92m   Compiling[0m anes v0.1.6
[1m[92m   Compiling[0m oorandom v11.1.5
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m torsten-storage v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 16s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-135cc081c40f3dc3)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 46.9s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 46.905 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [4.2870 s 4.4826 s 4.8005 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 452.8s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 452.77 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [54.376 ms 54.596 ms 54.819 ms]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) low mild
  1 (1.00%) high mild
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 4646.1s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 4646.1 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [530.52 ms 531.43 ms 532.40 ms]
Found 5 outliers among 100 measurements (5.00%)
  4 (4.00%) high mild
  1 (1.00%) high severe

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 402.2s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 402.16 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [55.099 ms 55.430 ms 55.769 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 384.6s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 384.59 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [55.407 ms 55.880 ms 56.393 ms]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 387.1s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 387.10 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [56.762 ms 57.101 ms 57.443 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.9s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 7.9329 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [8.5134 ms 8.7438 ms 8.9643 ms]

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.4s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 39.444 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.9160 s 3.9472 s 3.9814 s]
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 41.6s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 41.645 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [3.8912 s 3.9213 s 3.9497 s]
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.3s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 39.272 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [54.951 ms 56.456 ms 58.209 ms]
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.1s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 39.066 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [55.139 ms 56.076 ms 56.958 ms]

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 5.9287 s (30k iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [198.54 µs 199.12 µs 199.99 µs]
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.9265 s (30k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [195.89 µs 196.42 µs 196.93 µs]
Found 14 outliers among 100 measurements (14.00%)
  12 (12.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.2963 s (1600 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [3.2913 ms 3.3167 ms 3.3474 ms]
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 5.0901 s (2400 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [1.6074 ms 1.6147 ms 1.6228 ms]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.4s, enable flat sampling, or reduce sample count to 50.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 8.3785 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.6631 ms 1.6836 ms 1.7046 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.8869 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [27.729 ms 27.972 ms 28.243 ms]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0032 s (400 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [12.455 ms 12.483 ms 12.513 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.0006 s (400 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [12.460 ms 12.494 ms 12.531 ms]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) high mild
  3 (3.00%) high severe

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.0286 s (152k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [33.159 µs 33.169 µs 33.180 µs]
Found 3 outliers among 100 measurements (3.00%)
  1 (1.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.0268 s (152k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [33.185 µs 33.318 µs 33.525 µs]
Found 9 outliers among 100 measurements (9.00%)
  1 (1.00%) high mild
  8 (8.00%) high severe

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 5.9676 s (300 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [20.042 ms 20.124 ms 20.210 ms]
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.1767 s (300 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [20.414 ms 20.456 ms 20.501 ms]
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 5.5529 s (15k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [363.64 µs 364.58 µs 365.52 µs]
Found 9 outliers among 100 measurements (9.00%)
  1 (1.00%) low severe
  6 (6.00%) low mild
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 5.6118 s (15k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [366.65 µs 367.49 µs 368.22 µs]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) low severe
  2 (2.00%) low mild
  1 (1.00%) high mild

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 7.9298 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [791.78 µs 792.49 µs 793.19 µs]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) low mild
  2 (2.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.2342 s (800 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.5223 ms 6.5647 ms 6.6123 ms]
Found 7 outliers among 100 measurements (7.00%)
  5 (5.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2892 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.4722 ms 3.4766 ms 3.4814 ms]
Found 6 outliers among 100 measurements (6.00%)
  5 (5.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 9.4969 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [47.574 ms 47.736 ms 47.955 ms]
Found 7 outliers among 100 measurements (7.00%)
  4 (4.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.1264 s (700 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.1867 ms 7.2333 ms 7.2880 ms]
Found 8 outliers among 100 measurements (8.00%)
  3 (3.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.7s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 9.7066 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [95.799 ms 96.051 ms 96.318 ms]
Found 6 outliers among 100 measurements (6.00%)
  6 (6.00%) high mild

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0366 s (323k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [15.397 µs 15.404 µs 15.414 µs]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.0947 s (172k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [30.163 µs 30.173 µs 30.184 µs]
Found 6 outliers among 100 measurements (6.00%)
  1 (1.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0362 s (313k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [15.872 µs 15.884 µs 15.897 µs]
Found 10 outliers among 100 measurements (10.00%)
  6 (6.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0963 s (252k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [20.455 µs 20.464 µs 20.475 µs]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0090 s (308k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [16.054 µs 16.067 µs 16.086 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0767 s (258k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [19.972 µs 19.984 µs 20.001 µs]
Found 6 outliers among 100 measurements (6.00%)
  3 (3.00%) high mild
  3 (3.00%) high severe

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0405 s (434k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.598 µs 11.611 µs 11.629 µs]
Found 7 outliers among 100 measurements (7.00%)
  2 (2.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.0485 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [41.623 µs 41.632 µs 41.642 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0194 s (6435 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [782.25 µs 783.22 µs 783.90 µs]
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.0594 s (770 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.4935 ms 6.6148 ms 6.7830 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.0664 s (1430 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.4480 ms 3.4529 ms 3.4634 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 5.2440 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [47.706 ms 47.941 ms 48.104 ms]
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.1105 s (715 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.1017 ms 7.2273 ms 7.4049 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.3s or enable flat sampling.
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 5.2601 s (55 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [95.868 ms 96.064 ms 96.211 ms]
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 5.9895 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [26.657 ms 26.823 ms 26.932 ms]
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 6.0954 s (30 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [203.27 ms 205.36 ms 207.68 ms]
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.3564 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [66.564 ms 67.304 ms 68.110 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 8.3170 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [413.95 ms 415.95 ms 418.52 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.3s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 8.2721 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [150.03 ms 150.66 ms 151.29 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.5s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 8.5181 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [850.06 ms 856.42 ms 864.18 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0007 s (324k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [15.587 µs 15.594 µs 15.604 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0003 s (179k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [27.040 µs 27.263 µs 27.571 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0008 s (315k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [15.990 µs 15.999 µs 16.015 µs]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0006 s (245k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.180 µs 20.197 µs 20.216 µs]
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0006 s (300k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [16.836 µs 16.839 µs 16.842 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0003 s (252k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [19.647 µs 19.655 µs 19.660 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0009 s (289k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [17.412 µs 17.461 µs 17.591 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0011 s (253k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [19.552 µs 19.562 µs 19.576 µs]
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0001 s (310k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [16.247 µs 16.268 µs 16.283 µs]
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0005 s (257k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.266 µs 19.289 µs 19.317 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0007 s (308k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [16.368 µs 16.384 µs 16.403 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0010 s (258k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.196 µs 19.200 µs 19.206 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 5.0048 s (26k iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [195.19 µs 195.33 µs 195.47 µs]
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0037 s (26k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [195.07 µs 195.20 µs 195.31 µs]
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 5.0056 s (6105 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [816.13 µs 816.89 µs 817.67 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0297 s (6160 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [816.18 µs 819.06 µs 822.69 µs]
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 5.0718 s (2640 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [1.6401 ms 1.6685 ms 1.7054 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0861 s (3080 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [1.6793 ms 1.7402 ms 1.8096 ms]
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 5.3299 s (715 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [5.1549 ms 5.2262 ms 5.3361 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.0636 s (990 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [5.1249 ms 5.1454 ms 5.1706 ms]
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 6.1562 s (220 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [17.285 ms 17.385 ms 17.473 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.8325 s (330 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [17.601 ms 17.626 ms 17.656 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.7s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 39.687 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [3.8677 s 3.9383 s 4.0226 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 199.0s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 199.02 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [19.515 s 19.609 s 19.752 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 392.9s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 392.93 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [39.075 s 39.259 s 39.470 s]
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 985.6s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 985.59 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [97.673 s 98.087 s 98.606 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m zeroize_derive v1.4.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m   Compiling[0m shlex v1.3.0
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
[1m[92m   Compiling[0m heck v0.5.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m tap v1.0.1
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m wyz v0.5.1
[1m[92m   Compiling[0m strum_macros v0.26.4
[1m[92m   Compiling[0m peg-macros v0.8.5
[1m[92m   Compiling[0m threadpool v1.8.1
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-primitives v0.33.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m pallas-addresses v0.33.0
[1m[92m   Compiling[0m miette-derive v5.10.0
[1m[92m   Compiling[0m funty v2.0.0
[1m[92m   Compiling[0m arrayvec v0.5.2
[1m[92m   Compiling[0m hashbrown v0.12.3
[1m[92m   Compiling[0m unicode-segmentation v1.12.0
[1m[92m   Compiling[0m unicode-width v0.1.14
[1m[92m   Compiling[0m typed-arena v2.0.2
[1m[92m   Compiling[0m miette v5.10.0
[1m[92m   Compiling[0m pallas-traverse v0.33.0
[1m[92m   Compiling[0m pretty v0.11.3
[1m[92m   Compiling[0m peg v0.8.5
[1m[92m   Compiling[0m bitvec v1.0.1
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m secp256k1 v0.26.0
[1m[92m   Compiling[0m strum v0.26.3
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m fs2 v0.4.3
[1m[92m   Compiling[0m wait-timeout v0.2.1
[1m[92m   Compiling[0m fnv v1.0.7
[1m[92m   Compiling[0m quick-error v1.2.3
[1m[92m   Compiling[0m hamming v0.1.3
[1m[92m   Compiling[0m bit-vec v0.8.0
[1m[92m   Compiling[0m bit-set v0.8.0
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m uplc v1.1.21 (https://github.com/aiken-lang/aiken.git?rev=6806d52211fbb469ae13aa0e0290aeb6b9b3e8cf#6806d522)
[1m[92m   Compiling[0m rusty-fork v0.3.1
[1m[92m   Compiling[0m torsten-lsm v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-lsm)
[1m[92m   Compiling[0m torsten-crypto v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-crypto)
[1m[92m   Compiling[0m torsten-serialization v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-serialization)
[1m[92m   Compiling[0m rand_xorshift v0.4.0
[1m[92m   Compiling[0m bincode v1.3.3
[1m[92m   Compiling[0m unarray v0.1.4
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m torsten-ledger v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 04s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-a96bf51f1a9f04fb)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 30.6s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 30.617 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.9243 s 2.9397 s 2.9556 s]

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.6769 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [563.51 µs 564.32 µs 565.35 µs]
Found 9 outliers among 100 measurements (9.00%)
  2 (2.00%) low mild
  1 (1.00%) high mild
  6 (6.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 5.0792 s (15k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [331.15 µs 331.69 µs 332.36 µs]
Found 9 outliers among 100 measurements (9.00%)
  3 (3.00%) high mild
  6 (6.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 6.3248 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [413.17 µs 414.06 µs 415.18 µs]
Found 6 outliers among 100 measurements (6.00%)
  5 (5.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 6.2439 s (20k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [300.96 µs 301.38 µs 301.88 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 55.0s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 54.998 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.9101 s 2.9210 s 2.9333 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.8s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 28.805 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [270.17 ms 274.71 ms 280.20 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.6s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 28.590 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [260.30 ms 270.52 ms 278.64 ms]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low severe
  1 (10.00%) high mild

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 38.8s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 38.791 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.7032 s 3.7191 s 3.7358 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.3s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 37.252 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [140.55 ms 147.71 ms 155.11 ms]

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 29.9s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 29.885 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [293.28 ms 294.07 ms 294.82 ms]
Found 11 outliers among 100 measurements (11.00%)
  1 (1.00%) low severe
  7 (7.00%) high mild
  3 (3.00%) high severe

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 40.7s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 40.708 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [618.72 ms 620.68 ms 623.19 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.6s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 28.572 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.8552 s 2.8690 s 2.8835 s]
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.6s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 28.556 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.8604 s 2.8731 s 2.8879 s]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.5s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 28.534 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.8463 s 2.8593 s 2.8709 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.5s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 28.450 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.8489 s 2.8644 s 2.8806 s]
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.5s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 28.452 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.8319 s 2.8467 s 2.8611 s]

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 6.9375 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [455.37 µs 455.78 µs 456.24 µs]
Found 8 outliers among 100 measurements (8.00%)
  6 (6.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 6.9041 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [454.88 µs 455.55 µs 456.36 µs]
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 6.9381 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [456.39 µs 456.63 µs 456.90 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 6.9711 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [460.45 µs 460.96 µs 461.56 µs]
Found 12 outliers among 100 measurements (12.00%)
  1 (1.00%) low mild
  4 (4.00%) high mild
  7 (7.00%) high severe
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 6.9760 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [457.81 µs 458.30 µs 458.90 µs]
Found 7 outliers among 100 measurements (7.00%)
  2 (2.00%) high mild
  5 (5.00%) high severe

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 7.2767 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [240.58 ms 241.73 ms 242.90 ms]
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 13.5s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 13.509 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.3363 s 1.3429 s 1.3489 s]
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.7s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 28.749 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.8706 s 2.8798 s 2.8897 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0029 s (13k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [371.34 µs 372.01 µs 372.80 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0169 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [423.00 µs 423.87 µs 425.15 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0147 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [450.09 µs 450.70 µs 451.71 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 7.3545 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [16.111 ms 17.952 ms 20.022 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 13.5s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 13.451 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [113.27 ms 120.37 ms 126.71 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 28.4s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 28.420 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [259.10 ms 261.82 ms 264.48 ms]

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.5231 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [29.589 ms 29.777 ms 29.952 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.8s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 7.8351 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [141.32 ms 141.67 ms 142.03 ms]
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 6.2389 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [306.25 ms 310.09 ms 312.90 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 173.3s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 173.31 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [17.356 s 17.390 s 17.432 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 385.0s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 385.03 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [36.620 s 36.728 s 36.853 s]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
  1 (10.00%) high severe

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0153 s (3850 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.3045 ms 1.3111 ms 1.3212 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0832 s (3080 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.6483 ms 1.7710 ms 1.9389 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 15.3s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 15.316 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.5539 s 1.5583 s 1.5631 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 31.9s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 31.931 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.1062 s 3.1313 s 3.1619 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

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
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 29.24s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-a6e2ac7d854bf680)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.61s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
