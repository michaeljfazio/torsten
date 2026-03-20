# Nightly Benchmark Results — 2026-03-20

Machine: GitHub Actions ubuntu-latest
Branch: main (c73adaa)

## Storage Benchmarks
```
test chaindb/sequential_insert/10k_20kb ... bench:  7156536482 ns/iter (+/- 865814244)

test chaindb/random_read/by_hash/10000blks ... bench:    53644387 ns/iter (+/- 902725)
test chaindb/random_read/by_hash/100000blks ... bench:   532166329 ns/iter (+/- 6819181)

test chaindb/tip_query ... bench:    54955513 ns/iter (+/- 2245565)

test chaindb/has_block ... bench:    54126217 ns/iter (+/- 1192064)

test chaindb/slot_range_100 ... bench:    56742184 ns/iter (+/- 1146068)

test chaindb/flush_to_immutable/k_2160_blocks_20kb/2160 ... bench:    11787413 ns/iter (+/- 432600)

test chaindb/profile_comparison/insert_10k_20kb/in_memory ... bench:  8236898756 ns/iter (+/- 452781156)
test chaindb/profile_comparison/insert_10k_20kb/mmap ... bench:  7251055472 ns/iter (+/- 1067285240)
test chaindb/profile_comparison/read_500/in_memory ... bench:    54604965 ns/iter (+/- 1146598)
test chaindb/profile_comparison/read_500/mmap ... bench:    55834968 ns/iter (+/- 1761728)

test immutabledb/open/in_memory/10000 ... bench:      214924 ns/iter (+/- 3288)
test immutabledb/open/mmap_cached/10000 ... bench:      215650 ns/iter (+/- 1033)
test immutabledb/open/mmap_cold_rebuild/10000 ... bench:     3453124 ns/iter (+/- 240859)
test immutabledb/open/in_memory/100000 ... bench:     1701778 ns/iter (+/- 84070)
test immutabledb/open/mmap_cached/100000 ... bench:     1719207 ns/iter (+/- 92365)
test immutabledb/open/mmap_cold_rebuild/100000 ... bench:    27430064 ns/iter (+/- 935869)

test immutabledb/lookup/in_memory/10000 ... bench:    13111649 ns/iter (+/- 255369)
test immutabledb/lookup/mmap/10000 ... bench:    12843662 ns/iter (+/- 262135)

test immutabledb/has_block/in_memory ... bench:       33322 ns/iter (+/- 71)
test immutabledb/has_block/mmap ... bench:       33331 ns/iter (+/- 140)

test immutabledb/append/1k_blocks_20kb/in_memory ... bench:    20075430 ns/iter (+/- 207267)
test immutabledb/append/1k_blocks_20kb/mmap ... bench:    20984281 ns/iter (+/- 303584)

test immutabledb/slot_range/range_100/in_memory ... bench:      362812 ns/iter (+/- 6863)
test immutabledb/slot_range/range_100/mmap ... bench:      372665 ns/iter (+/- 7630)

test block_index/insert/in_memory/10000 ... bench:      791237 ns/iter (+/- 12777)
test block_index/insert/mmap/10000 ... bench:     6734121 ns/iter (+/- 284306)
test block_index/insert/in_memory/50000 ... bench:     3482626 ns/iter (+/- 116143)
test block_index/insert/mmap/50000 ... bench:    48628681 ns/iter (+/- 1050360)
test block_index/insert/in_memory/100000 ... bench:     7333799 ns/iter (+/- 624872)
test block_index/insert/mmap/100000 ... bench:   100403133 ns/iter (+/- 1214939)

test block_index/lookup/in_memory/10000 ... bench:       15604 ns/iter (+/- 37)
test block_index/lookup/mmap/10000 ... bench:       30283 ns/iter (+/- 65)
test block_index/lookup/in_memory/50000 ... bench:       15904 ns/iter (+/- 76)
test block_index/lookup/mmap/50000 ... bench:       20454 ns/iter (+/- 90)
test block_index/lookup/in_memory/100000 ... bench:       15919 ns/iter (+/- 167)
test block_index/lookup/mmap/100000 ... bench:       19942 ns/iter (+/- 468)

test block_index/contains_miss/in_memory ... bench:       11484 ns/iter (+/- 173)
test block_index/contains_miss/mmap ... bench:       41658 ns/iter (+/- 155)

test scaling/block_index_insert/in_memory/10000 ... bench:      782722 ns/iter (+/- 1367)
test scaling/block_index_insert/mmap/10000 ... bench:     6655031 ns/iter (+/- 68334)
test scaling/block_index_insert/in_memory/50000 ... bench:     3598887 ns/iter (+/- 136797)
test scaling/block_index_insert/mmap/50000 ... bench:    48888512 ns/iter (+/- 382134)
test scaling/block_index_insert/in_memory/100000 ... bench:     8125102 ns/iter (+/- 436928)
test scaling/block_index_insert/mmap/100000 ... bench:    99386277 ns/iter (+/- 1428805)
test scaling/block_index_insert/in_memory/250000 ... bench:    32790299 ns/iter (+/- 241830)
test scaling/block_index_insert/mmap/250000 ... bench:   212381147 ns/iter (+/- 1484029)
test scaling/block_index_insert/in_memory/500000 ... bench:    76514126 ns/iter (+/- 731386)
test scaling/block_index_insert/mmap/500000 ... bench:   434702590 ns/iter (+/- 3337900)
test scaling/block_index_insert/in_memory/1000000 ... bench:   160945118 ns/iter (+/- 1120978)
test scaling/block_index_insert/mmap/1000000 ... bench:   875004716 ns/iter (+/- 4341737)

test scaling/block_index_lookup/in_memory/10000 ... bench:       15510 ns/iter (+/- 11)
test scaling/block_index_lookup/mmap/10000 ... bench:       29703 ns/iter (+/- 26)
test scaling/block_index_lookup/in_memory/50000 ... bench:       16496 ns/iter (+/- 40)
test scaling/block_index_lookup/mmap/50000 ... bench:       20199 ns/iter (+/- 7)
test scaling/block_index_lookup/in_memory/100000 ... bench:       16253 ns/iter (+/- 52)
test scaling/block_index_lookup/mmap/100000 ... bench:       19703 ns/iter (+/- 33)
test scaling/block_index_lookup/in_memory/250000 ... bench:       16112 ns/iter (+/- 24)
test scaling/block_index_lookup/mmap/250000 ... bench:       19609 ns/iter (+/- 29)
test scaling/block_index_lookup/in_memory/500000 ... bench:       17602 ns/iter (+/- 22)
test scaling/block_index_lookup/mmap/500000 ... bench:       19298 ns/iter (+/- 15)
test scaling/block_index_lookup/in_memory/1000000 ... bench:       16009 ns/iter (+/- 37)
test scaling/block_index_lookup/mmap/1000000 ... bench:       19233 ns/iter (+/- 7)

test scaling/immutabledb_open/in_memory/10000 ... bench:      207281 ns/iter (+/- 316)
test scaling/immutabledb_open/mmap_cached/10000 ... bench:      215288 ns/iter (+/- 1077)
test scaling/immutabledb_open/in_memory/50000 ... bench:      857364 ns/iter (+/- 7362)
test scaling/immutabledb_open/mmap_cached/50000 ... bench:      861958 ns/iter (+/- 19648)
test scaling/immutabledb_open/in_memory/100000 ... bench:     1913923 ns/iter (+/- 34407)
test scaling/immutabledb_open/mmap_cached/100000 ... bench:     1917166 ns/iter (+/- 40748)
test scaling/immutabledb_open/in_memory/250000 ... bench:     4863553 ns/iter (+/- 70423)
test scaling/immutabledb_open/mmap_cached/250000 ... bench:     4935414 ns/iter (+/- 47970)
test scaling/immutabledb_open/in_memory/500000 ... bench:    18873849 ns/iter (+/- 220272)
test scaling/immutabledb_open/mmap_cached/500000 ... bench:    18672961 ns/iter (+/- 571056)

test scaling/chaindb_insert/default_20kb/10000 ... bench:  7338459281 ns/iter (+/- 864583767)
test scaling/chaindb_insert/default_20kb/50000 ... bench: 30569890145 ns/iter (+/- 3390272228)
test scaling/chaindb_insert/default_20kb/100000 ... bench: 67233283068 ns/iter (+/- 7509330957)
test scaling/chaindb_insert/default_20kb/250000 ... bench: 149847941130 ns/iter (+/- 17084478419)

```

