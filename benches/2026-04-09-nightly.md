# Nightly Benchmark Results — 2026-04-09

Machine: GitHub Actions ubuntu-latest
Branch: main (efdf6bd)

## Storage Benchmarks
```
[1m[92m    Updating[0m crates.io index
[1m[92m    Updating[0m git repository `https://github.com/iquerejeta/curve25519-dalek`
[1m[92m    Updating[0m git repository `https://github.com/input-output-hk/vrf`
[1m[92m    Updating[0m git repository `https://github.com/aiken-lang/aiken.git`
[1m[92m Downloading[0m crates ...
[1m[92m  Downloaded[0m deranged v0.5.8
[1m[92m  Downloaded[0m crossbeam-deque v0.8.6
[1m[92m  Downloaded[0m blake2 v0.10.6
[1m[92m  Downloaded[0m num-modular v0.6.1
[1m[92m  Downloaded[0m const-oid v0.9.6
[1m[92m  Downloaded[0m num-rational v0.4.2
[1m[92m  Downloaded[0m hashbrown v0.12.3
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m page_size v0.6.0
[1m[92m  Downloaded[0m semver v1.0.28
[1m[92m  Downloaded[0m minicbor v0.26.5
[1m[92m  Downloaded[0m sha2 v0.10.9
[1m[92m  Downloaded[0m serde_with v3.18.0
[1m[92m  Downloaded[0m schemars v0.9.0
[1m[92m  Downloaded[0m ref-cast-impl v1.0.25
[1m[92m  Downloaded[0m generic-array v0.14.9
[1m[92m  Downloaded[0m rustversion v1.0.22
[1m[92m  Downloaded[0m strsim v0.11.1
[1m[92m  Downloaded[0m scopeguard v1.2.0
[1m[92m  Downloaded[0m same-file v1.0.6
[1m[92m  Downloaded[0m subtle v2.6.1
[1m[92m  Downloaded[0m time-core v0.1.8
[1m[92m  Downloaded[0m signature v2.2.0
[1m[92m  Downloaded[0m tokio-macros v2.7.0
[1m[92m  Downloaded[0m num-bigint v0.4.6
[1m[92m  Downloaded[0m rand_core v0.5.1
[1m[92m  Downloaded[0m unicode-xid v0.2.6
[1m[92m  Downloaded[0m serde_with_macros v3.18.0
[1m[92m  Downloaded[0m static_assertions v1.1.0
[1m[92m  Downloaded[0m walkdir v2.5.0
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m zeroize v1.8.2
[1m[92m  Downloaded[0m shlex v1.3.0
[1m[92m  Downloaded[0m time-macros v0.2.27
[1m[92m  Downloaded[0m serde_derive v1.0.228
[1m[92m  Downloaded[0m zmij v1.0.21
[1m[92m  Downloaded[0m smallvec v1.15.1
[1m[92m  Downloaded[0m tracing-attributes v0.1.31
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m pallas-primitives v1.0.0-alpha.6
[1m[92m  Downloaded[0m unicode-ident v1.0.24
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m typenum v1.19.0
[1m[92m  Downloaded[0m tracing-core v0.1.36
[1m[92m  Downloaded[0m parking_lot v0.12.5
[1m[92m  Downloaded[0m schemars v1.2.1
[1m[92m  Downloaded[0m zerocopy-derive v0.8.48
[1m[92m  Downloaded[0m unicode-segmentation v1.13.2
[1m[92m  Downloaded[0m regex v1.12.3
[1m[92m  Downloaded[0m rand v0.9.2
[1m[92m  Downloaded[0m rayon-core v1.13.0
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m plotters v0.3.7
[1m[92m  Downloaded[0m serde v1.0.228
[1m[92m  Downloaded[0m num-traits v0.2.19
[1m[92m  Downloaded[0m rayon v1.11.0
[1m[92m  Downloaded[0m pallas-traverse v1.0.0-alpha.6
[1m[92m  Downloaded[0m pallas-crypto v1.0.0-alpha.6
[1m[92m  Downloaded[0m time v0.3.47
[1m[92m  Downloaded[0m criterion v0.8.2
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m tempfile v3.27.0
[1m[92m  Downloaded[0m zerocopy v0.8.48
[1m[92m  Downloaded[0m syn v2.0.117
[1m[92m  Downloaded[0m sha2 v0.11.0
[1m[92m  Downloaded[0m rand v0.8.5
[1m[92m  Downloaded[0m rustix v1.1.4
[1m[92m  Downloaded[0m pkcs8 v0.10.2
[1m[92m  Downloaded[0m indexmap v2.13.1
[1m[92m  Downloaded[0m tinytemplate v1.2.1
[1m[92m  Downloaded[0m parking_lot_core v0.9.12
[1m[92m  Downloaded[0m hashbrown v0.16.1
[1m[92m  Downloaded[0m tracing v0.1.44
[1m[92m  Downloaded[0m spki v0.7.3
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m version_check v0.9.5
[1m[92m  Downloaded[0m thiserror-impl v2.0.18
[1m[92m  Downloaded[0m sha2 v0.9.9
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m plotters-backend v0.3.7
[1m[92m  Downloaded[0m minicbor-derive v0.16.2
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m pin-project-lite v0.2.17
[1m[92m  Downloaded[0m num-conv v0.2.1
[1m[92m  Downloaded[0m memmap2 v0.9.10
[1m[92m  Downloaded[0m curve25519-dalek v4.1.3
[1m[92m  Downloaded[0m curve25519-dalek v3.2.0
[1m[92m  Downloaded[0m cpufeatures v0.3.0
[1m[92m  Downloaded[0m const-oid v0.10.2
[1m[92m  Downloaded[0m ciborium-ll v0.2.2
[1m[92m  Downloaded[0m chrono v0.4.44
[1m[92m  Downloaded[0m tokio v1.51.0
[1m[92m  Downloaded[0m cast v0.3.0
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m bitflags v2.11.0
[1m[92m  Downloaded[0m darling_core v0.23.0
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m cryptoxide v0.4.4
[1m[92m  Downloaded[0m clap_builder v4.6.0
[1m[92m  Downloaded[0m mio v1.2.0
[1m[92m  Downloaded[0m itertools v0.13.0
[1m[92m  Downloaded[0m cc v1.2.59
[1m[92m  Downloaded[0m base58 v0.2.0
[1m[92m  Downloaded[0m ref-cast v1.0.25
[1m[92m  Downloaded[0m dashu-int v0.4.1
[1m[92m  Downloaded[0m bytes v1.11.1
[1m[92m  Downloaded[0m bech32 v0.11.1
[1m[92m  Downloaded[0m plotters-svg v0.3.7
[1m[92m  Downloaded[0m half v2.7.1
[1m[92m  Downloaded[0m hybrid-array v0.4.10
[1m[92m  Downloaded[0m derive_more v2.1.1
[1m[92m  Downloaded[0m clap v4.6.0
[1m[92m  Downloaded[0m anstyle v1.0.14
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
[1m[92m  Downloaded[0m autocfg v1.5.0
[1m[92m  Downloaded[0m oorandom v11.1.5
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m getrandom v0.4.2
[1m[92m  Downloaded[0m byteorder v1.5.0
[1m[92m  Downloaded[0m rustc_version v0.4.1
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m crypto-common v0.2.1
[1m[92m  Downloaded[0m crypto-common v0.1.6
[1m[92m  Downloaded[0m crossbeam-epoch v0.9.18
[1m[92m  Downloaded[0m ciborium v0.2.2
[1m[92m  Downloaded[0m powerfmt v0.2.0
[1m[92m  Downloaded[0m num-integer v0.1.46
[1m[92m  Downloaded[0m lock_api v0.4.14
[1m[92m  Downloaded[0m convert_case v0.10.0
[1m[92m  Downloaded[0m constant_time_eq v0.4.2
[1m[92m  Downloaded[0m cfg-if v1.0.4
[1m[92m  Downloaded[0m itoa v1.0.18
[1m[92m  Downloaded[0m block-buffer v0.12.0
[1m[92m  Downloaded[0m bech32 v0.9.1
[1m[92m  Downloaded[0m rand_chacha v0.3.1
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m crc-catalog v2.4.0
[1m[92m  Downloaded[0m clap_lex v1.1.0
[1m[92m  Downloaded[0m ciborium-io v0.2.2
[1m[92m  Downloaded[0m blake2b_simd v1.0.4
[1m[92m  Downloaded[0m fastrand v2.4.1
[1m[92m  Downloaded[0m digest v0.9.0
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m pallas-codec v1.0.0-alpha.6
[1m[92m  Downloaded[0m darling v0.23.0
[1m[92m  Downloaded[0m alloca v0.4.0
[1m[92m  Downloaded[0m indexmap v1.9.3
[1m[92m  Downloaded[0m iana-time-zone v0.1.65
[1m[92m  Downloaded[0m getrandom v0.1.16
[1m[92m  Downloaded[0m find-msvc-tools v0.1.9
[1m[92m  Downloaded[0m either v1.15.0
[1m[92m  Downloaded[0m darling_macro v0.23.0
[1m[92m  Downloaded[0m crossbeam-utils v0.8.21
[1m[92m  Downloaded[0m arrayref v0.3.9
[1m[92m  Downloaded[0m anes v0.1.6
[1m[92m  Downloaded[0m num-order v1.2.0
[1m[92m  Downloaded[0m ident_case v1.0.1
[1m[92m  Downloaded[0m dashu-base v0.4.1
[1m[92m  Downloaded[0m criterion-plot v0.8.2
[1m[92m  Downloaded[0m crc v3.4.0
[1m[92m  Downloaded[0m block-buffer v0.9.0
[1m[92m  Downloaded[0m arrayvec v0.7.6
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m derive_more-impl v2.1.1
[1m[92m  Downloaded[0m der v0.7.10
[1m[92m  Downloaded[0m crc32fast v1.5.0
[1m[92m  Downloaded[0m ed25519 v2.2.3
[1m[92m  Downloaded[0m digest v0.11.2
[1m[92m  Downloaded[0m base64ct v1.8.3
[1m[92m  Downloaded[0m libc v0.2.184
[1m[92m  Downloaded[0m dyn-clone v1.0.20
[1m[92m  Downloaded[0m curve25519-dalek-derive v0.1.1
[1m[92m  Downloaded[0m cpufeatures v0.2.17
[1m[92m  Downloaded[0m pallas-addresses v1.0.0-alpha.6
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m opaque-debug v0.3.1
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m ed25519-dalek v2.2.0
[1m[92m  Downloaded[0m aho-corasick v1.1.4
[1m[92m   Compiling[0m proc-macro2 v1.0.106
[1m[92m   Compiling[0m quote v1.0.45
[1m[92m   Compiling[0m unicode-ident v1.0.24
[1m[92m   Compiling[0m cfg-if v1.0.4
[1m[92m   Compiling[0m libc v0.2.184
[1m[92m   Compiling[0m typenum v1.19.0
[1m[92m   Compiling[0m version_check v0.9.5
[1m[92m   Compiling[0m generic-array v0.14.9
[1m[92m   Compiling[0m zerocopy v0.8.48
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m subtle v2.6.1
[1m[92m   Compiling[0m autocfg v1.5.0
[1m[92m   Compiling[0m crypto-common v0.1.6
[1m[92m   Compiling[0m block-buffer v0.10.4
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m digest v0.10.7
[1m[92m   Compiling[0m semver v1.0.28
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m strsim v0.11.1
[1m[92m   Compiling[0m cpufeatures v0.2.17
[1m[92m   Compiling[0m ident_case v1.0.1
[1m[92m   Compiling[0m getrandom v0.3.4
[1m[92m   Compiling[0m rustc_version v0.4.1
[1m[92m   Compiling[0m getrandom v0.2.17
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m minicbor v0.26.5
[1m[92m   Compiling[0m rand_core v0.6.4
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m   Compiling[0m rand_core v0.9.5
[1m[92m   Compiling[0m signature v2.2.0
[1m[92m   Compiling[0m hex v0.4.3
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m sha2 v0.10.9
[1m[92m   Compiling[0m memchr v2.8.0
[1m[92m   Compiling[0m getrandom v0.1.16
[1m[92m   Compiling[0m itoa v1.0.18
[1m[92m   Compiling[0m crossbeam-utils v0.8.21
[1m[92m   Compiling[0m darling_core v0.23.0
[1m[92m   Compiling[0m cryptoxide v0.4.4
[1m[92m   Compiling[0m zerocopy-derive v0.8.48
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m darling_macro v0.23.0
[1m[92m   Compiling[0m minicbor-derive v0.16.2
[1m[92m   Compiling[0m thiserror-impl v1.0.69
[1m[92m   Compiling[0m darling v0.23.0
[1m[92m   Compiling[0m curve25519-dalek-derive v0.1.1
[1m[92m   Compiling[0m serde_with_macros v3.18.0
[1m[92m   Compiling[0m ed25519 v2.2.3
[1m[92m   Compiling[0m serde_with v3.18.0
[1m[92m   Compiling[0m either v1.15.0
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m unicode-segmentation v1.13.2
[1m[92m   Compiling[0m convert_case v0.10.0
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m digest v0.9.0
[1m[92m   Compiling[0m hybrid-array v0.4.10
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m unicode-xid v0.2.6
[1m[92m   Compiling[0m derive_more-impl v2.1.1
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m rand_core v0.5.1
[1m[92m   Compiling[0m pallas-codec v1.0.0-alpha.6
[1m[92m   Compiling[0m itertools v0.13.0
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.6
[1m[92m   Compiling[0m iana-time-zone v0.1.65
[1m[92m   Compiling[0m shlex v1.3.0
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m bytes v1.11.1
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m   Compiling[0m arrayref v0.3.9
[1m[92m   Compiling[0m parking_lot_core v0.9.12
[1m[92m   Compiling[0m constant_time_eq v0.4.2
[1m[92m   Compiling[0m arrayvec v0.7.6
[1m[92m   Compiling[0m rustversion v1.0.22
[1m[92m   Compiling[0m crc v3.4.0
[1m[92m   Compiling[0m blake2b_simd v1.0.4
[1m[92m   Compiling[0m cc v1.2.59
[1m[92m   Compiling[0m chrono v0.4.44
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m derive_more v2.1.1
[1m[92m   Compiling[0m crossbeam-epoch v0.9.18
[1m[92m   Compiling[0m crypto-common v0.2.1
[1m[92m   Compiling[0m block-buffer v0.12.0
[1m[92m   Compiling[0m blake2 v0.10.6
[1m[92m   Compiling[0m block-buffer v0.9.0
[1m[92m   Compiling[0m smallvec v1.15.1
[1m[92m   Compiling[0m bech32 v0.9.1
[1m[92m   Compiling[0m base58 v0.2.0
[1m[92m   Compiling[0m const-oid v0.10.2
[1m[92m   Compiling[0m rayon-core v1.13.0
[1m[92m   Compiling[0m num-modular v0.6.1
[1m[92m   Compiling[0m scopeguard v1.2.0
[1m[92m   Compiling[0m pin-project-lite v0.2.17
[1m[92m   Compiling[0m opaque-debug v0.3.1
[1m[92m   Compiling[0m bech32 v0.11.1
[1m[92m   Compiling[0m sha2 v0.9.9
[1m[92m   Compiling[0m dugite-primitives v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m num-order v1.2.0
[1m[92m   Compiling[0m lock_api v0.4.14
[1m[92m   Compiling[0m digest v0.11.2
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.6
[1m[92m   Compiling[0m crossbeam-deque v0.8.6
[1m[92m   Compiling[0m alloca v0.4.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m tracing-core v0.1.36
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.6
[1m[92m   Compiling[0m rand_chacha v0.3.1
[1m[92m   Compiling[0m tracing-attributes v0.1.31
[1m[92m   Compiling[0m errno v0.3.14
[1m[92m   Compiling[0m cpufeatures v0.3.0
[1m[92m   Compiling[0m dashu-base v0.4.1
[1m[92m   Compiling[0m ciborium-io v0.2.2
[1m[92m   Compiling[0m clap_lex v1.1.0
[1m[92m   Compiling[0m anstyle v1.0.14
[1m[92m   Compiling[0m crc32fast v1.5.0
[1m[92m   Compiling[0m plotters-backend v0.3.7
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m static_assertions v1.1.0
[1m[92m   Compiling[0m dashu-int v0.4.1
[1m[92m   Compiling[0m tracing v0.1.44
[1m[92m   Compiling[0m plotters-svg v0.3.7
[1m[92m   Compiling[0m clap_builder v4.6.0
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.6
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m rand v0.8.5
[1m[92m   Compiling[0m sha2 v0.11.0
[1m[92m   Compiling[0m signal-hook-registry v1.4.8
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m parking_lot v0.12.5
[1m[92m   Compiling[0m tokio-macros v2.7.0
[1m[92m   Compiling[0m socket2 v0.6.3
[1m[92m   Compiling[0m mio v1.2.0
[1m[92m   Compiling[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m cast v0.3.0
[1m[92m   Compiling[0m bitflags v2.11.0
[1m[92m   Compiling[0m same-file v1.0.6
[1m[92m   Compiling[0m criterion-plot v0.8.2
[1m[92m   Compiling[0m walkdir v2.5.0
[1m[92m   Compiling[0m tokio v1.51.0
[1m[92m   Compiling[0m rayon v1.11.0
[1m[92m   Compiling[0m dugite-crypto v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m dugite-serialization v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m clap v4.6.0
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m page_size v0.6.0
[1m[92m   Compiling[0m memmap2 v0.9.10
[1m[92m   Compiling[0m anes v0.1.6
[1m[92m   Compiling[0m oorandom v11.1.5
[1m[92m   Compiling[0m fastrand v2.4.1
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m criterion v0.8.2
[1m[92m   Compiling[0m dugite-storage v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 12s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-4baba329a1628840)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 44.1s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 44.055 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [3.8944 s 4.0656 s 4.3206 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 386.5s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 386.46 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [30.650 ms 30.860 ms 31.068 ms]
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 4130.2s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 4130.2 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [348.25 ms 351.57 ms 354.88 ms]

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 357.5s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 357.53 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [30.535 ms 30.735 ms 30.931 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) low mild

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 351.2s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 351.18 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [30.524 ms 30.698 ms 30.870 ms]

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 346.4s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 346.44 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [31.447 ms 31.639 ms 31.832 ms]

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.4s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 7.3831 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [6.3755 ms 6.5062 ms 6.6383 ms]

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.6s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 35.613 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.4462 s 3.4803 s 3.5319 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.4s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 34.415 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [3.4488 s 3.4709 s 3.4943 s]
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.4s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 34.397 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [29.988 ms 30.511 ms 31.066 ms]
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.2s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 35.159 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [30.763 ms 31.305 ms 31.798 ms]

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 5.6889 s (300 iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [19.306 ms 19.424 ms 19.545 ms]
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.0400 s (25k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [198.85 µs 198.96 µs 199.08 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.1031 s (1700 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [3.0081 ms 3.0207 ms 3.0348 ms]
Found 5 outliers among 100 measurements (5.00%)
  3 (3.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 113.6s, or reduce sample count to 10.
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 113.61 s (100 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [868.44 ms 869.58 ms 871.13 ms]
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) low mild
  1 (1.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.2s, enable flat sampling, or reduce sample count to 50.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 9.2335 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.7690 ms 1.7768 ms 1.7869 ms]
Found 8 outliers among 100 measurements (8.00%)
  6 (6.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.2070 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [25.964 ms 26.035 ms 26.121 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.5220 s (500 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [11.068 ms 11.113 ms 11.161 ms]
Found 13 outliers among 100 measurements (13.00%)
  7 (7.00%) high mild
  6 (6.00%) high severe
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.4913 s (500 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [10.978 ms 11.003 ms 11.029 ms]
Found 8 outliers among 100 measurements (8.00%)
  5 (5.00%) high mild
  3 (3.00%) high severe

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.1058 s (152k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [34.351 µs 34.498 µs 34.629 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.0941 s (152k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [34.360 µs 34.506 µs 34.638 µs]

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 5.8829 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [14.669 ms 14.683 ms 14.698 ms]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.1535 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [15.295 ms 15.323 ms 15.355 ms]
Found 5 outliers among 100 measurements (5.00%)
  3 (3.00%) high mild
  2 (2.00%) high severe

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 6.4687 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [317.69 µs 318.16 µs 318.69 µs]
Found 10 outliers among 100 measurements (10.00%)
  3 (3.00%) low severe
  7 (7.00%) low mild
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 6.4932 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [322.11 µs 322.58 µs 322.99 µs]
Found 8 outliers among 100 measurements (8.00%)
  2 (2.00%) low severe
  6 (6.00%) low mild

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 8.1116 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [796.07 µs 797.50 µs 798.97 µs]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.4242 s (900 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.0541 ms 6.0742 ms 6.0993 ms]
Found 7 outliers among 100 measurements (7.00%)
  3 (3.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2779 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.4645 ms 3.4739 ms 3.4831 ms]
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 8.7783 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [43.788 ms 43.948 ms 44.165 ms]
Found 7 outliers among 100 measurements (7.00%)
  3 (3.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.7043 s (800 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.0806 ms 7.1209 ms 7.1660 ms]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high severe
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.6s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 8.5557 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [85.292 ms 85.403 ms 85.519 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0204 s (278k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [18.109 µs 18.155 µs 18.205 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.1273 s (172k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [27.704 µs 27.755 µs 27.801 µs]
Found 21 outliers among 100 measurements (21.00%)
  16 (16.00%) low mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0475 s (273k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [18.596 µs 18.633 µs 18.672 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0417 s (247k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [20.629 µs 20.662 µs 20.693 µs]
Found 20 outliers among 100 measurements (20.00%)
  8 (8.00%) low severe
  7 (7.00%) low mild
  4 (4.00%) high mild
  1 (1.00%) high severe
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0513 s (273k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [18.608 µs 18.645 µs 18.683 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0208 s (252k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [20.080 µs 20.114 µs 20.146 µs]
Found 23 outliers among 100 measurements (23.00%)
  10 (10.00%) low severe
  6 (6.00%) low mild
  6 (6.00%) high mild
  1 (1.00%) high severe

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0091 s (429k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.663 µs 11.676 µs 11.699 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.0788 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [41.741 µs 41.868 µs 41.992 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0099 s (6270 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [798.48 µs 799.82 µs 800.95 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.0159 s (825 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.1039 ms 6.1332 ms 6.1944 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.0341 s (1430 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.5094 ms 3.5129 ms 3.5164 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 7.3335 s (165 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [43.935 ms 44.050 ms 44.186 ms]
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.1308 s (715 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.1500 ms 7.1592 ms 7.1689 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 9.3442 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [84.942 ms 85.245 ms 85.794 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 6.0171 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [26.907 ms 26.997 ms 27.165 ms]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 9.6s or enable flat sampling.
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 9.5769 s (55 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [173.82 ms 173.99 ms 174.18 ms]
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.3072 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [65.931 ms 66.900 ms 67.864 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 6.1360 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [307.29 ms 309.13 ms 311.02 ms]
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.4s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 8.3631 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [150.91 ms 151.78 ms 153.27 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.5s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 5.4719 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [554.01 ms 569.61 ms 586.05 ms]

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0006 s (278k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [18.132 µs 18.140 µs 18.152 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0013 s (165k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [30.553 µs 30.570 µs 30.589 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0003 s (271k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [18.567 µs 18.588 µs 18.607 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0010 s (242k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.820 µs 20.832 µs 20.850 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0010 s (269k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [18.689 µs 18.698 µs 18.711 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0001 s (248k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [20.288 µs 20.293 µs 20.298 µs]
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0008 s (265k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [18.798 µs 18.802 µs 18.806 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0001 s (249k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [20.180 µs 20.185 µs 20.187 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0007 s (268k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [18.770 µs 18.788 µs 18.806 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0001 s (253k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.897 µs 19.905 µs 19.913 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0004 s (267k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [18.859 µs 18.867 µs 18.876 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0006 s (254k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.805 µs 19.811 µs 19.817 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 5.4999 s (275 iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [19.277 ms 19.421 ms 19.614 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0020 s (25k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [200.06 µs 200.12 µs 200.22 µs]
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 8.8506 s (20 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [439.81 ms 440.57 ms 441.30 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0034 s (6380 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [778.07 µs 779.12 µs 780.85 µs]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.2s.
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 11.239 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [871.34 ms 872.84 ms 874.53 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0584 s (2860 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [1.7519 ms 1.7598 ms 1.7708 ms]
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 43.5s.
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 43.508 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [2.2613 s 2.2804 s 2.3114 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.0990 s (990 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [5.1992 ms 5.2242 ms 5.2429 ms]
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 76.0s.
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 75.956 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [22.855 s 23.519 s 24.431 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.2579 s (550 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [9.6111 ms 9.7961 ms 9.9654 ms]

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 41.0s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 40.973 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [3.4288 s 3.4686 s 3.5141 s]
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 195.1s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 195.07 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [17.431 s 17.519 s 17.638 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 351.3s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 351.33 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [34.916 s 35.017 s 35.125 s]
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 891.1s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 891.10 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [87.467 s 87.829 s 88.219 s]

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m zeroize_derive v1.4.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m minicbor-derive v0.15.3
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m secp256k1-sys v0.8.2
[1m[92m   Compiling[0m radium v0.7.0
[1m[92m   Compiling[0m log v0.4.29
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.6
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m pallas-codec v0.33.0
[1m[92m   Compiling[0m pallas-crypto v0.33.0
[1m[92m   Compiling[0m blst v0.3.16
[1m[92m   Compiling[0m indexmap v1.9.3
[1m[92m   Compiling[0m num_cpus v1.17.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m peg-runtime v0.8.5
[1m[92m   Compiling[0m heck v0.5.0
[1m[92m   Compiling[0m tap v1.0.1
[1m[92m   Compiling[0m wyz v0.5.1
[1m[92m   Compiling[0m strum_macros v0.26.4
[1m[92m   Compiling[0m dugite-primitives v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m peg-macros v0.8.5
[1m[92m   Compiling[0m threadpool v1.8.1
[1m[92m   Compiling[0m pallas-primitives v0.33.0
[1m[92m   Compiling[0m pallas-addresses v0.33.0
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.6
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.6
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m miette-derive v5.10.0
[1m[92m   Compiling[0m arrayvec v0.5.2
[1m[92m   Compiling[0m hashbrown v0.12.3
[1m[92m   Compiling[0m unicode-width v0.1.14
[1m[92m   Compiling[0m funty v2.0.0
[1m[92m   Compiling[0m typed-arena v2.0.2
[1m[92m   Compiling[0m unicode-segmentation v1.13.2
[1m[92m   Compiling[0m bitvec v1.0.1
[1m[92m   Compiling[0m pretty v0.11.3
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.6
[1m[92m   Compiling[0m peg v0.8.5
[1m[92m   Compiling[0m miette v5.10.0
[1m[92m   Compiling[0m pallas-traverse v0.33.0
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m secp256k1 v0.26.0
[1m[92m   Compiling[0m strum v0.26.3
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m itertools v0.10.5
[1m[92m   Compiling[0m wait-timeout v0.2.1
[1m[92m   Compiling[0m fs2 v0.4.3
[1m[92m   Compiling[0m bit-vec v0.8.0
[1m[92m   Compiling[0m fnv v1.0.7
[1m[92m   Compiling[0m hamming v0.1.3
[1m[92m   Compiling[0m quick-error v1.2.3
[1m[92m   Compiling[0m rusty-fork v0.3.1
[1m[92m   Compiling[0m uplc v1.1.21 (https://github.com/aiken-lang/aiken.git?rev=6806d52211fbb469ae13aa0e0290aeb6b9b3e8cf#6806d522)
[1m[92m   Compiling[0m bit-set v0.8.0
[1m[92m   Compiling[0m dugite-lsm v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m dugite-crypto v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m dugite-serialization v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m rand_xorshift v0.4.0
[1m[92m   Compiling[0m bincode v1.3.3
[1m[92m   Compiling[0m unarray v0.1.4
[1m[92m   Compiling[0m criterion v0.8.2
[1m[92m   Compiling[0m proptest v1.11.0
[1m[92m   Compiling[0m dugite-ledger v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 08s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-d09dbf192278226c)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.4s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 25.391 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.4641 s 2.4769 s 2.4892 s]

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.1483 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [507.75 µs 510.29 µs 513.07 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 6.2500 s (20k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [307.39 µs 307.60 µs 307.83 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 6.1839 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [407.88 µs 408.09 µs 408.34 µs]
Found 11 outliers among 100 measurements (11.00%)
  6 (6.00%) high mild
  5 (5.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 6.0361 s (20k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [297.07 µs 298.03 µs 299.10 µs]
Found 16 outliers among 100 measurements (16.00%)
  2 (2.00%) low severe
  10 (10.00%) low mild
  2 (2.00%) high mild
  2 (2.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 48.7s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 48.679 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.5650 s 2.5775 s 2.5919 s]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.0s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 24.951 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [253.11 ms 257.48 ms 261.39 ms]
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.9s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 24.882 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [255.96 ms 259.27 ms 262.20 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) low mild

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.8s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 34.750 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.2866 s 3.3053 s 3.3244 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 33.5s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 33.505 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [120.90 ms 127.71 ms 133.73 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 27.6s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 27.587 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [283.66 ms 285.48 ms 287.32 ms]

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.1s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 35.087 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [567.94 ms 570.75 ms 574.53 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.1s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 25.107 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.4803 s 2.4858 s 2.4918 s]
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.8s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 24.827 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.4702 s 2.4765 s 2.4830 s]
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.7s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 24.714 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.4634 s 2.4680 s 2.4727 s]
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.7s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 24.679 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.4681 s 2.4789 s 2.4896 s]
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.9s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 24.868 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.4759 s 2.4818 s 2.4883 s]

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 6.5668 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [439.89 µs 441.79 µs 443.63 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 6.6020 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [443.07 µs 444.81 µs 446.48 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 6.5465 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [438.62 µs 440.47 µs 442.24 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 6.5766 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [442.73 µs 444.62 µs 446.41 µs]
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 6.5603 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [439.55 µs 441.43 µs 443.22 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 6.1739 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [204.12 ms 205.39 ms 206.82 ms]
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.8s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 11.779 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.1584 s 1.1671 s 1.1755 s]
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.8s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 24.789 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.4622 s 2.4715 s 2.4803 s]

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0042 s (13k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [356.40 µs 357.08 µs 357.81 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0220 s (12k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [405.76 µs 405.92 µs 406.18 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0053 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [434.13 µs 434.89 µs 435.53 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 6.3151 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [13.690 ms 14.089 ms 14.486 ms]
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.5s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 11.526 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [103.39 ms 108.84 ms 114.86 ms]
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 24.6s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 24.624 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [236.47 ms 240.41 ms 244.68 ms]

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.4513 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [29.027 ms 29.121 ms 29.185 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.9s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 7.9171 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [143.04 ms 143.20 ms 143.29 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 5.8746 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [279.34 ms 286.13 ms 291.64 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high severe

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 143.2s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 143.24 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [14.209 s 14.241 s 14.277 s]
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 321.4s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 321.41 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [32.051 s 32.159 s 32.275 s]

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0305 s (4125 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.1910 ms 1.1914 ms 1.1921 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0073 s (3245 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.5091 ms 1.5102 ms 1.5112 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 16.1s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 16.079 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.6223 s 1.6462 s 1.6664 s]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low severe
  2 (20.00%) high severe
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 31.4s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 31.367 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.1965 s 3.2346 s 3.2625 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

```

## LSM Stress Tests
```
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m rusty-fork v0.3.1
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m rand v0.9.2
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m zerocopy-derive v0.8.48
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m dugite-lsm v1.0.3-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
[1m[92m   Compiling[0m zerocopy v0.8.48
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m proptest v1.11.0
[1m[92m   Compiling[0m criterion v0.8.2
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 28.60s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/dugite_lsm-44c73c6590ff0f59)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.08s

[1m[92m   Doc-tests[0m dugite_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
