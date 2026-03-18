# Nightly Benchmark Results — 2026-03-18

Machine: GitHub Actions ubuntu-latest
Branch: main (9d56f5b)

## Storage Benchmarks
```
test chaindb/sequential_insert/10k_20kb ... bench:  3884540750 ns/iter (+/- 449489360)

test chaindb/random_read/by_hash/10000blks ... bench:    54132910 ns/iter (+/- 946258)
test chaindb/random_read/by_hash/100000blks ... bench:   523911327 ns/iter (+/- 2963376)

test chaindb/tip_query ... bench:    55032298 ns/iter (+/- 1587856)

test chaindb/has_block ... bench:    55541525 ns/iter (+/- 1719410)

test chaindb/slot_range_100 ... bench:    57140324 ns/iter (+/- 1601510)

test chaindb/flush_to_immutable/k_2160_blocks_20kb/2160 ... bench:    11407572 ns/iter (+/- 633416)

test chaindb/profile_comparison/insert_10k_20kb/in_memory ... bench:  3539459538 ns/iter (+/- 110609824)
test chaindb/profile_comparison/insert_10k_20kb/mmap ... bench:  3522313188 ns/iter (+/- 32246548)
test chaindb/profile_comparison/read_500/in_memory ... bench:    54533730 ns/iter (+/- 1671600)
test chaindb/profile_comparison/read_500/mmap ... bench:    55743708 ns/iter (+/- 1544733)

test immutabledb/open/in_memory/10000 ... bench:      193710 ns/iter (+/- 1544)
test immutabledb/open/mmap_cached/10000 ... bench:      195525 ns/iter (+/- 808)
test immutabledb/open/mmap_cold_rebuild/10000 ... bench:     3066225 ns/iter (+/- 113525)
test immutabledb/open/in_memory/100000 ... bench:     1577948 ns/iter (+/- 47619)
test immutabledb/open/mmap_cached/100000 ... bench:     1569259 ns/iter (+/- 39549)
test immutabledb/open/mmap_cold_rebuild/100000 ... bench:    27100858 ns/iter (+/- 634926)

test immutabledb/lookup/in_memory/10000 ... bench:    13034174 ns/iter (+/- 155955)
test immutabledb/lookup/mmap/10000 ... bench:    13042663 ns/iter (+/- 193223)

test immutabledb/has_block/in_memory ... bench:       33250 ns/iter (+/- 170)
test immutabledb/has_block/mmap ... bench:       33247 ns/iter (+/- 51)

test immutabledb/append/1k_blocks_20kb/in_memory ... bench:    19833413 ns/iter (+/- 225188)
test immutabledb/append/1k_blocks_20kb/mmap ... bench:    20344647 ns/iter (+/- 217272)

test immutabledb/slot_range/range_100/in_memory ... bench:      355102 ns/iter (+/- 6501)
test immutabledb/slot_range/range_100/mmap ... bench:      349222 ns/iter (+/- 6089)

test block_index/insert/in_memory/10000 ... bench:      798549 ns/iter (+/- 5142)
test block_index/insert/mmap/10000 ... bench:     6510855 ns/iter (+/- 352415)
test block_index/insert/in_memory/50000 ... bench:     3571623 ns/iter (+/- 13783)
test block_index/insert/mmap/50000 ... bench:    47265460 ns/iter (+/- 753632)
test block_index/insert/in_memory/100000 ... bench:     7247459 ns/iter (+/- 51921)
test block_index/insert/mmap/100000 ... bench:    95719938 ns/iter (+/- 1596001)

test block_index/lookup/in_memory/10000 ... bench:       15563 ns/iter (+/- 106)
test block_index/lookup/mmap/10000 ... bench:       30169 ns/iter (+/- 56)
test block_index/lookup/in_memory/50000 ... bench:       16047 ns/iter (+/- 129)
test block_index/lookup/mmap/50000 ... bench:       20503 ns/iter (+/- 34)
test block_index/lookup/in_memory/100000 ... bench:       16192 ns/iter (+/- 64)
test block_index/lookup/mmap/100000 ... bench:       20000 ns/iter (+/- 85)

test block_index/contains_miss/in_memory ... bench:       11409 ns/iter (+/- 29)
test block_index/contains_miss/mmap ... bench:       41580 ns/iter (+/- 45)

test scaling/block_index_insert/in_memory/10000 ... bench:      791569 ns/iter (+/- 1036)
test scaling/block_index_insert/mmap/10000 ... bench:     6575250 ns/iter (+/- 84640)
test scaling/block_index_insert/in_memory/50000 ... bench:     3483335 ns/iter (+/- 111969)
test scaling/block_index_insert/mmap/50000 ... bench:    47590996 ns/iter (+/- 238483)
test scaling/block_index_insert/in_memory/100000 ... bench:     7070750 ns/iter (+/- 39265)
test scaling/block_index_insert/mmap/100000 ... bench:    92915941 ns/iter (+/- 209987)
test scaling/block_index_insert/in_memory/250000 ... bench:    28407630 ns/iter (+/- 437443)
test scaling/block_index_insert/mmap/250000 ... bench:   196002474 ns/iter (+/- 1489819)
test scaling/block_index_insert/in_memory/500000 ... bench:    65690595 ns/iter (+/- 714851)
test scaling/block_index_insert/mmap/500000 ... bench:   408923809 ns/iter (+/- 2252786)
test scaling/block_index_insert/in_memory/1000000 ... bench:   146825178 ns/iter (+/- 1806502)
test scaling/block_index_insert/mmap/1000000 ... bench:   833368186 ns/iter (+/- 7652042)

test scaling/block_index_lookup/in_memory/10000 ... bench:       15647 ns/iter (+/- 238)
test scaling/block_index_lookup/mmap/10000 ... bench:       29572 ns/iter (+/- 33)
test scaling/block_index_lookup/in_memory/50000 ... bench:       16162 ns/iter (+/- 142)
test scaling/block_index_lookup/mmap/50000 ... bench:       20206 ns/iter (+/- 18)
test scaling/block_index_lookup/in_memory/100000 ... bench:       16208 ns/iter (+/- 10)
test scaling/block_index_lookup/mmap/100000 ... bench:       19668 ns/iter (+/- 9)
test scaling/block_index_lookup/in_memory/250000 ... bench:       17405 ns/iter (+/- 25)
test scaling/block_index_lookup/mmap/250000 ... bench:       19551 ns/iter (+/- 52)
test scaling/block_index_lookup/in_memory/500000 ... bench:       16231 ns/iter (+/- 50)
test scaling/block_index_lookup/mmap/500000 ... bench:       19311 ns/iter (+/- 27)
test scaling/block_index_lookup/in_memory/1000000 ... bench:       16415 ns/iter (+/- 54)
test scaling/block_index_lookup/mmap/1000000 ... bench:       19219 ns/iter (+/- 9)

test scaling/immutabledb_open/in_memory/10000 ... bench:      194501 ns/iter (+/- 308)
test scaling/immutabledb_open/mmap_cached/10000 ... bench:      194610 ns/iter (+/- 376)
test scaling/immutabledb_open/in_memory/50000 ... bench:      769598 ns/iter (+/- 1259)
test scaling/immutabledb_open/mmap_cached/50000 ... bench:      767704 ns/iter (+/- 1603)
test scaling/immutabledb_open/in_memory/100000 ... bench:     1517430 ns/iter (+/- 12614)
test scaling/immutabledb_open/mmap_cached/100000 ... bench:     1505621 ns/iter (+/- 24292)
test scaling/immutabledb_open/in_memory/250000 ... bench:     4683010 ns/iter (+/- 43638)
test scaling/immutabledb_open/mmap_cached/250000 ... bench:     4602187 ns/iter (+/- 37428)
test scaling/immutabledb_open/in_memory/500000 ... bench:    18399813 ns/iter (+/- 163166)
test scaling/immutabledb_open/mmap_cached/500000 ... bench:    18200090 ns/iter (+/- 61106)

test scaling/chaindb_insert/default_20kb/10000 ... bench:  3496446549 ns/iter (+/- 123530342)
test scaling/chaindb_insert/default_20kb/50000 ... bench: 17664614282 ns/iter (+/- 139491062)
test scaling/chaindb_insert/default_20kb/100000 ... bench: 35775795407 ns/iter (+/- 510710441)
test scaling/chaindb_insert/default_20kb/250000 ... bench: 88763296073 ns/iter (+/- 1837501345)

```

