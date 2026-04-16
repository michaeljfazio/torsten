# Nightly Benchmark Results — 2026-04-16

Machine: GitHub Actions ubuntu-latest
Branch: main (76d1943)

## Storage Benchmarks
```
[1m[92m    Updating[0m crates.io index
[1m[92m    Updating[0m git repository `https://github.com/iquerejeta/curve25519-dalek`
[1m[92m    Updating[0m git repository `https://github.com/input-output-hk/vrf`
[1m[92m    Updating[0m git repository `https://github.com/aiken-lang/aiken.git`
[1m[92m Downloading[0m crates ...
[1m[92m  Downloaded[0m block-buffer v0.12.0
[1m[92m  Downloaded[0m autocfg v1.5.0
[1m[92m  Downloaded[0m num-conv v0.2.1
[1m[92m  Downloaded[0m itoa v1.0.18
[1m[92m  Downloaded[0m anes v0.1.6
[1m[92m  Downloaded[0m base64 v0.22.1
[1m[92m  Downloaded[0m hashbrown v0.12.3
[1m[92m  Downloaded[0m anstyle v1.0.14
[1m[92m  Downloaded[0m opaque-debug v0.3.1
[1m[92m  Downloaded[0m const-oid v0.9.6
[1m[92m  Downloaded[0m powerfmt v0.2.0
[1m[92m  Downloaded[0m const-oid v0.10.2
[1m[92m  Downloaded[0m cpufeatures v0.2.17
[1m[92m  Downloaded[0m scopeguard v1.2.0
[1m[92m  Downloaded[0m deranged v0.5.8
[1m[92m  Downloaded[0m same-file v1.0.6
[1m[92m  Downloaded[0m darling_macro v0.23.0
[1m[92m  Downloaded[0m strsim v0.11.1
[1m[92m  Downloaded[0m tokio-macros v2.7.0
[1m[92m  Downloaded[0m version_check v0.9.5
[1m[92m  Downloaded[0m time-core v0.1.8
[1m[92m  Downloaded[0m zeroize v1.8.2
[1m[92m  Downloaded[0m thiserror v1.0.69
[1m[92m  Downloaded[0m rustc_version v0.4.1
[1m[92m  Downloaded[0m zmij v1.0.21
[1m[92m  Downloaded[0m semver v1.0.28
[1m[92m  Downloaded[0m shlex v1.3.0
[1m[92m  Downloaded[0m subtle v2.6.1
[1m[92m  Downloaded[0m unicode-xid v0.2.6
[1m[92m  Downloaded[0m thiserror v2.0.18
[1m[92m  Downloaded[0m tracing-core v0.1.36
[1m[92m  Downloaded[0m static_assertions v1.1.0
[1m[92m  Downloaded[0m tracing-attributes v0.1.31
[1m[92m  Downloaded[0m smallvec v1.15.1
[1m[92m  Downloaded[0m signal-hook-registry v1.4.8
[1m[92m  Downloaded[0m unicode-ident v1.0.24
[1m[92m  Downloaded[0m getrandom v0.4.2
[1m[92m  Downloaded[0m zerocopy-derive v0.8.48
[1m[92m  Downloaded[0m typenum v1.19.0
[1m[92m  Downloaded[0m der v0.7.10
[1m[92m  Downloaded[0m unicode-segmentation v1.13.2
[1m[92m  Downloaded[0m itertools v0.13.0
[1m[92m  Downloaded[0m serde_json v1.0.149
[1m[92m  Downloaded[0m plotters v0.3.7
[1m[92m  Downloaded[0m regex v1.12.3
[1m[92m  Downloaded[0m serde_with v3.18.0
[1m[92m  Downloaded[0m time v0.3.47
[1m[92m  Downloaded[0m chrono v0.4.44
[1m[92m  Downloaded[0m cryptoxide v0.4.4
[1m[92m  Downloaded[0m rayon v1.11.0
[1m[92m  Downloaded[0m hashbrown v0.17.0
[1m[92m  Downloaded[0m zerocopy v0.8.48
[1m[92m  Downloaded[0m dashu-int v0.4.1
[1m[92m  Downloaded[0m syn v2.0.117
[1m[92m  Downloaded[0m curve25519-dalek v4.1.3
[1m[92m  Downloaded[0m regex-syntax v0.8.10
[1m[92m  Downloaded[0m indexmap v2.14.0
[1m[92m  Downloaded[0m rustix v1.1.4
[1m[92m  Downloaded[0m curve25519-dalek v3.2.0
[1m[92m  Downloaded[0m derive_more-impl v2.1.1
[1m[92m  Downloaded[0m criterion v0.8.2
[1m[92m  Downloaded[0m num-bigint v0.4.6
[1m[92m  Downloaded[0m tracing v0.1.44
[1m[92m  Downloaded[0m mio v1.2.0
[1m[92m  Downloaded[0m memchr v2.8.0
[1m[92m  Downloaded[0m ed25519-dalek v2.2.0
[1m[92m  Downloaded[0m schemars v1.2.1
[1m[92m  Downloaded[0m rand v0.9.4
[1m[92m  Downloaded[0m regex-automata v0.4.14
[1m[92m  Downloaded[0m clap v4.6.0
[1m[92m  Downloaded[0m darling_core v0.23.0
[1m[92m  Downloaded[0m serde v1.0.228
[1m[92m  Downloaded[0m hybrid-array v0.4.10
[1m[92m  Downloaded[0m half v2.7.1
[1m[92m  Downloaded[0m sha2 v0.9.9
[1m[92m  Downloaded[0m serde_derive v1.0.228
[1m[92m  Downloaded[0m minicbor v0.26.5
[1m[92m  Downloaded[0m indexmap v1.9.3
[1m[92m  Downloaded[0m getrandom v0.3.4
[1m[92m  Downloaded[0m libc v0.2.185
[1m[92m  Downloaded[0m clap_builder v4.6.0
[1m[92m  Downloaded[0m rayon-core v1.13.0
[1m[92m  Downloaded[0m tokio v1.51.1
[1m[92m  Downloaded[0m getrandom v0.2.17
[1m[92m  Downloaded[0m thiserror-impl v2.0.18
[1m[92m  Downloaded[0m thiserror-impl v1.0.69
[1m[92m  Downloaded[0m sha2 v0.11.0
[1m[92m  Downloaded[0m serde_with_macros v3.18.0
[1m[92m  Downloaded[0m schemars v0.9.0
[1m[92m  Downloaded[0m pallas-primitives v1.0.0-alpha.6
[1m[92m  Downloaded[0m num-modular v0.6.1
[1m[92m  Downloaded[0m num-integer v0.1.46
[1m[92m  Downloaded[0m derive_more v2.1.1
[1m[92m  Downloaded[0m crossbeam-utils v0.8.21
[1m[92m  Downloaded[0m crossbeam-epoch v0.9.18
[1m[92m  Downloaded[0m num-order v1.2.0
[1m[92m  Downloaded[0m memmap2 v0.9.10
[1m[92m  Downloaded[0m getrandom v0.1.16
[1m[92m  Downloaded[0m find-msvc-tools v0.1.9
[1m[92m  Downloaded[0m tempfile v3.27.0
[1m[92m  Downloaded[0m socket2 v0.6.3
[1m[92m  Downloaded[0m num-traits v0.2.19
[1m[92m  Downloaded[0m lock_api v0.4.14
[1m[92m  Downloaded[0m iana-time-zone v0.1.65
[1m[92m  Downloaded[0m ed25519 v2.2.3
[1m[92m  Downloaded[0m dyn-clone v1.0.20
[1m[92m  Downloaded[0m blake2 v0.10.6
[1m[92m  Downloaded[0m base64ct v1.8.3
[1m[92m  Downloaded[0m sha2 v0.10.9
[1m[92m  Downloaded[0m serde_core v1.0.228
[1m[92m  Downloaded[0m proc-macro2 v1.0.106
[1m[92m  Downloaded[0m pin-project-lite v0.2.17
[1m[92m  Downloaded[0m parking_lot v0.12.5
[1m[92m  Downloaded[0m minicbor-derive v0.16.2
[1m[92m  Downloaded[0m time-macros v0.2.27
[1m[92m  Downloaded[0m crypto-common v0.2.1
[1m[92m  Downloaded[0m cfg-if v1.0.4
[1m[92m  Downloaded[0m num-rational v0.4.2
[1m[92m  Downloaded[0m generic-array v0.14.9
[1m[92m  Downloaded[0m convert_case v0.10.0
[1m[92m  Downloaded[0m pkcs8 v0.10.2
[1m[92m  Downloaded[0m parking_lot_core v0.9.12
[1m[92m  Downloaded[0m hex v0.4.3
[1m[92m  Downloaded[0m digest v0.9.0
[1m[92m  Downloaded[0m blake2b_simd v1.0.4
[1m[92m  Downloaded[0m either v1.15.0
[1m[92m  Downloaded[0m crossbeam-deque v0.8.6
[1m[92m  Downloaded[0m walkdir v2.5.0
[1m[92m  Downloaded[0m tinytemplate v1.2.1
[1m[92m  Downloaded[0m signature v2.2.0
[1m[92m  Downloaded[0m rand_core v0.5.1
[1m[92m  Downloaded[0m equivalent v1.0.2
[1m[92m  Downloaded[0m digest v0.11.2
[1m[92m  Downloaded[0m digest v0.10.7
[1m[92m  Downloaded[0m dashu-base v0.4.1
[1m[92m  Downloaded[0m rand_core v0.6.4
[1m[92m  Downloaded[0m plotters-backend v0.3.7
[1m[92m  Downloaded[0m paste v1.0.15
[1m[92m  Downloaded[0m once_cell v1.21.4
[1m[92m  Downloaded[0m block-buffer v0.9.0
[1m[92m  Downloaded[0m spki v0.7.3
[1m[92m  Downloaded[0m pallas-crypto v1.0.0-alpha.6
[1m[92m  Downloaded[0m pallas-codec v1.0.0-alpha.6
[1m[92m  Downloaded[0m fastrand v2.4.1
[1m[92m  Downloaded[0m errno v0.3.14
[1m[92m  Downloaded[0m rand_core v0.9.5
[1m[92m  Downloaded[0m rand_chacha v0.9.0
[1m[92m  Downloaded[0m quote v1.0.45
[1m[92m  Downloaded[0m ppv-lite86 v0.2.21
[1m[92m  Downloaded[0m oorandom v11.1.5
[1m[92m  Downloaded[0m ident_case v1.0.1
[1m[92m  Downloaded[0m ciborium-ll v0.2.2
[1m[92m  Downloaded[0m cast v0.3.0
[1m[92m  Downloaded[0m arrayref v0.3.9
[1m[92m  Downloaded[0m ref-cast-impl v1.0.25
[1m[92m  Downloaded[0m curve25519-dalek-derive v0.1.1
[1m[92m  Downloaded[0m crc-catalog v2.4.0
[1m[92m  Downloaded[0m rustversion v1.0.22
[1m[92m  Downloaded[0m ref-cast v1.0.25
[1m[92m  Downloaded[0m plotters-svg v0.3.7
[1m[92m  Downloaded[0m pallas-traverse v1.0.0-alpha.6
[1m[92m  Downloaded[0m constant_time_eq v0.4.2
[1m[92m  Downloaded[0m block-buffer v0.10.4
[1m[92m  Downloaded[0m darling v0.23.0
[1m[92m  Downloaded[0m crypto-common v0.1.6
[1m[92m  Downloaded[0m crc v3.4.0
[1m[92m  Downloaded[0m ciborium-io v0.2.2
[1m[92m  Downloaded[0m pallas-addresses v1.0.0-alpha.6
[1m[92m  Downloaded[0m page_size v0.6.0
[1m[92m  Downloaded[0m criterion-plot v0.8.2
[1m[92m  Downloaded[0m cc v1.2.60
[1m[92m  Downloaded[0m bytes v1.11.1
[1m[92m  Downloaded[0m bitflags v2.11.0
[1m[92m  Downloaded[0m bech32 v0.11.1
[1m[92m  Downloaded[0m crc32fast v1.5.0
[1m[92m  Downloaded[0m clap_lex v1.1.0
[1m[92m  Downloaded[0m ciborium v0.2.2
[1m[92m  Downloaded[0m bech32 v0.9.1
[1m[92m  Downloaded[0m cpufeatures v0.3.0
[1m[92m  Downloaded[0m byteorder v1.5.0
[1m[92m  Downloaded[0m base58 v0.2.0
[1m[92m  Downloaded[0m alloca v0.4.0
[1m[92m  Downloaded[0m aho-corasick v1.1.4
[1m[92m  Downloaded[0m arrayvec v0.7.6
[1m[92m  Downloaded[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m proc-macro2 v1.0.106
[1m[92m   Compiling[0m quote v1.0.45
[1m[92m   Compiling[0m unicode-ident v1.0.24
[1m[92m   Compiling[0m cfg-if v1.0.4
[1m[92m   Compiling[0m libc v0.2.185
[1m[92m   Compiling[0m typenum v1.19.0
[1m[92m   Compiling[0m version_check v0.9.5
[1m[92m   Compiling[0m generic-array v0.14.9
[1m[92m   Compiling[0m serde_core v1.0.228
[1m[92m   Compiling[0m syn v2.0.117
[1m[92m   Compiling[0m zerocopy v0.8.48
[1m[92m   Compiling[0m serde v1.0.228
[1m[92m   Compiling[0m subtle v2.6.1
[1m[92m   Compiling[0m autocfg v1.5.0
[1m[92m   Compiling[0m block-buffer v0.10.4
[1m[92m   Compiling[0m crypto-common v0.1.6
[1m[92m   Compiling[0m digest v0.10.7
[1m[92m   Compiling[0m num-traits v0.2.19
[1m[92m   Compiling[0m strsim v0.11.1
[1m[92m   Compiling[0m semver v1.0.28
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m cpufeatures v0.2.17
[1m[92m   Compiling[0m getrandom v0.3.4
[1m[92m   Compiling[0m ident_case v1.0.1
[1m[92m   Compiling[0m rustc_version v0.4.1
[1m[92m   Compiling[0m minicbor v0.26.5
[1m[92m   Compiling[0m thiserror v1.0.69
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m zmij v1.0.21
[1m[92m   Compiling[0m rand_core v0.9.5
[1m[92m   Compiling[0m signature v2.2.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m hex v0.4.3
[1m[92m   Compiling[0m sha2 v0.10.9
[1m[92m   Compiling[0m itoa v1.0.18
[1m[92m   Compiling[0m darling_core v0.23.0
[1m[92m   Compiling[0m getrandom v0.1.16
[1m[92m   Compiling[0m memchr v2.8.0
[1m[92m   Compiling[0m rand_core v0.6.4
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
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m crossbeam-utils v0.8.21
[1m[92m   Compiling[0m unicode-segmentation v1.13.2
[1m[92m   Compiling[0m either v1.15.0
[1m[92m   Compiling[0m convert_case v0.10.0
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m rand v0.9.4
[1m[92m   Compiling[0m digest v0.9.0
[1m[92m   Compiling[0m hybrid-array v0.4.10
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m unicode-xid v0.2.6
[1m[92m   Compiling[0m derive_more-impl v2.1.1
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m rand_core v0.5.1
[1m[92m   Compiling[0m pallas-codec v1.0.0-alpha.6
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.6
[1m[92m   Compiling[0m itertools v0.13.0
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m parking_lot_core v0.9.12
[1m[92m   Compiling[0m bytes v1.11.1
[1m[92m   Compiling[0m constant_time_eq v0.4.2
[1m[92m   Compiling[0m paste v1.0.15
[1m[92m   Compiling[0m rustversion v1.0.22
[1m[92m   Compiling[0m iana-time-zone v0.1.65
[1m[92m   Compiling[0m arrayref v0.3.9
[1m[92m   Compiling[0m once_cell v1.21.4
[1m[92m   Compiling[0m crc-catalog v2.4.0
[1m[92m   Compiling[0m arrayvec v0.7.6
[1m[92m   Compiling[0m shlex v1.3.0
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m find-msvc-tools v0.1.9
[1m[92m   Compiling[0m blake2b_simd v1.0.4
[1m[92m   Compiling[0m crc v3.4.0
[1m[92m   Compiling[0m cc v1.2.60
[1m[92m   Compiling[0m chrono v0.4.44
[1m[92m   Compiling[0m derive_more v2.1.1
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m crossbeam-epoch v0.9.18
[1m[92m   Compiling[0m block-buffer v0.12.0
[1m[92m   Compiling[0m crypto-common v0.2.1
[1m[92m   Compiling[0m blake2 v0.10.6
[1m[92m   Compiling[0m block-buffer v0.9.0
[1m[92m   Compiling[0m smallvec v1.15.1
[1m[92m   Compiling[0m opaque-debug v0.3.1
[1m[92m   Compiling[0m num-modular v0.6.1
[1m[92m   Compiling[0m pin-project-lite v0.2.17
[1m[92m   Compiling[0m scopeguard v1.2.0
[1m[92m   Compiling[0m const-oid v0.10.2
[1m[92m   Compiling[0m base58 v0.2.0
[1m[92m   Compiling[0m bech32 v0.11.1
[1m[92m   Compiling[0m bech32 v0.9.1
[1m[92m   Compiling[0m rayon-core v1.13.0
[1m[92m   Compiling[0m num-order v1.2.0
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.6
[1m[92m   Compiling[0m dugite-primitives v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m digest v0.11.2
[1m[92m   Compiling[0m lock_api v0.4.14
[1m[92m   Compiling[0m sha2 v0.9.9
[1m[92m   Compiling[0m crossbeam-deque v0.8.6
[1m[92m   Compiling[0m alloca v0.4.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m tracing-core v0.1.36
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.6
[1m[92m   Compiling[0m tracing-attributes v0.1.31
[1m[92m   Compiling[0m errno v0.3.14
[1m[92m   Compiling[0m clap_lex v1.1.0
[1m[92m   Compiling[0m anstyle v1.0.14
[1m[92m   Compiling[0m dashu-base v0.4.1
[1m[92m   Compiling[0m cpufeatures v0.3.0
[1m[92m   Compiling[0m crc32fast v1.5.0
[1m[92m   Compiling[0m plotters-backend v0.3.7
[1m[92m   Compiling[0m rustix v1.1.4
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m ciborium-io v0.2.2
[1m[92m   Compiling[0m static_assertions v1.1.0
[1m[92m   Compiling[0m getrandom v0.4.2
[1m[92m   Compiling[0m dashu-int v0.4.1
[1m[92m   Compiling[0m tracing v0.1.44
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m plotters-svg v0.3.7
[1m[92m   Compiling[0m sha2 v0.11.0
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.6
[1m[92m   Compiling[0m clap_builder v4.6.0
[1m[92m   Compiling[0m signal-hook-registry v1.4.8
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m parking_lot v0.12.5
[1m[92m   Compiling[0m tokio-macros v2.7.0
[1m[92m   Compiling[0m mio v1.2.0
[1m[92m   Compiling[0m socket2 v0.6.3
[1m[92m   Compiling[0m same-file v1.0.6
[1m[92m   Compiling[0m bitflags v2.11.0
[1m[92m   Compiling[0m cast v0.3.0
[1m[92m   Compiling[0m linux-raw-sys v0.12.1
[1m[92m   Compiling[0m criterion-plot v0.8.2
[1m[92m   Compiling[0m tokio v1.51.1
[1m[92m   Compiling[0m walkdir v2.5.0
[1m[92m   Compiling[0m rayon v1.11.0
[1m[92m   Compiling[0m clap v4.6.0
[1m[92m   Compiling[0m dugite-serialization v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m dugite-crypto v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m plotters v0.3.7
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m memmap2 v0.9.10
[1m[92m   Compiling[0m page_size v0.6.0
[1m[92m   Compiling[0m fastrand v2.4.1
[1m[92m   Compiling[0m anes v0.1.6
[1m[92m   Compiling[0m oorandom v11.1.5
[1m[92m   Compiling[0m tempfile v3.27.0
[1m[92m   Compiling[0m criterion v0.8.2
[1m[92m   Compiling[0m dugite-storage v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 15s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-b9715fc74edd732e)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 51.7s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 51.749 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [4.6395 s 4.7512 s 4.9284 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 448.3s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 448.31 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [31.962 ms 32.222 ms 32.480 ms]
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 4908.7s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 4908.7 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [349.72 ms 354.59 ms 359.71 ms]
Found 4 outliers among 100 measurements (4.00%)
  4 (4.00%) high mild

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 404.6s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 404.61 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [30.316 ms 30.523 ms 30.732 ms]

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 480.3s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 480.29 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [30.827 ms 31.005 ms 31.184 ms]

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 397.6s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 397.58 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [32.917 ms 33.219 ms 33.523 ms]

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.4s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 8.3798 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [6.5008 ms 6.6365 ms 6.7541 ms]

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.1s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 39.135 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.9510 s 4.0233 s 4.0920 s]
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.4s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 39.411 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [4.2092 s 4.3826 s 4.5769 s]
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 42.0s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 42.007 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [29.879 ms 30.413 ms 31.024 ms]
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.2s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 39.197 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [30.779 ms 31.496 ms 32.160 ms]

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.3s, or reduce sample count to 50.
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 9.2848 s (100 iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [92.625 ms 92.736 ms 92.846 ms]
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) low mild
  1 (1.00%) high mild
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.1159 s (25k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [201.13 µs 201.34 µs 201.62 µs]
Found 8 outliers among 100 measurements (8.00%)
  4 (4.00%) high mild
  4 (4.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.0905 s (1600 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [3.1832 ms 3.3412 ms 3.6349 ms]
Found 7 outliers among 100 measurements (7.00%)
  4 (4.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 117.8s, or reduce sample count to 10.
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 117.76 s (100 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [866.48 ms 868.03 ms 869.82 ms]
Found 11 outliers among 100 measurements (11.00%)
  5 (5.00%) high mild
  6 (6.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 10.0s, enable flat sampling, or reduce sample count to 40.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 9.9707 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.9460 ms 1.9691 ms 1.9918 ms]
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.6003 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [26.909 ms 27.181 ms 27.483 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.5008 s (500 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [10.996 ms 11.057 ms 11.120 ms]
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.5542 s (500 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [11.158 ms 11.214 ms 11.270 ms]

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.0684 s (146k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [34.190 µs 34.407 µs 34.676 µs]
Found 3 outliers among 100 measurements (3.00%)
  1 (1.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.0694 s (146k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [34.136 µs 34.293 µs 34.460 µs]

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 6.1210 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [15.151 ms 15.208 ms 15.268 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.3663 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [15.772 ms 15.827 ms 15.881 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 6.4571 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [319.76 µs 320.95 µs 322.14 µs]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) low severe
  3 (3.00%) low mild
  1 (1.00%) high mild
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 6.5122 s (20k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [321.33 µs 322.79 µs 324.16 µs]
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) low mild

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 8.0893 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [810.85 µs 818.53 µs 832.53 µs]
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.1665 s (800 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.2319 ms 6.2755 ms 6.3251 ms]
Found 6 outliers among 100 measurements (6.00%)
  3 (3.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2137 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.5039 ms 3.5148 ms 3.5261 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 9.1231 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [45.300 ms 45.643 ms 46.042 ms]
Found 7 outliers among 100 measurements (7.00%)
  1 (1.00%) high mild
  6 (6.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.1646 s (700 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.5602 ms 7.6511 ms 7.7459 ms]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.9s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 8.8902 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [88.874 ms 89.336 ms 89.888 ms]
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high severe

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0052 s (273k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [18.156 µs 18.184 µs 18.212 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.0582 s (167k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [35.095 µs 35.770 µs 36.329 µs]
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0624 s (268k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [18.694 µs 18.717 µs 18.741 µs]
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0370 s (242k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [23.718 µs 24.173 µs 24.550 µs]
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0651 s (268k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [18.721 µs 18.771 µs 18.842 µs]
Found 7 outliers among 100 measurements (7.00%)
  2 (2.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0919 s (252k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [23.164 µs 23.616 µs 23.994 µs]

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0044 s (429k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.545 µs 11.555 µs 11.568 µs]
Found 11 outliers among 100 measurements (11.00%)
  1 (1.00%) low mild
  5 (5.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.1741 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [42.169 µs 42.538 µs 42.988 µs]
Found 5 outliers among 100 measurements (5.00%)
  5 (5.00%) high severe

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0223 s (6270 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [799.02 µs 799.90 µs 800.77 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.1041 s (825 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.2330 ms 6.2783 ms 6.3155 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.1525 s (1485 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.4750 ms 3.5792 ms 3.7255 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 7.4667 s (165 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [45.345 ms 45.547 ms 45.720 ms]
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.2646 s (660 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.3516 ms 7.4596 ms 7.5768 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 9.7075 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [88.673 ms 89.251 ms 89.638 ms]
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 6.4213 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [28.808 ms 29.463 ms 30.538 ms]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 5.5441 s (30 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [184.08 ms 185.05 ms 186.21 ms]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.7843 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [70.664 ms 71.817 ms 72.830 ms]
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 6.4245 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [325.80 ms 328.26 ms 330.86 ms]
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.7s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 8.7132 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [155.47 ms 156.76 ms 158.93 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.7s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 5.6814 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [560.50 ms 563.68 ms 567.05 ms]

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0003 s (275k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [17.988 µs 17.997 µs 18.010 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0015 s (164k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [29.865 µs 29.893 µs 29.930 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0008 s (266k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [18.664 µs 18.671 µs 18.680 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0008 s (240k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.381 µs 20.389 µs 20.400 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0009 s (265k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [18.657 µs 18.680 µs 18.705 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0009 s (246k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [19.888 µs 19.979 µs 20.100 µs]
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0008 s (263k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [18.787 µs 18.808 µs 18.845 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0000 s (248k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [19.765 µs 19.768 µs 19.774 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0002 s (246k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [20.217 µs 20.220 µs 20.225 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0003 s (252k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.465 µs 19.473 µs 19.486 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0004 s (263k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [18.805 µs 18.828 µs 18.856 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0001 s (253k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.394 µs 19.401 µs 19.414 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.1s or enable flat sampling.
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 5.1371 s (55 iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [92.861 ms 93.371 ms 94.240 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0006 s (24k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [208.06 µs 209.49 µs 211.79 µs]
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 8.8849 s (20 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [440.05 ms 441.36 ms 442.85 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0391 s (5995 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [830.01 µs 837.95 µs 844.87 µs]
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 10.9s.
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 10.948 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [867.36 ms 869.46 ms 871.45 ms]
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0449 s (2420 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [2.0410 ms 2.0783 ms 2.1166 ms]
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.2s.
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 34.209 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [2.1632 s 2.1659 s 2.1687 s]
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.2810 s (935 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [5.6213 ms 5.6649 ms 5.7025 ms]
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 67.9s.
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 67.903 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [22.915 s 23.194 s 23.637 s]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.3884 s (550 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [9.9016 ms 9.9965 ms 10.095 ms]

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 39.8s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 39.783 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [3.9519 s 4.0410 s 4.1263 s]
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 202.7s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 202.73 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [19.978 s 20.224 s 20.479 s]
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 412.1s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 412.12 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [40.514 s 40.859 s 41.242 s]
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 1025.1s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 1025.1 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [100.96 s 101.67 s 102.43 s]

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m zeroize_derive v1.4.3
[1m[92m   Compiling[0m num-integer v0.1.46
[1m[92m   Compiling[0m minicbor v0.25.1
[1m[92m   Compiling[0m minicbor-derive v0.15.3
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m num-bigint v0.4.6
[1m[92m   Compiling[0m secp256k1-sys v0.8.2
[1m[92m   Compiling[0m zeroize v1.8.2
[1m[92m   Compiling[0m log v0.4.29
[1m[92m   Compiling[0m curve25519-dalek v4.1.3
[1m[92m   Compiling[0m radium v0.7.0
[1m[92m   Compiling[0m ed25519-dalek v2.2.0
[1m[92m   Compiling[0m pallas-crypto v1.0.0-alpha.6
[1m[92m   Compiling[0m num-rational v0.4.2
[1m[92m   Compiling[0m pallas-codec v0.33.0
[1m[92m   Compiling[0m pallas-crypto v0.33.0
[1m[92m   Compiling[0m blst v0.3.16
[1m[92m   Compiling[0m indexmap v1.9.3
[1m[92m   Compiling[0m num_cpus v1.17.0
[1m[92m   Compiling[0m peg-runtime v0.8.5
[1m[92m   Compiling[0m heck v0.5.0
[1m[92m   Compiling[0m regex-syntax v0.8.10
[1m[92m   Compiling[0m tap v1.0.1
[1m[92m   Compiling[0m wyz v0.5.1
[1m[92m   Compiling[0m dugite-primitives v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m strum_macros v0.26.4
[1m[92m   Compiling[0m peg-macros v0.8.5
[1m[92m   Compiling[0m threadpool v1.8.1
[1m[92m   Compiling[0m pallas-addresses v0.33.0
[1m[92m   Compiling[0m pallas-primitives v0.33.0
[1m[92m   Compiling[0m pallas-addresses v1.0.0-alpha.6
[1m[92m   Compiling[0m pallas-primitives v1.0.0-alpha.6
[1m[92m   Compiling[0m curve25519-dalek v3.2.0
[1m[92m   Compiling[0m curve25519-dalek v3.2.0 (https://github.com/iquerejeta/curve25519-dalek?branch=ietf03_vrf_compat_ell2#70a36f41)
[1m[92m   Compiling[0m miette-derive v5.10.0
[1m[92m   Compiling[0m funty v2.0.0
[1m[92m   Compiling[0m hashbrown v0.12.3
[1m[92m   Compiling[0m unicode-width v0.1.14
[1m[92m   Compiling[0m typed-arena v2.0.2
[1m[92m   Compiling[0m unicode-segmentation v1.13.2
[1m[92m   Compiling[0m arrayvec v0.5.2
[1m[92m   Compiling[0m pretty v0.11.3
[1m[92m   Compiling[0m peg v0.8.5
[1m[92m   Compiling[0m miette v5.10.0
[1m[92m   Compiling[0m pallas-traverse v0.33.0
[1m[92m   Compiling[0m pallas-traverse v1.0.0-alpha.6
[1m[92m   Compiling[0m bitvec v1.0.1
[1m[92m   Compiling[0m vrf_dalek v0.1.0 (https://github.com/input-output-hk/vrf?rev=03ac038e9b92c754ebbcb71824866d93f25e27f3#03ac038e)
[1m[92m   Compiling[0m secp256k1 v0.26.0
[1m[92m   Compiling[0m strum v0.26.3
[1m[92m   Compiling[0m regex-automata v0.4.14
[1m[92m   Compiling[0m itertools v0.10.5
[1m[92m   Compiling[0m wait-timeout v0.2.1
[1m[92m   Compiling[0m fs2 v0.4.3
[1m[92m   Compiling[0m hamming v0.1.3
[1m[92m   Compiling[0m bit-vec v0.8.0
[1m[92m   Compiling[0m fnv v1.0.7
[1m[92m   Compiling[0m quick-error v1.2.3
[1m[92m   Compiling[0m rusty-fork v0.3.1
[1m[92m   Compiling[0m bit-set v0.8.0
[1m[92m   Compiling[0m uplc v1.1.21 (https://github.com/aiken-lang/aiken.git?rev=6806d52211fbb469ae13aa0e0290aeb6b9b3e8cf#6806d522)
[1m[92m   Compiling[0m dugite-lsm v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
[1m[92m   Compiling[0m regex v1.12.3
[1m[92m   Compiling[0m dugite-serialization v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m dugite-crypto v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m rand_xorshift v0.4.0
[1m[92m   Compiling[0m bincode v1.3.3
[1m[92m   Compiling[0m unarray v0.1.4
[1m[92m   Compiling[0m proptest v1.11.0
[1m[92m   Compiling[0m criterion v0.8.2
[1m[92m   Compiling[0m dugite-ledger v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 1m 10s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-49c5e074f7e9cfa5)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.5s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 26.548 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.5793 s 2.5900 s 2.6011 s]

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.9788 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [582.80 µs 585.50 µs 588.44 µs]
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) high mild
  4 (4.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 5.1064 s (15k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [342.61 µs 343.19 µs 343.68 µs]
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 7.0536 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [462.94 µs 463.49 µs 464.08 µs]
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) high mild
  4 (4.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 5.1186 s (15k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [334.48 µs 335.10 µs 335.71 µs]
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) high mild
  4 (4.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 50.2s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 50.157 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.6876 s 2.7043 s 2.7218 s]

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.7s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 26.671 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [303.82 ms 308.58 ms 313.59 ms]
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.4s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 26.376 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [277.14 ms 281.46 ms 286.54 ms]

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 33.7s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 33.674 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.3504 s 3.3680 s 3.3867 s]
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.0s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 34.018 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [114.61 ms 123.38 ms 132.90 ms]

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 30.8s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 30.798 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [293.51 ms 295.20 ms 296.91 ms]

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.6s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 35.599 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [517.41 ms 521.90 ms 527.02 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.7s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 25.732 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.5718 s 2.5914 s 2.6142 s]
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.0s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 26.043 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.5685 s 2.5756 s 2.5828 s]
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.8s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 25.772 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.5464 s 2.5543 s 2.5631 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.2s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 25.215 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.5208 s 2.5324 s 2.5438 s]
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.4s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 25.438 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.5200 s 2.5277 s 2.5355 s]

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 6.8193 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [438.08 µs 441.06 µs 445.03 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 6.7893 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [437.81 µs 439.78 µs 441.87 µs]
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 6.8177 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [437.99 µs 439.81 µs 441.71 µs]
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 6.7682 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [435.32 µs 437.38 µs 439.61 µs]
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 6.8331 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [439.74 µs 441.75 µs 443.87 µs]

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 6.3338 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [209.10 ms 210.48 ms 212.00 ms]
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.9s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 11.906 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.1918 s 1.1983 s 1.2047 s]
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.4s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 25.440 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.5299 s 2.5407 s 2.5518 s]

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0062 s (14k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [365.85 µs 366.34 µs 366.82 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) low mild
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0063 s (12k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [420.61 µs 421.27 µs 421.78 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low severe
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0209 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [449.17 µs 452.70 µs 457.31 µs]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high mild

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 6.3490 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [18.440 ms 19.175 ms 19.805 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 12.1s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 12.122 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [127.10 ms 130.95 ms 134.40 ms]
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 25.7s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 25.662 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [271.39 ms 275.71 ms 281.70 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.2190 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [28.198 ms 28.227 ms 28.279 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.5s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 7.5360 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [136.46 ms 136.55 ms 136.71 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 5.6708 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [283.06 ms 285.28 ms 289.05 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 157.6s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 157.60 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [15.512 s 15.569 s 15.635 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 341.4s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 341.35 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [32.947 s 33.071 s 33.183 s]

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0420 s (3905 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.2921 ms 1.2973 ms 1.3067 ms]
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high severe
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0043 s (3025 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.6432 ms 1.6465 ms 1.6483 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 16.1s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 16.058 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.6083 s 1.6198 s 1.6386 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 31.3s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 31.294 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.1365 s 3.1639 s 3.2092 s]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

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
[1m[92m   Compiling[0m rand v0.9.4
[1m[92m   Compiling[0m byteorder v1.5.0
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m zerocopy-derive v0.8.48
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m dugite-lsm v1.1.0-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
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
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/dugite_lsm-880b5a55f0b6c001)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 7.72s

[1m[92m   Doc-tests[0m dugite_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
