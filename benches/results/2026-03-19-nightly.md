# Nightly Benchmark Results — 2026-03-19

Machine: GitHub Actions ubuntu-latest
Branch: main (a90f7ba)

## Storage Benchmarks
```
test chaindb/sequential_insert/10k_20kb ... bench:  4203152777 ns/iter (+/- 360445880)

test chaindb/random_read/by_hash/10000blks ... bench:    53347931 ns/iter (+/- 1402160)
test chaindb/random_read/by_hash/100000blks ... bench:   527397488 ns/iter (+/- 7331714)

test chaindb/tip_query ... bench:    54313192 ns/iter (+/- 2141189)

test chaindb/has_block ... bench:    54667131 ns/iter (+/- 2427149)

test chaindb/slot_range_100 ... bench:    56082167 ns/iter (+/- 1597072)

test chaindb/flush_to_immutable/k_2160_blocks_20kb/2160 ... bench:     8546734 ns/iter (+/- 355032)

test chaindb/profile_comparison/insert_10k_20kb/in_memory ... bench:  3738972543 ns/iter (+/- 69810046)
test chaindb/profile_comparison/insert_10k_20kb/mmap ... bench:  3724859160 ns/iter (+/- 100143335)
test chaindb/profile_comparison/read_500/in_memory ... bench:    53907196 ns/iter (+/- 3509593)
test chaindb/profile_comparison/read_500/mmap ... bench:    54569076 ns/iter (+/- 1516533)

test immutabledb/open/in_memory/10000 ... bench:      197767 ns/iter (+/- 1166)
test immutabledb/open/mmap_cached/10000 ... bench:      200888 ns/iter (+/- 1320)
test immutabledb/open/mmap_cold_rebuild/10000 ... bench:     3179553 ns/iter (+/- 187125)
test immutabledb/open/in_memory/100000 ... bench:     1769338 ns/iter (+/- 116230)
test immutabledb/open/mmap_cached/100000 ... bench:     1721266 ns/iter (+/- 103176)
test immutabledb/open/mmap_cold_rebuild/100000 ... bench:    28148105 ns/iter (+/- 610237)

test immutabledb/lookup/in_memory/10000 ... bench:    13255649 ns/iter (+/- 448277)
test immutabledb/lookup/mmap/10000 ... bench:    12630385 ns/iter (+/- 151527)

test immutabledb/has_block/in_memory ... bench:       34924 ns/iter (+/- 55)
test immutabledb/has_block/mmap ... bench:       34907 ns/iter (+/- 127)

test immutabledb/append/1k_blocks_20kb/in_memory ... bench:    19761934 ns/iter (+/- 233013)
test immutabledb/append/1k_blocks_20kb/mmap ... bench:    20509934 ns/iter (+/- 572326)

test immutabledb/slot_range/range_100/in_memory ... bench:      353578 ns/iter (+/- 5685)
test immutabledb/slot_range/range_100/mmap ... bench:      351174 ns/iter (+/- 7123)

test block_index/insert/in_memory/10000 ... bench:      792566 ns/iter (+/- 3561)
test block_index/insert/mmap/10000 ... bench:     6403505 ns/iter (+/- 320441)
test block_index/insert/in_memory/50000 ... bench:     3445416 ns/iter (+/- 19903)
test block_index/insert/mmap/50000 ... bench:    47743853 ns/iter (+/- 2315974)
test block_index/insert/in_memory/100000 ... bench:     7029760 ns/iter (+/- 287098)
test block_index/insert/mmap/100000 ... bench:    95317243 ns/iter (+/- 1725616)

test block_index/lookup/in_memory/10000 ... bench:       15482 ns/iter (+/- 61)
test block_index/lookup/mmap/10000 ... bench:       30220 ns/iter (+/- 82)
test block_index/lookup/in_memory/50000 ... bench:       15871 ns/iter (+/- 131)
test block_index/lookup/mmap/50000 ... bench:       20539 ns/iter (+/- 112)
test block_index/lookup/in_memory/100000 ... bench:       16151 ns/iter (+/- 29)
test block_index/lookup/mmap/100000 ... bench:       20029 ns/iter (+/- 56)

test block_index/contains_miss/in_memory ... bench:       11125 ns/iter (+/- 67)
test block_index/contains_miss/mmap ... bench:       41837 ns/iter (+/- 113)

test scaling/block_index_insert/in_memory/10000 ... bench:      790063 ns/iter (+/- 3719)
test scaling/block_index_insert/mmap/10000 ... bench:     6479473 ns/iter (+/- 71665)
test scaling/block_index_insert/in_memory/50000 ... bench:     3508497 ns/iter (+/- 65929)
test scaling/block_index_insert/mmap/50000 ... bench:    48610678 ns/iter (+/- 759642)
test scaling/block_index_insert/in_memory/100000 ... bench:     7204119 ns/iter (+/- 42268)
test scaling/block_index_insert/mmap/100000 ... bench:    98711005 ns/iter (+/- 1934170)
test scaling/block_index_insert/in_memory/250000 ... bench:    28427806 ns/iter (+/- 785465)
test scaling/block_index_insert/mmap/250000 ... bench:   208637234 ns/iter (+/- 2732962)
test scaling/block_index_insert/in_memory/500000 ... bench:    68378086 ns/iter (+/- 892500)
test scaling/block_index_insert/mmap/500000 ... bench:   413003169 ns/iter (+/- 3469679)
test scaling/block_index_insert/in_memory/1000000 ... bench:   156133444 ns/iter (+/- 5678944)
test scaling/block_index_insert/mmap/1000000 ... bench:   861360758 ns/iter (+/- 13786083)

test scaling/block_index_lookup/in_memory/10000 ... bench:       15390 ns/iter (+/- 10)
test scaling/block_index_lookup/mmap/10000 ... bench:       30242 ns/iter (+/- 21)
test scaling/block_index_lookup/in_memory/50000 ... bench:       15971 ns/iter (+/- 13)
test scaling/block_index_lookup/mmap/50000 ... bench:       20465 ns/iter (+/- 9)
test scaling/block_index_lookup/in_memory/100000 ... bench:       16280 ns/iter (+/- 77)
test scaling/block_index_lookup/mmap/100000 ... bench:       19955 ns/iter (+/- 25)
test scaling/block_index_lookup/in_memory/250000 ... bench:       16096 ns/iter (+/- 14)
test scaling/block_index_lookup/mmap/250000 ... bench:       19803 ns/iter (+/- 8)
test scaling/block_index_lookup/in_memory/500000 ... bench:       15960 ns/iter (+/- 14)
test scaling/block_index_lookup/mmap/500000 ... bench:       19524 ns/iter (+/- 8)
test scaling/block_index_lookup/in_memory/1000000 ... bench:       16173 ns/iter (+/- 36)
test scaling/block_index_lookup/mmap/1000000 ... bench:       19455 ns/iter (+/- 14)

test scaling/immutabledb_open/in_memory/10000 ... bench:      196160 ns/iter (+/- 686)
test scaling/immutabledb_open/mmap_cached/10000 ... bench:      196909 ns/iter (+/- 633)
test scaling/immutabledb_open/in_memory/50000 ... bench:      833942 ns/iter (+/- 3375)
test scaling/immutabledb_open/mmap_cached/50000 ... bench:      829820 ns/iter (+/- 3556)
test scaling/immutabledb_open/in_memory/100000 ... bench:     1632303 ns/iter (+/- 28847)
test scaling/immutabledb_open/mmap_cached/100000 ... bench:     1656707 ns/iter (+/- 77253)
test scaling/immutabledb_open/in_memory/250000 ... bench:     5010917 ns/iter (+/- 208577)
test scaling/immutabledb_open/mmap_cached/250000 ... bench:     5010064 ns/iter (+/- 44982)
test scaling/immutabledb_open/in_memory/500000 ... bench:    17280427 ns/iter (+/- 151547)
test scaling/immutabledb_open/mmap_cached/500000 ... bench:    17398062 ns/iter (+/- 54606)

test scaling/chaindb_insert/default_20kb/10000 ... bench:  3718928150 ns/iter (+/- 109324382)
test scaling/chaindb_insert/default_20kb/50000 ... bench: 19006010244 ns/iter (+/- 210404351)
test scaling/chaindb_insert/default_20kb/100000 ... bench: 37580350375 ns/iter (+/- 349319774)
test scaling/chaindb_insert/default_20kb/250000 ... bench: 93782472416 ns/iter (+/- 817918351)

```