## UTxO Benchmarks
```
test utxo_store/insert/default/1000000 ... bench:  2956226277 ns/iter (+/- 21663321)

test utxo_store/lookup/hit/1000000 ... bench:      516826 ns/iter (+/- 5439)
test utxo_store/lookup/miss/1000000 ... bench:      310628 ns/iter (+/- 2028)

test utxo_store/contains/hit ... bench:      386125 ns/iter (+/- 2634)
test utxo_store/contains/miss ... bench:      303082 ns/iter (+/- 3604)

test utxo_store/remove/sequential/1000000 ... bench:  2959986664 ns/iter (+/- 10540693)

test utxo_store/apply_tx/block_50tx_3in_2out ... bench:   277809458 ns/iter (+/- 16516085)
test utxo_store/apply_tx/block_300tx_2in_2out ... bench:   276917824 ns/iter (+/- 9199277)

test utxo_store/multi_asset/insert_mixed_30pct/1000000 ... bench:  3750918694 ns/iter (+/- 20211512)
test utxo_store/multi_asset/lookup_mixed_30pct/1000000 ... bench:   155537234 ns/iter (+/- 16242520)

test utxo_store/total_lovelace/scan/1000000 ... bench:   274014483 ns/iter (+/- 2629498)

test utxo_store/rebuild_address_index/rebuild/1000000 ... bench:   600451676 ns/iter (+/- 4521184)

test utxo_store/insert_configs/low_8gb/1000000 ... bench:  2861785799 ns/iter (+/- 11508226)
test utxo_store/insert_configs/mid_16gb/1000000 ... bench:  2855851090 ns/iter (+/- 23009074)
test utxo_store/insert_configs/high_32gb/1000000 ... bench:  2895888674 ns/iter (+/- 22019477)
test utxo_store/insert_configs/high_bloom_16gb/1000000 ... bench:  2887683909 ns/iter (+/- 21935113)
test utxo_store/insert_configs/legacy_small/1000000 ... bench:  2872574666 ns/iter (+/- 23946085)

test utxo_store/lookup_configs/low_8gb/1000000 ... bench:      478868 ns/iter (+/- 2645)
test utxo_store/lookup_configs/mid_16gb/1000000 ... bench:      481836 ns/iter (+/- 3726)
test utxo_store/lookup_configs/high_32gb/1000000 ... bench:      478337 ns/iter (+/- 1825)
test utxo_store/lookup_configs/high_bloom_16gb/1000000 ... bench:      482123 ns/iter (+/- 2302)
test utxo_store/lookup_configs/legacy_small/1000000 ... bench:      480464 ns/iter (+/- 2104)

test utxo_scaling/insert/default/100000 ... bench:   244397531 ns/iter (+/- 2303798)
test utxo_scaling/insert/default/500000 ... bench:  1327572380 ns/iter (+/- 12242894)
test utxo_scaling/insert/default/1000000 ... bench:  2860497558 ns/iter (+/- 11507303)

test utxo_scaling/lookup/hit/100000 ... bench:      379882 ns/iter (+/- 1143)
test utxo_scaling/lookup/hit/500000 ... bench:      428362 ns/iter (+/- 798)
test utxo_scaling/lookup/hit/1000000 ... bench:      462819 ns/iter (+/- 997)

test utxo_scaling/apply_tx/block_50tx_3in_2out/100000 ... bench:    17319571 ns/iter (+/- 1882127)
test utxo_scaling/apply_tx/block_50tx_3in_2out/500000 ... bench:   129378458 ns/iter (+/- 9803862)
test utxo_scaling/apply_tx/block_50tx_3in_2out/1000000 ... bench:   279517867 ns/iter (+/- 5203436)

test utxo_scaling/total_lovelace/scan/100000 ... bench:    27825064 ns/iter (+/- 124179)
test utxo_scaling/total_lovelace/scan/500000 ... bench:   133878623 ns/iter (+/- 930872)
test utxo_scaling/total_lovelace/scan/1000000 ... bench:   298488613 ns/iter (+/- 1763888)

test utxo_large_scale/insert/default/5000000 ... bench: 17230671370 ns/iter (+/- 105593515)
test utxo_large_scale/insert/default/10000000 ... bench: 37497188799 ns/iter (+/- 456823888)

test utxo_large_scale/lookup/hit/5000000 ... bench:     1377762 ns/iter (+/- 20395)
test utxo_large_scale/lookup/hit/10000000 ... bench:     1663568 ns/iter (+/- 62210)

test utxo_large_scale/total_lovelace/scan/5000000 ... bench:  1791948192 ns/iter (+/- 5762493)
test utxo_large_scale/total_lovelace/scan/10000000 ... bench:  3543832877 ns/iter (+/- 43503476)

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
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 29.79s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-a6e2ac7d854bf680)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.94s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