## UTxO Benchmarks
```
test utxo_store/insert/default/1000000 ... bench:  2859324728 ns/iter (+/- 35672737)

test utxo_store/lookup/hit/1000000 ... bench:      541538 ns/iter (+/- 8495)
test utxo_store/lookup/miss/1000000 ... bench:      310699 ns/iter (+/- 1967)

test utxo_store/contains/hit ... bench:      376793 ns/iter (+/- 3259)
test utxo_store/contains/miss ... bench:      292705 ns/iter (+/- 2575)

test utxo_store/remove/sequential/1000000 ... bench:  2856511113 ns/iter (+/- 18068238)

test utxo_store/apply_tx/block_50tx_3in_2out ... bench:   248798399 ns/iter (+/- 17863852)
test utxo_store/apply_tx/block_300tx_2in_2out ... bench:   252802972 ns/iter (+/- 12864965)

test utxo_store/multi_asset/insert_mixed_30pct/1000000 ... bench:  3651692209 ns/iter (+/- 29129108)
test utxo_store/multi_asset/lookup_mixed_30pct/1000000 ... bench:   127030587 ns/iter (+/- 18810631)

test utxo_store/total_lovelace/scan/1000000 ... bench:   293281680 ns/iter (+/- 2317694)

test utxo_store/rebuild_address_index/rebuild/1000000 ... bench:   597145246 ns/iter (+/- 3442322)

test utxo_store/insert_configs/low_8gb/1000000 ... bench:  2808917167 ns/iter (+/- 12390242)
test utxo_store/insert_configs/mid_16gb/1000000 ... bench:  2807568452 ns/iter (+/- 22975110)
test utxo_store/insert_configs/high_32gb/1000000 ... bench:  2784761665 ns/iter (+/- 14752997)
test utxo_store/insert_configs/high_bloom_16gb/1000000 ... bench:  2774816066 ns/iter (+/- 14760448)
test utxo_store/insert_configs/legacy_small/1000000 ... bench:  2763525679 ns/iter (+/- 19471628)

test utxo_store/lookup_configs/low_8gb/1000000 ... bench:      437008 ns/iter (+/- 3757)
test utxo_store/lookup_configs/mid_16gb/1000000 ... bench:      438828 ns/iter (+/- 3370)
test utxo_store/lookup_configs/high_32gb/1000000 ... bench:      438705 ns/iter (+/- 1431)
test utxo_store/lookup_configs/high_bloom_16gb/1000000 ... bench:      437926 ns/iter (+/- 6069)
test utxo_store/lookup_configs/legacy_small/1000000 ... bench:      441516 ns/iter (+/- 6037)

test utxo_scaling/insert/default/100000 ... bench:   233886524 ns/iter (+/- 1607091)
test utxo_scaling/insert/default/500000 ... bench:  1309333657 ns/iter (+/- 4509367)
test utxo_scaling/insert/default/1000000 ... bench:  2771149149 ns/iter (+/- 12605872)

test utxo_scaling/lookup/hit/100000 ... bench:      359458 ns/iter (+/- 1310)
test utxo_scaling/lookup/hit/500000 ... bench:      405383 ns/iter (+/- 485)
test utxo_scaling/lookup/hit/1000000 ... bench:      441763 ns/iter (+/- 863)

test utxo_scaling/apply_tx/block_50tx_3in_2out/100000 ... bench:    11709367 ns/iter (+/- 581331)
test utxo_scaling/apply_tx/block_50tx_3in_2out/500000 ... bench:   125069677 ns/iter (+/- 7994826)
test utxo_scaling/apply_tx/block_50tx_3in_2out/1000000 ... bench:   262481302 ns/iter (+/- 18724306)

test utxo_scaling/total_lovelace/scan/100000 ... bench:    29359818 ns/iter (+/- 115108)
test utxo_scaling/total_lovelace/scan/500000 ... bench:   143800986 ns/iter (+/- 700304)
test utxo_scaling/total_lovelace/scan/1000000 ... bench:   301931092 ns/iter (+/- 1409222)

test utxo_large_scale/insert/default/5000000 ... bench: 16829871830 ns/iter (+/- 43235344)
test utxo_large_scale/insert/default/10000000 ... bench: 36069998370 ns/iter (+/- 210838075)

test utxo_large_scale/lookup/hit/5000000 ... bench:     1271915 ns/iter (+/- 2789)
test utxo_large_scale/lookup/hit/10000000 ... bench:     1609063 ns/iter (+/- 6713)

test utxo_large_scale/total_lovelace/scan/5000000 ... bench:  1581363432 ns/iter (+/- 9803349)
test utxo_large_scale/total_lovelace/scan/10000000 ... bench:  3108669762 ns/iter (+/- 30409694)

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
[1m[92m   Compiling[0m zerocopy-derive v0.8.42
[1m[92m   Compiling[0m serde_derive v1.0.228
[1m[92m   Compiling[0m thiserror-impl v2.0.18
[1m[92m   Compiling[0m serde_json v1.0.149
[1m[92m   Compiling[0m thiserror v2.0.18
[1m[92m   Compiling[0m torsten-lsm v0.4.3-alpha (/home/runner/work/torsten/torsten/crates/torsten-lsm)
[1m[92m   Compiling[0m zerocopy v0.8.42
[1m[92m   Compiling[0m tinytemplate v1.2.1
[1m[92m   Compiling[0m half v2.7.1
[1m[92m   Compiling[0m ppv-lite86 v0.2.21
[1m[92m   Compiling[0m ciborium-ll v0.2.2
[1m[92m   Compiling[0m rand_chacha v0.9.0
[1m[92m   Compiling[0m ciborium v0.2.2
[1m[92m   Compiling[0m proptest v1.10.0
[1m[92m   Compiling[0m criterion v0.5.1
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 28.89s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/torsten_lsm-79bc3ef93c50625a)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.64s

[1m[92m   Doc-tests[0m torsten_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s

```