## UTxO Benchmarks
```
test utxo_store/insert/default/1000000 ... bench:  2928828674 ns/iter (+/- 38177933)

test utxo_store/lookup/hit/1000000 ... bench:      574613 ns/iter (+/- 10791)
test utxo_store/lookup/miss/1000000 ... bench:      337493 ns/iter (+/- 2195)

test utxo_store/contains/hit ... bench:      460707 ns/iter (+/- 5662)
test utxo_store/contains/miss ... bench:      329158 ns/iter (+/- 3502)

test utxo_store/remove/sequential/1000000 ... bench:  2930221990 ns/iter (+/- 34573599)

test utxo_store/apply_tx/block_50tx_3in_2out ... bench:   275915343 ns/iter (+/- 14951851)
test utxo_store/apply_tx/block_300tx_2in_2out ... bench:   259492776 ns/iter (+/- 12752513)

test utxo_store/multi_asset/insert_mixed_30pct/1000000 ... bench:  3799151700 ns/iter (+/- 42562601)
test utxo_store/multi_asset/lookup_mixed_30pct/1000000 ... bench:   164549376 ns/iter (+/- 13246484)

test utxo_store/total_lovelace/scan/1000000 ... bench:   273672304 ns/iter (+/- 2722515)

test utxo_store/rebuild_address_index/rebuild/1000000 ... bench:   592583867 ns/iter (+/- 25206279)

test utxo_store/insert_configs/low_8gb/1000000 ... bench:  2849798814 ns/iter (+/- 50886140)
test utxo_store/insert_configs/mid_16gb/1000000 ... bench:  2832962765 ns/iter (+/- 28670865)
test utxo_store/insert_configs/high_32gb/1000000 ... bench:  2889972680 ns/iter (+/- 57777155)
test utxo_store/insert_configs/high_bloom_16gb/1000000 ... bench:  2897173390 ns/iter (+/- 48072511)
test utxo_store/insert_configs/legacy_small/1000000 ... bench:  2851469768 ns/iter (+/- 23559990)

test utxo_store/lookup_configs/low_8gb/1000000 ... bench:      462347 ns/iter (+/- 12355)
test utxo_store/lookup_configs/mid_16gb/1000000 ... bench:      464824 ns/iter (+/- 3365)
test utxo_store/lookup_configs/high_32gb/1000000 ... bench:      461462 ns/iter (+/- 8752)
test utxo_store/lookup_configs/high_bloom_16gb/1000000 ... bench:      460401 ns/iter (+/- 2780)
test utxo_store/lookup_configs/legacy_small/1000000 ... bench:      463333 ns/iter (+/- 1957)

test utxo_scaling/insert/default/100000 ... bench:   241753368 ns/iter (+/- 3528496)
test utxo_scaling/insert/default/500000 ... bench:  1342515760 ns/iter (+/- 12970749)
test utxo_scaling/insert/default/1000000 ... bench:  2824335681 ns/iter (+/- 40182187)

test utxo_scaling/lookup/hit/100000 ... bench:      382444 ns/iter (+/- 1117)
test utxo_scaling/lookup/hit/500000 ... bench:      428699 ns/iter (+/- 5154)
test utxo_scaling/lookup/hit/1000000 ... bench:      461420 ns/iter (+/- 922)

test utxo_scaling/apply_tx/block_50tx_3in_2out/100000 ... bench:    12184226 ns/iter (+/- 631045)
test utxo_scaling/apply_tx/block_50tx_3in_2out/500000 ... bench:   124215234 ns/iter (+/- 4758564)
test utxo_scaling/apply_tx/block_50tx_3in_2out/1000000 ... bench:   260452532 ns/iter (+/- 9206711)

test utxo_scaling/total_lovelace/scan/100000 ... bench:    28451495 ns/iter (+/- 731807)
test utxo_scaling/total_lovelace/scan/500000 ... bench:   134588278 ns/iter (+/- 1792491)
test utxo_scaling/total_lovelace/scan/1000000 ... bench:   283857061 ns/iter (+/- 2601282)

test utxo_large_scale/insert/default/5000000 ... bench: 17109404266 ns/iter (+/- 137146490)
test utxo_large_scale/insert/default/10000000 ... bench: 36727180987 ns/iter (+/- 439689239)

test utxo_large_scale/lookup/hit/5000000 ... bench:     1249951 ns/iter (+/- 7241)
test utxo_large_scale/lookup/hit/10000000 ... bench:     1645622 ns/iter (+/- 18355)

test utxo_large_scale/total_lovelace/scan/5000000 ... bench:  1684579758 ns/iter (+/- 31586783)
test utxo_large_scale/total_lovelace/scan/10000000 ... bench:  3306569272 ns/iter (+/- 23843452)

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
[1m[92m   Compiling[0m zerocopy-derive v0.8.42
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m torsten-lsm v0.4.5-alpha (/home/runner/work/torsten/torsten/crates/torsten-lsm)
[1m[92m   Compiling[0m zerocopy v0.8.42
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 29.14s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-6cc7f3026238ee1e)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.63s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
