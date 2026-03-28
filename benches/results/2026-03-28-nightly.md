# Nightly Benchmark Results — 2026-03-28

Machine: GitHub Actions ubuntu-latest
Branch: main (4abf9fa)

## Storage Benchmarks
```
[1m[92m    Updating[0m crates.io index
[1m[92m    Updating[0m git repository `https://github.com/iquerejeta/curve25519-dalek`
[1m[92m    Updating[0m git repository `https://github.com/input-output-hk/vrf`
[1m[92m    Updating[0m git repository `https://github.com/aiken-lang/aiken.git`
[1m[92m Downloading[0m crates ...
[1m[92m  Downloaded[0m ciborium-ll v0.2.2
[1m[92m  Downloaded[0m arrayvec v0.7.6
[1m[92m  Downloaded[0m minicbor-derive v0.16.2
[1m[92m  Downloaded[0m autocfg v1.5.0
[1m[92m  Downloaded[0m ref-cast v1.0.25
[1m[92m  Downloaded[0m pin-project-lite v0.2.17
[1m[92m  Downloaded[0m parking_lot v0.12.5
[1m[92m  Downloaded[0m base64ct v1.8.3
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m pallas-primitives v1.0.0-alpha.5
[1m[92m  Downloaded[0m opaque-debug v0.3.1
[1m[92m  Downloaded[0m pkcs8 v0.10.2
[1m[92m  Downloaded[0m num-order v1.2.0
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m crossbeam-epoch v0.9.18
[1m[92m  Downloaded[0m minicbor-derive v0.15.3
[1m[92m  Downloaded[0m version_check v0.9.5
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m crc v3.4.0
[1m[92m  Downloaded[0m signature v2.2.0
[1m[92m  Downloaded[0m ciborium v0.2.2
[1m[92m  Downloaded[0m zmij v1.0.21
[1m[92m  Downloaded[0m pallas-addresses v1.0.0-alpha.5
[1m[92m  Downloaded[0m itoa v1.0.17
[1m[92m  Downloaded[0m is-terminal v0.4.17
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m bitflags v2.11.0
[1m[92m  Downloaded[0m spki v0.7.3
[1m[92m  Downloaded[0m num-conv v0.2.0
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m half v2.7.1
[1m[92m  Downloaded[0m pallas-codec v1.0.0-alpha.5
[1m[92m  Downloaded[0m time-core v0.1.8
[1m[92m  Downloaded[0m getrandom v0.4.2
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m num-bigint v0.4.6
[1m[92m  Downloaded[0m rustc_version v0.4.1
[1m[92m  Downloaded[0m hashbrown v0.16.1
[1m[92m  Downloaded[0m unicode-xid v0.2.6
[1m[92m  Downloaded[0m cryptoxide v0.4.4
[1m[92m  Downloaded[0m walkdir v2.5.0
[1m[92m  Downloaded[0m tinytemplate v1.2.1
[1m[92m  Downloaded[0m chrono v0.4.44
[1m[92m  Downloaded[0m zeroize v1.8.2
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m tokio-macros v2.6.1
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m tempfile v3.27.0
[1m[92m  Downloaded[0m semver v1.0.27
[1m[92m  Downloaded[0m tracing-core v0.1.36
[1m[92m  Downloaded[0m serde_with_macros v3.18.0
[1m[92m  Downloaded[0m curve25519-dalek v4.1.3
[1m[92m  Downloaded[0m unicode-ident v1.0.24
[1m[92m  Downloaded[0m tracing-attributes v0.1.31
[1m[92m  Downloaded[0m typenum v1.19.0
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m zerocopy-derive v0.8.46
[1m[92m  Downloaded[0m plotters v0.3.7
[1m[92m  Downloaded[0m schemars v1.2.1
[1m[92m  Downloaded[0m rand v0.9.2
[1m[92m  Downloaded[0m smallvec v1.15.1
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m regex v1.12.3
[1m[92m  Downloaded[0m zerocopy v0.8.46
[1m[92m  Downloaded[0m serde_with v3.18.0
[1m[92m  Downloaded[0m time v0.3.47
[1m[92m  Downloaded[0m rayon v1.11.0
[1m[92m  Downloaded[0m schemars v0.9.0
[1m[92m  Downloaded[0m syn v2.0.117
[1m[92m  Downloaded[0m curve25519-dalek v3.2.0
[1m[92m  Downloaded[0m serde v1.0.228
[1m[92m  Downloaded[0m rayon-core v1.13.0
[1m[92m  Downloaded[0m time-macros v0.2.27
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m rustix v1.1.4
[1m[92m  Downloaded[0m serde_derive v1.0.228
[1m[92m  Downloaded[0m libc v0.2.183
[1m[92m  Downloaded[0m tracing v0.1.44
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m static_assertions v1.1.0
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m thiserror-impl v2.0.18
[1m[92m  Downloaded[0m sha2 v0.10.9
[1m[92m  Downloaded[0m sha2 v0.9.9
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m rand_core v0.5.1
[1m[92m  Downloaded[0m clap_builder v4.6.0
[1m[92m  Downloaded[0m strsim v0.11.1
[1m[92m  Downloaded[0m scopeguard v1.2.0
[1m[92m  Downloaded[0m powerfmt v0.2.0
[1m[92m  Downloaded[0m plotters-svg v0.3.7
[1m[92m  Downloaded[0m itertools v0.13.0
[1m[92m  Downloaded[0m dashu-int v0.4.1
[1m[92m  Downloaded[0m rand v0.8.5
[1m[92m  Downloaded[0m itertools v0.10.5
[1m[92m  Downloaded[0m indexmap v2.13.0
[1m[92m  Downloaded[0m ed25519-dalek v2.2.0
[1m[92m  Downloaded[0m tokio v1.50.0
[1m[92m  Downloaded[0m der v0.7.10
[1m[92m  Downloaded[0m darling_core v0.23.0
[1m[92m  Downloaded[0m same-file v1.0.6
[1m[92m  Downloaded[0m rustversion v1.0.22
[1m[92m  Downloaded[0m ref-cast-impl v1.0.25
[1m[92m  Downloaded[0m bech32 v0.11.1
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m darling v0.23.0
[1m[92m  Downloaded[0m const-oid v0.9.6
[1m[92m  Downloaded[0m blake2 v0.10.6
[1m[92m  Downloaded[0m num-modular v0.6.1
[1m[92m  Downloaded[0m mio v1.1.1
[1m[92m  Downloaded[0m minicbor v0.25.1
[1m[92m  Downloaded[0m memmap2 v0.9.10
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m derive_more-impl v1.0.0
[1m[92m  Downloaded[0m derive_more v1.0.0
[1m[92m  Downloaded[0m dashu-base v0.4.1
[1m[92m  Downloaded[0m criterion-plot v0.5.0
[1m[92m  Downloaded[0m cast v0.3.0
[1m[92m  Downloaded[0m byteorder v1.5.0
[1m[92m  Downloaded[0m oorandom v11.1.5
[1m[92m  Downloaded[0m num-traits v0.2.19
[1m[92m  Downloaded[0m num-rational v0.4.2
[1m[92m  Downloaded[0m num-integer v0.1.46
[1m[92m  Downloaded[0m minicbor v0.26.5
[1m[92m  Downloaded[0m iana-time-zone v0.1.65
[1m[92m  Downloaded[0m either v1.15.0
[1m[92m  Downloaded[0m criterion v0.5.1
[1m[92m  Downloaded[0m cpufeatures v0.2.17
[1m[92m  Downloaded[0m fastrand v2.3.0
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m digest v0.9.0
[1m[92m  Downloaded[0m darling_macro v0.23.0
[1m[92m  Downloaded[0m crc-catalog v2.4.0
[1m[92m  Downloaded[0m bytes v1.11.1
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m rand_chacha v0.3.1
[1m[92m  Downloaded[0m plotters-backend v0.3.7
[1m[92m  Downloaded[0m curve25519-dalek-derive v0.1.1
[1m[92m  Downloaded[0m crypto-common v0.1.6
[1m[92m  Downloaded[0m cfg-if v1.0.4
[1m[92m  Downloaded[0m clap_lex v1.1.0
[1m[92m  Downloaded[0m ed25519 v2.2.3
[1m[92m  Downloaded[0m crc32fast v1.5.0
[1m[92m  Downloaded[0m subtle v2.6.1
[1m[92m  Downloaded[0m indexmap v1.9.3
[1m[92m  Downloaded[0m bech32 v0.9.1
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m lock_api v0.4.14
[1m[92m  Downloaded[0m generic-array v0.14.9
[1m[92m  Downloaded[0m deranged v0.5.8
[1m[92m  Downloaded[0m crossbeam-utils v0.8.21
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m pallas-traverse v1.0.0-alpha.5
[1m[92m  Downloaded[0m ident_case v1.0.1
[1m[92m  Downloaded[0m hashbrown v0.12.3
[1m[92m  Downloaded[0m getrandom v0.1.16
[1m[92m  Downloaded[0m ciborium-io v0.2.2
[1m[92m  Downloaded[0m pallas-crypto v1.0.0-alpha.5
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m block-buffer v0.9.0
[1m[92m  Downloaded[0m blake2b_simd v1.0.4
[1m[92m  Downloaded[0m dyn-clone v1.0.20
[1m[92m  Downloaded[0m crossbeam-deque v0.8.6
[1m[92m  Downloaded[0m clap v4.6.0
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m parking_lot_core v0.9.12
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m constant_time_eq v0.4.2
[1m[92m  Downloaded[0m base58 v0.2.0
[1m[92m  Downloaded[0m anes v0.1.6
[1m[92m  Downloaded[0m arrayref v0.3.9
[1m[92m  Downloaded[0m anstyle v1.0.14
[1m[92m  Downloaded[0m aho-corasick v1.1.4
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m proc-macro2 v1.0.106
[1m[92m   Compiling[0m unicode-ident v1.0.24
[1m[92m   Compiling[0m quote v1.0.45
[1m[92m   Compiling[0m cfg-if v1.0.4
[1m[92m   Compiling[0m libc v0.2.183
[1m[92m   Compiling[0m version_check v0.9.5
[1m[92m   Compiling[0m typenum v1.19.0
[1m[92m   Compiling[0m generic-array v0.14.9
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m zerocopy v0.8.46
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m subtle v2.6.1
[1m[92m   Compiling[0m autocfg v1.5.0
[1m[92m   Compiling[0m crypto-common v0.1.6
[1m[92m   Compiling[0m block-buffer v0.10.4
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m digest v0.10.7
[1m[92m   Compiling[0m semver v1.0.27
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m ident_case v1.0.1
[1m[92m   Compiling[0m strsim v0.11.1
[1m[92m   Compiling[0m cpufeatures v0.2.17
[1m[92m   Compiling[0m getrandom v0.3.4
[1m[92m   Compiling[0m rustc_version v0.4.1
[1m[92m   Compiling[0m getrandom v0.2.17
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m rand_core v0.6.4
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m   Compiling[0m minicbor v0.26.5
[1m[92m   Compiling[0m rand_core v0.9.5
[1m[92m   Compiling[0m hex v0.4.3
[1m[92m   Compiling[0m signature v2.2.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m sha2 v0.10.9
[1m[92m   Compiling[0m itoa v1.0.17
[1m[92m   Compiling[0m darling_core v0.23.0
[1m[92m   Compiling[0m either v1.15.0
[1m[92m   Compiling[0m memchr v2.8.0
[1m[92m   Compiling[0m getrandom v0.1.16
[1m[92m   Compiling[0m crossbeam-utils v0.8.21
[1m[92m   Compiling[0m cryptoxide v0.4.4
[1m[92m   Compiling[0m zerocopy-derive v0.8.46
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m darling_macro v0.23.0
[1m[92m   Compiling[0m thiserror-impl v1.0.69
[1m[92m   Compiling[0m darling v0.23.0
[1m[92m   Compiling[0m minicbor-derive v0.16.2
[1m[92m   Compiling[0m curve25519-dalek-derive v0.1.1
[1m[92m   Compiling[0m serde_with_macros v3.18.0
[1m[92m   Compiling[0m ed25519 v2.2.3
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m digest v0.9.0
[1m[92m   Compiling[0m serde_with v3.18.0
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m unicode-xid v0.2.6
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m derive_more-impl v1.0.0
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m rand_core v0.5.1
[1m[92m   Compiling[0m minicbor-derive v0.15.3
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m bytes v1.11.1
[1m[92m   Compiling[0m pallas-codec v1.0.0-alpha.5
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m arrayvec v0.7.6
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.5
[1m[92m   Compiling[0m rustversion v1.0.22
[1m[92m   Compiling[0m parking_lot_core v0.9.12
[1m[92m   Compiling[0m constant_time_eq v0.4.2
[1m[92m   Compiling[0m iana-time-zone v0.1.65
[1m[92m   Compiling[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m arrayref v0.3.9
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m   Compiling[0m blake2b_simd v1.0.4
[1m[92m   Compiling[0m crc v3.4.0
[1m[92m   Compiling[0m chrono v0.4.44
[1m[92m   Compiling[0m crossbeam-epoch v0.9.18
[1m[92m   Compiling[0m derive_more v1.0.0
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m blake2 v0.10.6
[1m[92m   Compiling[0m block-buffer v0.9.0
[1m[92m   Compiling[0m smallvec v1.15.1
[1m[92m   Compiling[0m bech32 v0.11.1
[1m[92m   Compiling[0m scopeguard v1.2.0
[1m[92m   Compiling[0m opaque-debug v0.3.1
[1m[92m   Compiling[0m bech32 v0.9.1
[1m[92m   Compiling[0m pin-project-lite v0.2.17
[1m[92m   Compiling[0m base58 v0.2.0
[1m[92m   Compiling[0m rayon-core v1.13.0
[1m[92m   Compiling[0m num-modular v0.6.1
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m num-order v1.2.0
[1m[92m   Compiling[0m sha2 v0.9.9
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
[1m[92m   Compiling[0m dashu-base v0.4.1
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m   Compiling[0m crc32fast v1.5.0
[1m[92m   Compiling[0m static_assertions v1.1.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m plotters-backend v0.3.7
[1m[92m   Compiling[0m anstyle v1.0.14
[1m[92m   Compiling[0m clap_lex v1.1.0
[1m[92m   Compiling[0m ciborium-io v0.2.2
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m clap_builder v4.6.0
[1m[92m   Compiling[0m plotters-svg v0.3.7
[1m[92m   Compiling[0m dashu-int v0.4.1
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m tracing v0.1.44
[1m[92m   Compiling[0m signal-hook-registry v1.4.8
[1m[92m   Compiling[0m rand v0.8.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m parking_lot v0.12.5
[1m[92m   Compiling[0m tokio-macros v2.6.1
[1m[92m   Compiling[0m itertools v0.10.5
[1m[92m   Compiling[0m mio v1.1.1
[1m[92m   Compiling[0m socket2 v0.6.3
[1m[92m   Compiling[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m same-file v1.0.6
[1m[92m   Compiling[0m cast v0.3.0
[1m[92m   Compiling[0m bitflags v2.11.0
[1m[92m   Compiling[0m walkdir v2.5.0
[1m[92m   Compiling[0m criterion-plot v0.5.0
[1m[92m   Compiling[0m tokio v1.50.0
[1m[92m   Compiling[0m rayon v1.11.0
[1m[92m   Compiling[0m torsten-crypto v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-crypto)
[1m[92m   Compiling[0m torsten-serialization v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-serialization)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m clap v4.6.0
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m memmap2 v0.9.10
[1m[92m   Compiling[0m is-terminal v0.4.17
[1m[92m   Compiling[0m anes v0.1.6
[1m[92m   Compiling[0m fastrand v2.3.0
[1m[92m   Compiling[0m oorandom v11.1.5
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m torsten-storage v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 09s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-51833895afe17d60)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 47.5s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 47.499 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [4.2145 s 4.3576 s 4.5614 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 413.8s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 413.77 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [30.006 ms 30.134 ms 30.264 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 4492.3s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 4492.3 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [355.07 ms 359.89 ms 364.88 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 389.4s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 389.40 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [30.258 ms 30.860 ms 31.946 ms]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 375.4s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 375.41 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [29.982 ms 30.128 ms 30.276 ms]
Found 6 outliers among 100 measurements (6.00%)
  1 (1.00%) low mild
  5 (5.00%) high mild

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 372.0s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 371.95 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [30.436 ms 30.544 ms 30.652 ms]

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.0s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 8.0407 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [6.5296 ms 6.7054 ms 6.8782 ms]

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.3s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 37.334 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.7140 s 3.7858 s 3.8944 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.0s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 36.958 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [3.7236 s 3.7554 s 3.7886 s]
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.1s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 37.096 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [30.050 ms 30.302 ms 30.547 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.5s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 37.514 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [29.326 ms 29.698 ms 30.062 ms]

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 5.9908 s (300 iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [19.698 ms 19.784 ms 19.869 ms]
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.7628 s (30k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [190.00 µs 190.14 µs 190.28 µs]
Found 7 outliers among 100 measurements (7.00%)
  1 (1.00%) low mild
  4 (4.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.1665 s (1700 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [3.0416 ms 3.0559 ms 3.0709 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 115.0s, or reduce sample count to 10.
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 114.95 s (100 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [863.38 ms 864.37 ms 865.65 ms]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.0s, enable flat sampling, or reduce sample count to 50.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 9.0071 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.8147 ms 1.8293 ms 1.8439 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.1661 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [25.613 ms 25.713 ms 25.821 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.4966 s (500 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [11.049 ms 11.093 ms 11.139 ms]
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.6263 s (500 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [11.353 ms 11.397 ms 11.442 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.1257 s (146k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [34.981 µs 34.994 µs 35.010 µs]
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.1289 s (146k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [35.134 µs 35.217 µs 35.294 µs]
Found 16 outliers among 100 measurements (16.00%)
  2 (2.00%) high mild
  14 (14.00%) high severe

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 6.0244 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [14.965 ms 15.003 ms 15.043 ms]
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.3022 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [15.687 ms 15.753 ms 15.821 ms]

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 6.4264 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [319.08 µs 319.53 µs 319.96 µs]
Found 10 outliers among 100 measurements (10.00%)
  5 (5.00%) low severe
  3 (3.00%) low mild
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 6.4190 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [317.85 µs 318.45 µs 319.07 µs]
Found 8 outliers among 100 measurements (8.00%)
  3 (3.00%) low severe
  4 (4.00%) low mild
  1 (1.00%) high mild

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 8.0862 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [807.24 µs 808.06 µs 808.87 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.5615 s (900 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.1107 ms 6.1440 ms 6.1827 ms]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2683 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.5059 ms 3.5091 ms 3.5125 ms]
Found 9 outliers among 100 measurements (9.00%)
  5 (5.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 8.9360 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [44.799 ms 45.039 ms 45.403 ms]
Found 7 outliers among 100 measurements (7.00%)
  5 (5.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.2948 s (700 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.4467 ms 7.5136 ms 7.5852 ms]
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.9s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 8.8801 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [86.961 ms 87.241 ms 87.531 ms]

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0526 s (328k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [15.332 µs 15.338 µs 15.346 µs]
Found 6 outliers among 100 measurements (6.00%)
  3 (3.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.0258 s (167k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [30.158 µs 30.198 µs 30.251 µs]
Found 6 outliers among 100 measurements (6.00%)
  1 (1.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0445 s (318k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [15.813 µs 15.821 µs 15.830 µs]
Found 9 outliers among 100 measurements (9.00%)
  5 (5.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0751 s (247k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [20.557 µs 20.564 µs 20.572 µs]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0636 s (318k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [15.886 µs 15.893 µs 15.900 µs]
Found 8 outliers among 100 measurements (8.00%)
  5 (5.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0399 s (252k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [20.006 µs 20.026 µs 20.058 µs]
Found 8 outliers among 100 measurements (8.00%)
  4 (4.00%) high mild
  4 (4.00%) high severe

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0236 s (449k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.169 µs 11.175 µs 11.182 µs]
Found 10 outliers among 100 measurements (10.00%)
  3 (3.00%) high mild
  7 (7.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.0907 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [41.964 µs 41.976 µs 41.988 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0158 s (6270 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [801.19 µs 801.81 µs 802.33 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.1025 s (825 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.2108 ms 6.3110 ms 6.4469 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.0564 s (1375 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.5819 ms 3.5987 ms 3.6362 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 7.4278 s (165 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [44.577 ms 44.854 ms 45.293 ms]
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.2311 s (660 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.3413 ms 7.4209 ms 7.4856 ms]
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 9.5451 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [86.508 ms 87.167 ms 87.758 ms]
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 6.3140 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [27.674 ms 28.271 ms 28.690 ms]
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 5.4743 s (30 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [180.01 ms 181.23 ms 182.40 ms]
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.3242 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [65.963 ms 66.680 ms 67.872 ms]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low mild
  2 (20.00%) high severe
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 6.2173 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [309.74 ms 310.87 ms 312.29 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.0s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 8.0267 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [144.33 ms 145.58 ms 147.08 ms]
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.5s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 5.4872 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [556.25 ms 569.61 ms 583.81 ms]

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0000 s (235k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [15.387 µs 15.394 µs 15.402 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0014 s (174k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [27.702 µs 27.708 µs 27.716 µs]
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0008 s (230k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [15.776 µs 15.785 µs 15.799 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0001 s (248k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.453 µs 20.458 µs 20.463 µs]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) low mild
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0006 s (227k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [15.989 µs 16.061 µs 16.180 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0010 s (254k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [19.942 µs 19.947 µs 19.951 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0008 s (227k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [15.983 µs 15.993 µs 16.006 µs]
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0001 s (256k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [19.844 µs 19.850 µs 19.855 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0000 s (227k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [16.139 µs 16.158 µs 16.175 µs]
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0003 s (259k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.559 µs 19.562 µs 19.565 µs]
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0011 s (225k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [16.173 µs 16.182 µs 16.196 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0010 s (260k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.484 µs 19.489 µs 19.500 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 5.3179 s (275 iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [19.487 ms 19.801 ms 19.992 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0081 s (26k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [190.81 µs 191.05 µs 191.31 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 9.9145 s (110 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [88.193 ms 89.067 ms 89.812 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0150 s (6655 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [745.98 µs 748.01 µs 750.92 µs]
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.1s.
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 11.055 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [864.79 ms 865.77 ms 866.89 ms]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low mild
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0405 s (2915 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [1.7314 ms 1.7665 ms 1.7976 ms]
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 36.0s.
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 35.999 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [2.2649 s 2.2719 s 2.2807 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.0469 s (990 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [5.0930 ms 5.1151 ms 5.1336 ms]
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 75.2s.
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 75.240 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [22.780 s 23.257 s 23.865 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.2933 s (550 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [9.4495 ms 9.5572 ms 9.6758 ms]

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.5s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 39.492 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [3.7171 s 3.7554 s 3.8015 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 195.1s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 195.09 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [19.012 s 19.192 s 19.462 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 391.3s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 391.26 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [38.836 s 38.962 s 39.080 s]
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 979.4s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 979.39 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [96.122 s 96.474 s 96.773 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) low mild

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m zeroize_derive v1.4.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m shlex v1.3.0
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m cc v1.2.57
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m pallas-codec v0.33.0
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.5
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m pallas-crypto v0.33.0
[1m[92m   Compiling[0m secp256k1-sys v0.8.2
[1m[92m   Compiling[0m radium v0.7.0
[1m[92m   Compiling[0m log v0.4.29
[1m[92m   Compiling[0m blst v0.3.16
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m indexmap v1.9.3
[1m[92m   Compiling[0m num_cpus v1.17.0
[1m[92m   Compiling[0m peg-runtime v0.8.5
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m tap v1.0.1
[1m[92m   Compiling[0m heck v0.5.0
[1m[92m   Compiling[0m wyz v0.5.1
[1m[92m   Compiling[0m strum_macros v0.26.4
[1m[92m   Compiling[0m torsten-primitives v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-primitives)
[1m[92m   Compiling[0m peg-macros v0.8.5
[1m[92m   Compiling[0m threadpool v1.8.1
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.5
[1m[92m   Compiling[0m pallas-primitives v0.33.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m pallas-addresses v0.33.0
[1m[92m   Compiling[0m miette-derive v5.10.0
[1m[92m   Compiling[0m arrayvec v0.5.2
[1m[92m   Compiling[0m funty v2.0.0
[1m[92m   Compiling[0m typed-arena v2.0.2
[1m[92m   Compiling[0m unicode-width v0.1.14
[1m[92m   Compiling[0m hashbrown v0.12.3
[1m[92m   Compiling[0m unicode-segmentation v1.12.0
[1m[92m   Compiling[0m miette v5.10.0
[1m[92m   Compiling[0m pretty v0.11.3
[1m[92m   Compiling[0m peg v0.8.5
[1m[92m   Compiling[0m bitvec v1.0.1
[1m[92m   Compiling[0m pallas-traverse v0.33.0
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.5
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m secp256k1 v0.26.0
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m strum v0.26.3
[1m[92m   Compiling[0m fs2 v0.4.3
[1m[92m   Compiling[0m wait-timeout v0.2.1
[1m[92m   Compiling[0m bit-vec v0.8.0
[1m[92m   Compiling[0m hamming v0.1.3
[1m[92m   Compiling[0m fnv v1.0.7
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
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m torsten-ledger v0.4.6-alpha (/home/runner/work/torsten/torsten/crates/torsten-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 07s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-4b37a42019cfa4eb)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.1s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 26.096 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.5569 s 2.5761 s 2.5936 s]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low severe
  1 (10.00%) low mild
  1 (10.00%) high mild

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.9022 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [566.03 µs 567.41 µs 569.03 µs]
Found 5 outliers among 100 measurements (5.00%)
  3 (3.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 5.2066 s (15k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [342.42 µs 343.10 µs 343.85 µs]
Found 6 outliers among 100 measurements (6.00%)
  4 (4.00%) high mild
  2 (2.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 6.0854 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [397.44 µs 398.25 µs 399.10 µs]
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 6.4368 s (20k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [306.20 µs 308.99 µs 312.08 µs]
Found 17 outliers among 100 measurements (17.00%)
  16 (16.00%) low severe
  1 (1.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 50.1s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 50.120 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.6003 s 2.6142 s 2.6277 s]

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.7s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 25.712 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [256.39 ms 260.23 ms 264.30 ms]
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.6s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 25.575 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [259.34 ms 264.80 ms 270.61 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 33.9s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 33.924 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.3773 s 3.3913 s 3.4065 s]
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.0s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 33.997 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [114.94 ms 124.34 ms 134.11 ms]

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 27.7s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 27.692 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [275.20 ms 275.87 ms 276.46 ms]
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) low severe
  3 (3.00%) high mild

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.8s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 35.828 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [568.59 ms 570.87 ms 573.20 ms]

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 25.189 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.5189 s 2.5304 s 2.5429 s]
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 25.209 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.5138 s 2.5229 s 2.5321 s]
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.6s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 25.585 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.5396 s 2.5478 s 2.5546 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.6s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 25.580 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.5671 s 2.5795 s 2.5928 s]
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.6s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 25.649 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.5492 s 2.5574 s 2.5654 s]

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 7.1178 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [467.77 µs 469.64 µs 472.68 µs]
Found 8 outliers among 100 measurements (8.00%)
  4 (4.00%) high mild
  4 (4.00%) high severe
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 7.1064 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [469.26 µs 469.85 µs 470.50 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 7.1805 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [475.25 µs 475.60 µs 475.95 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 7.1343 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [472.32 µs 472.81 µs 473.31 µs]
Found 6 outliers among 100 measurements (6.00%)
  4 (4.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 7.1775 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [474.01 µs 474.80 µs 475.63 µs]
Found 5 outliers among 100 measurements (5.00%)
  5 (5.00%) high mild

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 6.3329 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [210.98 ms 212.92 ms 215.18 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 12.0s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 11.985 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.1919 s 1.1999 s 1.2079 s]
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.3s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 25.324 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.5561 s 2.5667 s 2.5783 s]

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0188 s (14k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [387.67 µs 389.05 µs 390.32 µs]
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0147 s (12k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [439.15 µs 439.71 µs 440.55 µs]
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0159 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [466.52 µs 467.14 µs 468.09 µs]

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 6.4262 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [19.191 ms 19.950 ms 20.666 ms]
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 12.1s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 12.081 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [127.50 ms 130.93 ms 133.63 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.8s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 25.826 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [259.82 ms 262.66 ms 265.38 ms]

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.1423 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [27.493 ms 27.590 ms 27.772 ms]
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.3s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 7.2634 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [130.85 ms 131.21 ms 131.52 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 5.4424 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [272.06 ms 274.77 ms 276.79 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 154.8s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 154.76 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [15.363 s 15.428 s 15.497 s]
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 338.9s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 338.89 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [32.636 s 32.758 s 32.887 s]

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0639 s (3850 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.2801 ms 1.3041 ms 1.3244 ms]
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0487 s (3080 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.5865 ms 1.5879 ms 1.5896 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 15.8s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 15.820 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.6184 s 1.6273 s 1.6365 s]
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 31.5s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 31.468 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.2740 s 3.3036 s 3.3393 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

```

## LSM Stress Tests
```
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m serde_core v1.0.228
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
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 28.75s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-e9460fbb829b3490)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.39s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
