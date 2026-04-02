# Nightly Benchmark Results — 2026-03-31

Machine: GitHub Actions ubuntu-latest
Branch: main (80a4450)

## Storage Benchmarks
```
[1m[92m   Compiling[0m dugite-primitives v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m dugite-serialization v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m dugite-crypto v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m dugite-storage v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-storage)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 21.31s
[1m[92m     Running[0m benches/storage_bench.rs (target/release/deps/storage_bench-51833895afe17d60)
Gnuplot not found, using plotters backend
Benchmarking chaindb/sequential_insert/10k_20kb
Benchmarking chaindb/sequential_insert/10k_20kb: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 43.6s.
Benchmarking chaindb/sequential_insert/10k_20kb: Collecting 10 samples in estimated 43.636 s (10 iterations)
Benchmarking chaindb/sequential_insert/10k_20kb: Analyzing
chaindb/sequential_insert/10k_20kb
                        time:   [3.8565 s 3.8959 s 3.9457 s]
                        change: [-5.5923% +0.4670% +4.8559%] (p = 0.90 > 0.05)
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking chaindb/random_read/by_hash/10000blks
Benchmarking chaindb/random_read/by_hash/10000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 397.8s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/10000blks: Collecting 100 samples in estimated 397.84 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/10000blks: Analyzing
chaindb/random_read/by_hash/10000blks
                        time:   [33.166 ms 33.454 ms 33.747 ms]
                        change: [+10.285% +11.308% +12.373%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
Benchmarking chaindb/random_read/by_hash/100000blks
Benchmarking chaindb/random_read/by_hash/100000blks: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 3654.5s, or reduce sample count to 10.
Benchmarking chaindb/random_read/by_hash/100000blks: Collecting 100 samples in estimated 3654.5 s (100 iterations)
Benchmarking chaindb/random_read/by_hash/100000blks: Analyzing
chaindb/random_read/by_hash/100000blks
                        time:   [348.67 ms 352.49 ms 356.45 ms]
                        change: [-2.6268% -0.5403% +1.4778%] (p = 0.61 > 0.05)
                        No change in performance detected.
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild

Benchmarking chaindb/tip_query
Benchmarking chaindb/tip_query: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 358.7s, or reduce sample count to 10.
Benchmarking chaindb/tip_query: Collecting 100 samples in estimated 358.69 s (100 iterations)
Benchmarking chaindb/tip_query: Analyzing
chaindb/tip_query       time:   [31.402 ms 31.921 ms 32.661 ms]
                        change: [+7.2799% +9.1513% +11.415%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking chaindb/has_block
Benchmarking chaindb/has_block: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 364.9s, or reduce sample count to 10.
Benchmarking chaindb/has_block: Collecting 100 samples in estimated 364.87 s (100 iterations)
Benchmarking chaindb/has_block: Analyzing
chaindb/has_block       time:   [31.409 ms 31.734 ms 32.071 ms]
                        change: [+8.2218% +9.3760% +10.577%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild

Benchmarking chaindb/slot_range_100
Benchmarking chaindb/slot_range_100: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 352.6s, or reduce sample count to 10.
Benchmarking chaindb/slot_range_100: Collecting 100 samples in estimated 352.58 s (100 iterations)
Benchmarking chaindb/slot_range_100: Analyzing
chaindb/slot_range_100  time:   [33.470 ms 33.757 ms 34.049 ms]
                        change: [+10.469% +11.478% +12.526%] (p = 0.00 < 0.05)
                        Performance has regressed.

Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.6s.
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Collecting 10 samples in estimated 7.5804 s (10 iterations)
Benchmarking chaindb/flush_to_immutable/k_2160_blocks_20kb/2160: Analyzing
chaindb/flush_to_immutable/k_2160_blocks_20kb/2160
                        time:   [6.3241 ms 6.4461 ms 6.5751 ms]
                        change: [-2.7359% +0.0419% +2.9727%] (p = 0.99 > 0.05)
                        No change in performance detected.

Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 34.8s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Collecting 10 samples in estimated 34.842 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/in_memory: Analyzing
chaindb/profile_comparison/insert_10k_20kb/in_memory
                        time:   [3.5728 s 3.6097 s 3.6472 s]
                        change: [-3.4471% -1.1512% +1.3688%] (p = 0.39 > 0.05)
                        No change in performance detected.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.7s.
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Collecting 10 samples in estimated 35.673 s (10 iterations)
Benchmarking chaindb/profile_comparison/insert_10k_20kb/mmap: Analyzing
chaindb/profile_comparison/insert_10k_20kb/mmap
                        time:   [3.5176 s 3.5559 s 3.5964 s]
                        change: [-7.0750% -5.1991% -3.2685%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking chaindb/profile_comparison/read_500/in_memory
Benchmarking chaindb/profile_comparison/read_500/in_memory: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.9s.
Benchmarking chaindb/profile_comparison/read_500/in_memory: Collecting 10 samples in estimated 35.856 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/in_memory: Analyzing
chaindb/profile_comparison/read_500/in_memory
                        time:   [31.424 ms 32.301 ms 33.143 ms]
                        change: [+6.9564% +9.9607% +12.958%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking chaindb/profile_comparison/read_500/mmap
Benchmarking chaindb/profile_comparison/read_500/mmap: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 36.0s.
Benchmarking chaindb/profile_comparison/read_500/mmap: Collecting 10 samples in estimated 36.008 s (10 iterations)
Benchmarking chaindb/profile_comparison/read_500/mmap: Analyzing
chaindb/profile_comparison/read_500/mmap
                        time:   [31.247 ms 32.167 ms 33.067 ms]
                        change: [+5.6814% +9.1467% +12.197%] (p = 0.00 < 0.05)
                        Performance has regressed.

Benchmarking immutabledb/open/in_memory/10000
Benchmarking immutabledb/open/in_memory/10000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 9.7s, or reduce sample count to 50.
Benchmarking immutabledb/open/in_memory/10000: Collecting 100 samples in estimated 9.6883 s (100 iterations)
Benchmarking immutabledb/open/in_memory/10000: Analyzing
immutabledb/open/in_memory/10000
                        time:   [96.646 ms 96.815 ms 96.990 ms]
                        change: [+6.3660% +6.6210% +6.8681%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
Benchmarking immutabledb/open/mmap_cached/10000
Benchmarking immutabledb/open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cached/10000: Collecting 100 samples in estimated 5.0416 s (25k iterations)
Benchmarking immutabledb/open/mmap_cached/10000: Analyzing
immutabledb/open/mmap_cached/10000
                        time:   [198.94 µs 199.07 µs 199.22 µs]
                        change: [-3.0779% -2.7223% -2.3773%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 10 outliers among 100 measurements (10.00%)
  1 (1.00%) low mild
  6 (6.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/10000
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Collecting 100 samples in estimated 5.0206 s (1700 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/10000: Analyzing
immutabledb/open/mmap_cold_rebuild/10000
                        time:   [2.9777 ms 3.1165 ms 3.3796 ms]
                        change: [-9.5862% +1.4414% +13.094%] (p = 0.81 > 0.05)
                        No change in performance detected.
Found 4 outliers among 100 measurements (4.00%)
  1 (1.00%) high mild
  3 (3.00%) high severe
Benchmarking immutabledb/open/in_memory/100000
Benchmarking immutabledb/open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 116.7s, or reduce sample count to 10.
Benchmarking immutabledb/open/in_memory/100000: Collecting 100 samples in estimated 116.65 s (100 iterations)
Benchmarking immutabledb/open/in_memory/100000: Analyzing
immutabledb/open/in_memory/100000
                        time:   [890.02 ms 890.68 ms 891.46 ms]
                        change: [+3.0230% +3.1749% +3.3166%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) high mild
  1 (1.00%) high severe
Benchmarking immutabledb/open/mmap_cached/100000
Benchmarking immutabledb/open/mmap_cached/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.9s, enable flat sampling, or reduce sample count to 50.
Benchmarking immutabledb/open/mmap_cached/100000: Collecting 100 samples in estimated 8.8595 s (5050 iterations)
Benchmarking immutabledb/open/mmap_cached/100000: Analyzing
immutabledb/open/mmap_cached/100000
                        time:   [1.7507 ms 1.7541 ms 1.7579 ms]
                        change: [-0.3120% +0.2394% +0.6978%] (p = 0.38 > 0.05)
                        No change in performance detected.
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) low mild
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/open/mmap_cold_rebuild/100000
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Warming up for 3.0000 s
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Collecting 100 samples in estimated 5.1798 s (200 iterations)
Benchmarking immutabledb/open/mmap_cold_rebuild/100000: Analyzing
immutabledb/open/mmap_cold_rebuild/100000
                        time:   [25.904 ms 25.951 ms 26.001 ms]
                        change: [+1.6676% +1.9542% +2.2252%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild

Benchmarking immutabledb/lookup/in_memory/10000
Benchmarking immutabledb/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/in_memory/10000: Collecting 100 samples in estimated 5.2691 s (500 iterations)
Benchmarking immutabledb/lookup/in_memory/10000: Analyzing
immutabledb/lookup/in_memory/10000
                        time:   [10.502 ms 10.520 ms 10.540 ms]
                        change: [-1.7348% -1.4505% -1.1454%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 6 outliers among 100 measurements (6.00%)
  4 (4.00%) high mild
  2 (2.00%) high severe
Benchmarking immutabledb/lookup/mmap/10000
Benchmarking immutabledb/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking immutabledb/lookup/mmap/10000: Collecting 100 samples in estimated 5.2771 s (500 iterations)
Benchmarking immutabledb/lookup/mmap/10000: Analyzing
immutabledb/lookup/mmap/10000
                        time:   [10.491 ms 10.511 ms 10.534 ms]
                        change: [-0.7439% -0.4575% -0.1904%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 6 outliers among 100 measurements (6.00%)
  5 (5.00%) high mild
  1 (1.00%) high severe

Benchmarking immutabledb/has_block/in_memory
Benchmarking immutabledb/has_block/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/in_memory: Collecting 100 samples in estimated 5.1179 s (146k iterations)
Benchmarking immutabledb/has_block/in_memory: Analyzing
immutabledb/has_block/in_memory
                        time:   [34.931 µs 34.949 µs 34.975 µs]
                        change: [+0.6912% +1.0511% +1.2965%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 10 outliers among 100 measurements (10.00%)
  1 (1.00%) low mild
  4 (4.00%) high mild
  5 (5.00%) high severe
Benchmarking immutabledb/has_block/mmap
Benchmarking immutabledb/has_block/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/has_block/mmap: Collecting 100 samples in estimated 5.1180 s (146k iterations)
Benchmarking immutabledb/has_block/mmap: Analyzing
immutabledb/has_block/mmap
                        time:   [34.928 µs 34.937 µs 34.945 µs]
                        change: [+1.0670% +1.1849% +1.3024%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe

Benchmarking immutabledb/append/1k_blocks_20kb/in_memory
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Collecting 100 samples in estimated 5.9078 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/in_memory: Analyzing
immutabledb/append/1k_blocks_20kb/in_memory
                        time:   [14.700 ms 14.716 ms 14.732 ms]
                        change: [+0.5921% +0.7985% +0.9932%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
Benchmarking immutabledb/append/1k_blocks_20kb/mmap
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Collecting 100 samples in estimated 6.2386 s (400 iterations)
Benchmarking immutabledb/append/1k_blocks_20kb/mmap: Analyzing
immutabledb/append/1k_blocks_20kb/mmap
                        time:   [15.440 ms 15.471 ms 15.505 ms]
                        change: [+1.5208% +1.8136% +2.1077%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high severe

Benchmarking immutabledb/slot_range/range_100/in_memory
Benchmarking immutabledb/slot_range/range_100/in_memory: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/in_memory: Collecting 100 samples in estimated 5.1031 s (15k iterations)
Benchmarking immutabledb/slot_range/range_100/in_memory: Analyzing
immutabledb/slot_range/range_100/in_memory
                        time:   [331.47 µs 333.37 µs 335.36 µs]
                        change: [+5.1285% +6.0390% +6.9429%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 8 outliers among 100 measurements (8.00%)
  3 (3.00%) low severe
  3 (3.00%) low mild
  2 (2.00%) high mild
Benchmarking immutabledb/slot_range/range_100/mmap
Benchmarking immutabledb/slot_range/range_100/mmap: Warming up for 3.0000 s
Benchmarking immutabledb/slot_range/range_100/mmap: Collecting 100 samples in estimated 5.0415 s (15k iterations)
Benchmarking immutabledb/slot_range/range_100/mmap: Analyzing
immutabledb/slot_range/range_100/mmap
                        time:   [327.01 µs 328.30 µs 329.57 µs]
                        change: [+3.0729% +3.8614% +4.6162%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 6 outliers among 100 measurements (6.00%)
  2 (2.00%) low severe
  2 (2.00%) low mild
  2 (2.00%) high mild

Benchmarking block_index/insert/in_memory/10000
Benchmarking block_index/insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/10000: Collecting 100 samples in estimated 7.9861 s (10k iterations)
Benchmarking block_index/insert/in_memory/10000: Analyzing
block_index/insert/in_memory/10000
                        time:   [792.72 µs 793.60 µs 794.71 µs]
                        change: [-1.0961% -0.9222% -0.7304%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 3 outliers among 100 measurements (3.00%)
  1 (1.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/mmap/10000
Benchmarking block_index/insert/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/10000: Collecting 100 samples in estimated 5.4395 s (900 iterations)
Benchmarking block_index/insert/mmap/10000: Analyzing
block_index/insert/mmap/10000
                        time:   [6.0940 ms 6.1462 ms 6.2073 ms]
                        change: [-0.4581% +0.7447% +1.9679%] (p = 0.24 > 0.05)
                        No change in performance detected.
Found 11 outliers among 100 measurements (11.00%)
  3 (3.00%) high mild
  8 (8.00%) high severe
Benchmarking block_index/insert/in_memory/50000
Benchmarking block_index/insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/50000: Collecting 100 samples in estimated 5.2042 s (1500 iterations)
Benchmarking block_index/insert/in_memory/50000: Analyzing
block_index/insert/in_memory/50000
                        time:   [3.4683 ms 3.4725 ms 3.4792 ms]
                        change: [-0.3269% -0.0962% +0.1429%] (p = 0.47 > 0.05)
                        No change in performance detected.
Found 7 outliers among 100 measurements (7.00%)
  5 (5.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/insert/mmap/50000
Benchmarking block_index/insert/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/insert/mmap/50000: Collecting 100 samples in estimated 8.9408 s (200 iterations)
Benchmarking block_index/insert/mmap/50000: Analyzing
block_index/insert/mmap/50000
                        time:   [44.521 ms 44.744 ms 45.029 ms]
                        change: [+0.4845% +1.4134% +2.3083%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking block_index/insert/in_memory/100000
Benchmarking block_index/insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/insert/in_memory/100000: Collecting 100 samples in estimated 5.0003 s (700 iterations)
Benchmarking block_index/insert/in_memory/100000: Analyzing
block_index/insert/in_memory/100000
                        time:   [7.0738 ms 7.0958 ms 7.1206 ms]
                        change: [+0.0975% +0.4177% +0.7866%] (p = 0.01 < 0.05)
                        Change within noise threshold.
Found 12 outliers among 100 measurements (12.00%)
  6 (6.00%) high mild
  6 (6.00%) high severe
Benchmarking block_index/insert/mmap/100000
Benchmarking block_index/insert/mmap/100000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 8.7s, or reduce sample count to 50.
Benchmarking block_index/insert/mmap/100000: Collecting 100 samples in estimated 8.6956 s (100 iterations)
Benchmarking block_index/insert/mmap/100000: Analyzing
block_index/insert/mmap/100000
                        time:   [86.638 ms 86.973 ms 87.511 ms]
                        change: [+1.9727% +2.4033% +3.0434%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe

Benchmarking block_index/lookup/in_memory/10000
Benchmarking block_index/lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/10000: Collecting 100 samples in estimated 5.0756 s (323k iterations)
Benchmarking block_index/lookup/in_memory/10000: Analyzing
block_index/lookup/in_memory/10000
                        time:   [15.990 µs 16.022 µs 16.080 µs]
                        change: [-8.6981% -8.2496% -7.6827%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 11 outliers among 100 measurements (11.00%)
  2 (2.00%) high mild
  9 (9.00%) high severe
Benchmarking block_index/lookup/mmap/10000
Benchmarking block_index/lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/10000: Collecting 100 samples in estimated 5.1370 s (172k iterations)
Benchmarking block_index/lookup/mmap/10000: Analyzing
block_index/lookup/mmap/10000
                        time:   [29.617 µs 29.645 µs 29.691 µs]
                        change: [+3.0511% +3.9111% +4.7901%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 8 outliers among 100 measurements (8.00%)
  3 (3.00%) high mild
  5 (5.00%) high severe
Benchmarking block_index/lookup/in_memory/50000
Benchmarking block_index/lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/50000: Collecting 100 samples in estimated 5.0745 s (318k iterations)
Benchmarking block_index/lookup/in_memory/50000: Analyzing
block_index/lookup/in_memory/50000
                        time:   [16.426 µs 16.431 µs 16.437 µs]
                        change: [-8.7062% -8.4978% -8.2466%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 7 outliers among 100 measurements (7.00%)
  3 (3.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/50000
Benchmarking block_index/lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/50000: Collecting 100 samples in estimated 5.0333 s (247k iterations)
Benchmarking block_index/lookup/mmap/50000: Analyzing
block_index/lookup/mmap/50000
                        time:   [20.181 µs 20.184 µs 20.188 µs]
                        change: [-0.2709% -0.1504% -0.0020%] (p = 0.02 < 0.05)
                        Change within noise threshold.
Found 8 outliers among 100 measurements (8.00%)
  4 (4.00%) low mild
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking block_index/lookup/in_memory/100000
Benchmarking block_index/lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/in_memory/100000: Collecting 100 samples in estimated 5.0347 s (313k iterations)
Benchmarking block_index/lookup/in_memory/100000: Analyzing
block_index/lookup/in_memory/100000
                        time:   [16.570 µs 16.577 µs 16.583 µs]
                        change: [-10.017% -9.7382% -9.3078%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/lookup/mmap/100000
Benchmarking block_index/lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking block_index/lookup/mmap/100000: Collecting 100 samples in estimated 5.0111 s (252k iterations)
Benchmarking block_index/lookup/mmap/100000: Analyzing
block_index/lookup/mmap/100000
                        time:   [19.698 µs 19.709 µs 19.726 µs]
                        change: [-0.0689% +0.1290% +0.4034%] (p = 0.35 > 0.05)
                        No change in performance detected.
Found 4 outliers among 100 measurements (4.00%)
  4 (4.00%) high severe

Benchmarking block_index/contains_miss/in_memory
Benchmarking block_index/contains_miss/in_memory: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/in_memory: Collecting 100 samples in estimated 5.0097 s (449k iterations)
Benchmarking block_index/contains_miss/in_memory: Analyzing
block_index/contains_miss/in_memory
                        time:   [11.146 µs 11.156 µs 11.172 µs]
                        change: [+0.7502% +1.1338% +1.6958%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 7 outliers among 100 measurements (7.00%)
  3 (3.00%) high mild
  4 (4.00%) high severe
Benchmarking block_index/contains_miss/mmap
Benchmarking block_index/contains_miss/mmap: Warming up for 3.0000 s
Benchmarking block_index/contains_miss/mmap: Collecting 100 samples in estimated 5.1523 s (121k iterations)
Benchmarking block_index/contains_miss/mmap: Analyzing
block_index/contains_miss/mmap
                        time:   [42.483 µs 42.491 µs 42.500 µs]
                        change: [-0.0048% +0.1100% +0.2733%] (p = 0.11 > 0.05)
                        No change in performance detected.
Found 9 outliers among 100 measurements (9.00%)
  3 (3.00%) low mild
  4 (4.00%) high mild
  2 (2.00%) high severe

Benchmarking scaling/block_index_insert/in_memory/10000
Benchmarking scaling/block_index_insert/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/10000: Collecting 10 samples in estimated 5.0136 s (6325 iterations)
Benchmarking scaling/block_index_insert/in_memory/10000: Analyzing
scaling/block_index_insert/in_memory/10000
                        time:   [790.94 µs 791.79 µs 792.89 µs]
                        change: [-1.0313% -0.8305% -0.6399%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Benchmarking scaling/block_index_insert/mmap/10000
Benchmarking scaling/block_index_insert/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/10000: Collecting 10 samples in estimated 5.0585 s (825 iterations)
Benchmarking scaling/block_index_insert/mmap/10000: Analyzing
scaling/block_index_insert/mmap/10000
                        time:   [6.1100 ms 6.1601 ms 6.2217 ms]
                        change: [+0.1296% +1.3816% +3.1585%] (p = 0.09 > 0.05)
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/50000
Benchmarking scaling/block_index_insert/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/50000: Collecting 10 samples in estimated 5.0085 s (1430 iterations)
Benchmarking scaling/block_index_insert/in_memory/50000: Analyzing
scaling/block_index_insert/in_memory/50000
                        time:   [3.4833 ms 3.4886 ms 3.4949 ms]
                        change: [-1.9416% -1.0369% -0.4653%] (p = 0.01 < 0.05)
                        Change within noise threshold.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/50000
Benchmarking scaling/block_index_insert/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/50000: Collecting 10 samples in estimated 7.4114 s (165 iterations)
Benchmarking scaling/block_index_insert/mmap/50000: Analyzing
scaling/block_index_insert/mmap/50000
                        time:   [44.930 ms 45.414 ms 45.950 ms]
                        change: [-0.2937% +2.2480% +3.9675%] (p = 0.07 > 0.05)
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_insert/in_memory/100000
Benchmarking scaling/block_index_insert/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/100000: Collecting 10 samples in estimated 5.0890 s (660 iterations)
Benchmarking scaling/block_index_insert/in_memory/100000: Analyzing
scaling/block_index_insert/in_memory/100000
                        time:   [7.7086 ms 7.8747 ms 8.1343 ms]
                        change: [+9.0141% +12.452% +16.708%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/100000
Benchmarking scaling/block_index_insert/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/100000: Collecting 10 samples in estimated 9.7761 s (110 iterations)
Benchmarking scaling/block_index_insert/mmap/100000: Analyzing
scaling/block_index_insert/mmap/100000
                        time:   [87.357 ms 87.704 ms 88.071 ms]
                        change: [+2.6673% +3.3126% +3.9670%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/block_index_insert/in_memory/250000
Benchmarking scaling/block_index_insert/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/250000: Collecting 10 samples in estimated 6.5103 s (220 iterations)
Benchmarking scaling/block_index_insert/in_memory/250000: Analyzing
scaling/block_index_insert/in_memory/250000
                        time:   [29.439 ms 30.215 ms 30.788 ms]
                        change: [+4.4213% +6.4447% +8.6110%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/block_index_insert/mmap/250000
Benchmarking scaling/block_index_insert/mmap/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 9.7s or enable flat sampling.
Benchmarking scaling/block_index_insert/mmap/250000: Collecting 10 samples in estimated 9.6850 s (55 iterations)
Benchmarking scaling/block_index_insert/mmap/250000: Analyzing
scaling/block_index_insert/mmap/250000
                        time:   [178.21 ms 179.77 ms 180.78 ms]
                        change: [+3.2455% +3.7511% +4.3701%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/500000
Benchmarking scaling/block_index_insert/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/in_memory/500000: Collecting 10 samples in estimated 7.9367 s (110 iterations)
Benchmarking scaling/block_index_insert/in_memory/500000: Analyzing
scaling/block_index_insert/in_memory/500000
                        time:   [70.866 ms 71.425 ms 72.341 ms]
                        change: [+7.2517% +8.4908% +9.8531%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/mmap/500000
Benchmarking scaling/block_index_insert/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_insert/mmap/500000: Collecting 10 samples in estimated 6.5613 s (20 iterations)
Benchmarking scaling/block_index_insert/mmap/500000: Analyzing
scaling/block_index_insert/mmap/500000
                        time:   [323.78 ms 326.52 ms 329.60 ms]
                        change: [+7.0815% +8.0177% +9.2551%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_insert/in_memory/1000000
Benchmarking scaling/block_index_insert/in_memory/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 9.0s or enable flat sampling.
Benchmarking scaling/block_index_insert/in_memory/1000000: Collecting 10 samples in estimated 9.0346 s (55 iterations)
Benchmarking scaling/block_index_insert/in_memory/1000000: Analyzing
scaling/block_index_insert/in_memory/1000000
                        time:   [160.87 ms 162.39 ms 165.38 ms]
                        change: [+8.3978% +9.9899% +12.348%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/block_index_insert/mmap/1000000
Benchmarking scaling/block_index_insert/mmap/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.7s.
Benchmarking scaling/block_index_insert/mmap/1000000: Collecting 10 samples in estimated 5.7017 s (10 iterations)
Benchmarking scaling/block_index_insert/mmap/1000000: Analyzing
scaling/block_index_insert/mmap/1000000
                        time:   [568.54 ms 571.56 ms 574.41 ms]
                        change: [-2.3904% +0.0326% +2.4653%] (p = 0.98 > 0.05)
                        No change in performance detected.

Benchmarking scaling/block_index_lookup/in_memory/10000
Benchmarking scaling/block_index_lookup/in_memory/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/10000: Collecting 10 samples in estimated 5.0002 s (318k iterations)
Benchmarking scaling/block_index_lookup/in_memory/10000: Analyzing
scaling/block_index_lookup/in_memory/10000
                        time:   [15.702 µs 15.708 µs 15.716 µs]
                        change: [+1.7881% +1.8329% +1.8776%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/10000
Benchmarking scaling/block_index_lookup/mmap/10000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/10000: Collecting 10 samples in estimated 5.0016 s (166k iterations)
Benchmarking scaling/block_index_lookup/mmap/10000: Analyzing
scaling/block_index_lookup/mmap/10000
                        time:   [29.992 µs 30.047 µs 30.125 µs]
                        change: [+0.0725% +0.3311% +0.5514%] (p = 0.02 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) low mild
Benchmarking scaling/block_index_lookup/in_memory/50000
Benchmarking scaling/block_index_lookup/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/50000: Collecting 10 samples in estimated 5.0001 s (312k iterations)
Benchmarking scaling/block_index_lookup/in_memory/50000: Analyzing
scaling/block_index_lookup/in_memory/50000
                        time:   [16.004 µs 16.017 µs 16.029 µs]
                        change: [+1.1686% +1.2368% +1.3135%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/block_index_lookup/mmap/50000
Benchmarking scaling/block_index_lookup/mmap/50000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/50000: Collecting 10 samples in estimated 5.0004 s (244k iterations)
Benchmarking scaling/block_index_lookup/mmap/50000: Analyzing
scaling/block_index_lookup/mmap/50000
                        time:   [20.489 µs 20.492 µs 20.494 µs]
                        change: [+0.5961% +0.6629% +0.7268%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/100000
Benchmarking scaling/block_index_lookup/in_memory/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/100000: Collecting 10 samples in estimated 5.0004 s (311k iterations)
Benchmarking scaling/block_index_lookup/in_memory/100000: Analyzing
scaling/block_index_lookup/in_memory/100000
                        time:   [16.051 µs 16.056 µs 16.061 µs]
                        change: [+0.4425% +0.6570% +0.8347%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/100000
Benchmarking scaling/block_index_lookup/mmap/100000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/100000: Collecting 10 samples in estimated 5.0003 s (251k iterations)
Benchmarking scaling/block_index_lookup/mmap/100000: Analyzing
scaling/block_index_lookup/mmap/100000
                        time:   [19.969 µs 19.973 µs 19.977 µs]
                        change: [+0.4070% +0.4614% +0.5238%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/250000
Benchmarking scaling/block_index_lookup/in_memory/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/250000: Collecting 10 samples in estimated 5.0005 s (312k iterations)
Benchmarking scaling/block_index_lookup/in_memory/250000: Analyzing
scaling/block_index_lookup/in_memory/250000
                        time:   [16.051 µs 16.058 µs 16.064 µs]
                        change: [+0.4445% +0.5100% +0.5816%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/block_index_lookup/mmap/250000
Benchmarking scaling/block_index_lookup/mmap/250000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/250000: Collecting 10 samples in estimated 5.0008 s (252k iterations)
Benchmarking scaling/block_index_lookup/mmap/250000: Analyzing
scaling/block_index_lookup/mmap/250000
                        time:   [19.861 µs 19.867 µs 19.873 µs]
                        change: [+0.4256% +0.5474% +0.7224%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe
Benchmarking scaling/block_index_lookup/in_memory/500000
Benchmarking scaling/block_index_lookup/in_memory/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/500000: Collecting 10 samples in estimated 5.0002 s (310k iterations)
Benchmarking scaling/block_index_lookup/in_memory/500000: Analyzing
scaling/block_index_lookup/in_memory/500000
                        time:   [16.111 µs 16.116 µs 16.120 µs]
                        change: [+0.3893% +0.6397% +0.8523%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Benchmarking scaling/block_index_lookup/mmap/500000
Benchmarking scaling/block_index_lookup/mmap/500000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/500000: Collecting 10 samples in estimated 5.0010 s (256k iterations)
Benchmarking scaling/block_index_lookup/mmap/500000: Analyzing
scaling/block_index_lookup/mmap/500000
                        time:   [19.590 µs 19.594 µs 19.597 µs]
                        change: [+0.5682% +0.6230% +0.6657%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Benchmarking scaling/block_index_lookup/in_memory/1000000
Benchmarking scaling/block_index_lookup/in_memory/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/in_memory/1000000: Collecting 10 samples in estimated 5.0009 s (310k iterations)
Benchmarking scaling/block_index_lookup/in_memory/1000000: Analyzing
scaling/block_index_lookup/in_memory/1000000
                        time:   [16.114 µs 16.122 µs 16.127 µs]
                        change: [-1.4493% -0.5542% +0.0585%] (p = 0.21 > 0.05)
                        No change in performance detected.
Benchmarking scaling/block_index_lookup/mmap/1000000
Benchmarking scaling/block_index_lookup/mmap/1000000: Warming up for 3.0000 s
Benchmarking scaling/block_index_lookup/mmap/1000000: Collecting 10 samples in estimated 5.0009 s (257k iterations)
Benchmarking scaling/block_index_lookup/mmap/1000000: Analyzing
scaling/block_index_lookup/mmap/1000000
                        time:   [19.534 µs 19.536 µs 19.538 µs]
                        change: [+0.6082% +0.6729% +0.7317%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking scaling/immutabledb_open/in_memory/10000
Benchmarking scaling/immutabledb_open/in_memory/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.5s or enable flat sampling.
Benchmarking scaling/immutabledb_open/in_memory/10000: Collecting 10 samples in estimated 5.5049 s (55 iterations)
Benchmarking scaling/immutabledb_open/in_memory/10000: Analyzing
scaling/immutabledb_open/in_memory/10000
                        time:   [96.896 ms 97.192 ms 97.509 ms]
                        change: [+6.9456% +7.2465% +7.5184%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/immutabledb_open/mmap_cached/10000
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Collecting 10 samples in estimated 5.0071 s (25k iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/10000: Analyzing
scaling/immutabledb_open/mmap_cached/10000
                        time:   [198.32 µs 198.43 µs 198.56 µs]
                        change: [-1.8797% -1.1441% -0.4968%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
Benchmarking scaling/immutabledb_open/in_memory/50000
Benchmarking scaling/immutabledb_open/in_memory/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/in_memory/50000: Collecting 10 samples in estimated 9.0862 s (20 iterations)
Benchmarking scaling/immutabledb_open/in_memory/50000: Analyzing
scaling/immutabledb_open/in_memory/50000
                        time:   [451.55 ms 452.94 ms 454.28 ms]
                        change: [+4.1610% +4.5027% +4.8598%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/immutabledb_open/mmap_cached/50000
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Collecting 10 samples in estimated 5.0382 s (6875 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/50000: Analyzing
scaling/immutabledb_open/mmap_cached/50000
                        time:   [731.96 µs 732.87 µs 733.74 µs]
                        change: [-7.8723% -6.6146% -5.4404%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking scaling/immutabledb_open/in_memory/100000
Benchmarking scaling/immutabledb_open/in_memory/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 11.7s.
Benchmarking scaling/immutabledb_open/in_memory/100000: Collecting 10 samples in estimated 11.734 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/100000: Analyzing
scaling/immutabledb_open/in_memory/100000
                        time:   [896.63 ms 897.72 ms 898.97 ms]
                        change: [+3.8380% +4.0138% +4.1922%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/immutabledb_open/mmap_cached/100000
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Collecting 10 samples in estimated 5.0564 s (2640 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/100000: Analyzing
scaling/immutabledb_open/mmap_cached/100000
                        time:   [2.0499 ms 2.1506 ms 2.2075 ms]
                        change: [+13.976% +18.275% +22.612%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/immutabledb_open/in_memory/250000
Benchmarking scaling/immutabledb_open/in_memory/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 36.7s.
Benchmarking scaling/immutabledb_open/in_memory/250000: Collecting 10 samples in estimated 36.715 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/250000: Analyzing
scaling/immutabledb_open/in_memory/250000
                        time:   [2.2323 s 2.2376 s 2.2447 s]
                        change: [+3.8275% +4.0753% +4.3962%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/250000
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Collecting 10 samples in estimated 5.0064 s (825 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/250000: Analyzing
scaling/immutabledb_open/mmap_cached/250000
                        time:   [6.1209 ms 6.1824 ms 6.2723 ms]
                        change: [+20.308% +21.889% +23.457%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking scaling/immutabledb_open/in_memory/500000
Benchmarking scaling/immutabledb_open/in_memory/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 80.0s.
Benchmarking scaling/immutabledb_open/in_memory/500000: Collecting 10 samples in estimated 80.045 s (10 iterations)
Benchmarking scaling/immutabledb_open/in_memory/500000: Analyzing
scaling/immutabledb_open/in_memory/500000
                        time:   [22.978 s 23.105 s 23.283 s]
                        change: [-2.2190% -0.7393% +0.4884%] (p = 0.37 > 0.05)
                        No change in performance detected.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high severe
Benchmarking scaling/immutabledb_open/mmap_cached/500000
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Warming up for 3.0000 s
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Collecting 10 samples in estimated 5.2302 s (440 iterations)
Benchmarking scaling/immutabledb_open/mmap_cached/500000: Analyzing
scaling/immutabledb_open/mmap_cached/500000
                        time:   [11.341 ms 11.531 ms 11.695 ms]
                        change: [+19.203% +21.074% +23.785%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe

Benchmarking scaling/chaindb_insert/default_20kb/10000
Benchmarking scaling/chaindb_insert/default_20kb/10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.2s.
Benchmarking scaling/chaindb_insert/default_20kb/10000: Collecting 10 samples in estimated 35.160 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/10000: Analyzing
scaling/chaindb_insert/default_20kb/10000
                        time:   [3.4646 s 3.4857 s 3.5117 s]
                        change: [-23.106% -21.704% -20.404%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking scaling/chaindb_insert/default_20kb/50000
Benchmarking scaling/chaindb_insert/default_20kb/50000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 183.3s.
Benchmarking scaling/chaindb_insert/default_20kb/50000: Collecting 10 samples in estimated 183.33 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/50000: Analyzing
scaling/chaindb_insert/default_20kb/50000
                        time:   [18.005 s 18.097 s 18.203 s]
                        change: [-22.114% -21.085% -19.929%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking scaling/chaindb_insert/default_20kb/100000
Benchmarking scaling/chaindb_insert/default_20kb/100000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 362.0s.
Benchmarking scaling/chaindb_insert/default_20kb/100000: Collecting 10 samples in estimated 361.98 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/100000: Analyzing
scaling/chaindb_insert/default_20kb/100000
                        time:   [35.708 s 35.970 s 36.238 s]
                        change: [-15.933% -14.346% -12.819%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking scaling/chaindb_insert/default_20kb/250000
Benchmarking scaling/chaindb_insert/default_20kb/250000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 924.8s.
Benchmarking scaling/chaindb_insert/default_20kb/250000: Collecting 10 samples in estimated 924.75 s (10 iterations)
Benchmarking scaling/chaindb_insert/default_20kb/250000: Analyzing
scaling/chaindb_insert/default_20kb/250000
                        time:   [90.400 s 90.826 s 91.308 s]
                        change: [-13.270% -11.781% -10.267%] (p = 0.00 < 0.05)
                        Performance has improved.

```

## UTxO Benchmarks
```
[1m[92m   Compiling[0m dugite-primitives v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-primitives)
[1m[92m   Compiling[0m dugite-lsm v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
[1m[92m   Compiling[0m dugite-serialization v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-serialization)
[1m[92m   Compiling[0m dugite-crypto v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-crypto)
[1m[92m   Compiling[0m dugite-ledger v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-ledger)
[1m[92m    Finished[0m `bench` profile [optimized] target(s) in 31.20s
[1m[92m     Running[0m benches/utxo_bench.rs (target/release/deps/utxo_bench-4b37a42019cfa4eb)
Gnuplot not found, using plotters backend
Benchmarking utxo_store/insert/default/1000000
Benchmarking utxo_store/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.7s.
Benchmarking utxo_store/insert/default/1000000: Collecting 10 samples in estimated 27.654 s (10 iterations)
Benchmarking utxo_store/insert/default/1000000: Analyzing
utxo_store/insert/default/1000000
                        time:   [2.7135 s 2.7272 s 2.7424 s]
                        change: [+6.7929% +7.5279% +8.3125%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_store/lookup/hit/1000000
Benchmarking utxo_store/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/hit/1000000: Collecting 100 samples in estimated 5.6555 s (10k iterations)
Benchmarking utxo_store/lookup/hit/1000000: Analyzing
utxo_store/lookup/hit/1000000
                        time:   [571.53 µs 572.17 µs 572.90 µs]
                        change: [-1.1454% -0.8087% -0.5179%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup/miss/1000000
Benchmarking utxo_store/lookup/miss/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup/miss/1000000: Collecting 100 samples in estimated 5.0879 s (15k iterations)
Benchmarking utxo_store/lookup/miss/1000000: Analyzing
utxo_store/lookup/miss/1000000
                        time:   [340.28 µs 340.96 µs 341.64 µs]
                        change: [+1.6372% +2.0216% +2.3477%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high severe

Benchmarking utxo_store/contains/hit
Benchmarking utxo_store/contains/hit: Warming up for 3.0000 s
Benchmarking utxo_store/contains/hit: Collecting 100 samples in estimated 6.3002 s (15k iterations)
Benchmarking utxo_store/contains/hit: Analyzing
utxo_store/contains/hit time:   [411.58 µs 412.75 µs 414.74 µs]
                        change: [+3.7242% +4.4689% +5.7193%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 5 outliers among 100 measurements (5.00%)
  1 (1.00%) high mild
  4 (4.00%) high severe
Benchmarking utxo_store/contains/miss
Benchmarking utxo_store/contains/miss: Warming up for 3.0000 s
Benchmarking utxo_store/contains/miss: Collecting 100 samples in estimated 6.3952 s (20k iterations)
Benchmarking utxo_store/contains/miss: Analyzing
utxo_store/contains/miss
                        time:   [313.77 µs 314.51 µs 315.18 µs]
                        change: [+3.4345% +3.8806% +4.3569%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 6 outliers among 100 measurements (6.00%)
  5 (5.00%) high mild
  1 (1.00%) high severe

Benchmarking utxo_store/remove/sequential/1000000
Benchmarking utxo_store/remove/sequential/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 51.6s.
Benchmarking utxo_store/remove/sequential/1000000: Collecting 10 samples in estimated 51.622 s (10 iterations)
Benchmarking utxo_store/remove/sequential/1000000: Analyzing
utxo_store/remove/sequential/1000000
                        time:   [2.7521 s 2.7713 s 2.7926 s]
                        change: [+6.6256% +7.5678% +8.5695%] (p = 0.00 < 0.05)
                        Performance has regressed.

Benchmarking utxo_store/apply_tx/block_50tx_3in_2out
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.4s.
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Collecting 10 samples in estimated 27.400 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_50tx_3in_2out: Analyzing
utxo_store/apply_tx/block_50tx_3in_2out
                        time:   [303.88 ms 309.00 ms 314.04 ms]
                        change: [+14.563% +17.682% +20.713%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.1s.
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Collecting 10 samples in estimated 27.097 s (10 iterations)
Benchmarking utxo_store/apply_tx/block_300tx_2in_2out: Analyzing
utxo_store/apply_tx/block_300tx_2in_2out
                        time:   [308.95 ms 315.98 ms 324.55 ms]
                        change: [+15.613% +20.267% +26.747%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high severe

Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 37.0s.
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Collecting 10 samples in estimated 37.005 s (10 iterations)
Benchmarking utxo_store/multi_asset/insert_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/insert_mixed_30pct/1000000
                        time:   [3.5050 s 3.5239 s 3.5416 s]
                        change: [+5.5791% +6.2536% +6.9193%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 35.2s.
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Collecting 10 samples in estimated 35.192 s (10 iterations)
Benchmarking utxo_store/multi_asset/lookup_mixed_30pct/1000000: Analyzing
utxo_store/multi_asset/lookup_mixed_30pct/1000000
                        time:   [125.07 ms 131.25 ms 137.87 ms]
                        change: [-0.3369% +7.2916% +16.268%] (p = 0.11 > 0.05)
                        No change in performance detected.

Benchmarking utxo_store/total_lovelace/scan/1000000
Benchmarking utxo_store/total_lovelace/scan/1000000: Warming up for 3.0000 s

Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 31.7s, or reduce sample count to 10.
Benchmarking utxo_store/total_lovelace/scan/1000000: Collecting 100 samples in estimated 31.664 s (100 iterations)
Benchmarking utxo_store/total_lovelace/scan/1000000: Analyzing
utxo_store/total_lovelace/scan/1000000
                        time:   [318.13 ms 319.19 ms 320.24 ms]
                        change: [+7.6720% +8.1627% +8.6402%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  1 (1.00%) low severe
  1 (1.00%) high mild

Benchmarking utxo_store/rebuild_address_index/rebuild/1000000
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 38.1s.
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Collecting 10 samples in estimated 38.141 s (10 iterations)
Benchmarking utxo_store/rebuild_address_index/rebuild/1000000: Analyzing
utxo_store/rebuild_address_index/rebuild/1000000
                        time:   [625.08 ms 627.63 ms 629.77 ms]
                        change: [+7.0614% +7.8204% +8.5147%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild

Benchmarking utxo_store/insert_configs/low_8gb/1000000
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.0s.
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Collecting 10 samples in estimated 26.957 s (10 iterations)
Benchmarking utxo_store/insert_configs/low_8gb/1000000: Analyzing
utxo_store/insert_configs/low_8gb/1000000
                        time:   [2.6996 s 2.7066 s 2.7136 s]
                        change: [+7.0643% +7.6037% +8.0917%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.1s.
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Collecting 10 samples in estimated 27.135 s (10 iterations)
Benchmarking utxo_store/insert_configs/mid_16gb/1000000: Analyzing
utxo_store/insert_configs/mid_16gb/1000000
                        time:   [2.6984 s 2.7213 s 2.7431 s]
                        change: [+6.9408% +7.9055% +8.9584%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/insert_configs/high_32gb/1000000
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.4s.
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Collecting 10 samples in estimated 27.440 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_32gb/1000000: Analyzing
utxo_store/insert_configs/high_32gb/1000000
                        time:   [2.6939 s 2.7061 s 2.7178 s]
                        change: [+6.8115% +7.4814% +8.1556%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.1s.
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Collecting 10 samples in estimated 27.146 s (10 iterations)
Benchmarking utxo_store/insert_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/insert_configs/high_bloom_16gb/1000000
                        time:   [2.7050 s 2.7208 s 2.7359 s]
                        change: [+7.7127% +8.4982% +9.1683%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_store/insert_configs/legacy_small/1000000
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.0s.
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Collecting 10 samples in estimated 27.049 s (10 iterations)
Benchmarking utxo_store/insert_configs/legacy_small/1000000: Analyzing
utxo_store/insert_configs/legacy_small/1000000
                        time:   [2.6878 s 2.7081 s 2.7311 s]
                        change: [+6.7427% +7.8779% +9.0464%] (p = 0.00 < 0.05)
                        Performance has regressed.

Benchmarking utxo_store/lookup_configs/low_8gb/1000000
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Collecting 100 samples in estimated 6.7586 s (15k iterations)
Benchmarking utxo_store/lookup_configs/low_8gb/1000000: Analyzing
utxo_store/lookup_configs/low_8gb/1000000
                        time:   [446.31 µs 446.85 µs 447.46 µs]
                        change: [-0.2319% +0.0049% +0.2576%] (p = 0.97 > 0.05)
                        No change in performance detected.
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Collecting 100 samples in estimated 6.7499 s (15k iterations)
Benchmarking utxo_store/lookup_configs/mid_16gb/1000000: Analyzing
utxo_store/lookup_configs/mid_16gb/1000000
                        time:   [446.07 µs 446.53 µs 446.96 µs]
                        change: [+0.3195% +0.5751% +0.9419%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 4 outliers among 100 measurements (4.00%)
  3 (3.00%) high mild
  1 (1.00%) high severe
Benchmarking utxo_store/lookup_configs/high_32gb/1000000
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Collecting 100 samples in estimated 6.8245 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_32gb/1000000: Analyzing
utxo_store/lookup_configs/high_32gb/1000000
                        time:   [450.71 µs 451.34 µs 452.00 µs]
                        change: [+1.6047% +1.8812% +2.2442%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 5 outliers among 100 measurements (5.00%)
  2 (2.00%) high mild
  3 (3.00%) high severe
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Collecting 100 samples in estimated 6.8044 s (15k iterations)
Benchmarking utxo_store/lookup_configs/high_bloom_16gb/1000000: Analyzing
utxo_store/lookup_configs/high_bloom_16gb/1000000
                        time:   [450.82 µs 451.54 µs 452.39 µs]
                        change: [+1.7109% +1.9950% +2.3276%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high severe
Benchmarking utxo_store/lookup_configs/legacy_small/1000000
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Warming up for 3.0000 s
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Collecting 100 samples in estimated 6.7654 s (15k iterations)
Benchmarking utxo_store/lookup_configs/legacy_small/1000000: Analyzing
utxo_store/lookup_configs/legacy_small/1000000
                        time:   [443.50 µs 444.00 µs 444.53 µs]
                        change: [-0.7797% -0.5229% -0.1439%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 5 outliers among 100 measurements (5.00%)
  4 (4.00%) high mild
  1 (1.00%) high severe

Benchmarking utxo_scaling/insert/default/100000
Benchmarking utxo_scaling/insert/default/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/insert/default/100000: Collecting 10 samples in estimated 6.4742 s (30 iterations)
Benchmarking utxo_scaling/insert/default/100000: Analyzing
utxo_scaling/insert/default/100000
                        time:   [217.93 ms 218.90 ms 219.86 ms]
                        change: [+7.4718% +8.2476% +9.0430%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_scaling/insert/default/500000
Benchmarking utxo_scaling/insert/default/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 12.5s.
Benchmarking utxo_scaling/insert/default/500000: Collecting 10 samples in estimated 12.478 s (10 iterations)
Benchmarking utxo_scaling/insert/default/500000: Analyzing
utxo_scaling/insert/default/500000
                        time:   [1.2331 s 1.2446 s 1.2559 s]
                        change: [+5.1983% +6.5160% +7.9033%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_scaling/insert/default/1000000
Benchmarking utxo_scaling/insert/default/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 27.2s.
Benchmarking utxo_scaling/insert/default/1000000: Collecting 10 samples in estimated 27.151 s (10 iterations)
Benchmarking utxo_scaling/insert/default/1000000: Analyzing
utxo_scaling/insert/default/1000000
                        time:   [2.7033 s 2.7196 s 2.7395 s]
                        change: [+8.4421% +9.1651% +9.9677%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high severe

Benchmarking utxo_scaling/lookup/hit/100000
Benchmarking utxo_scaling/lookup/hit/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/100000: Collecting 10 samples in estimated 5.0074 s (13k iterations)
Benchmarking utxo_scaling/lookup/hit/100000: Analyzing
utxo_scaling/lookup/hit/100000
                        time:   [365.73 µs 366.63 µs 367.33 µs]
                        change: [+0.6812% +1.0822% +1.6290%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Benchmarking utxo_scaling/lookup/hit/500000
Benchmarking utxo_scaling/lookup/hit/500000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/500000: Collecting 10 samples in estimated 5.0112 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/500000: Analyzing
utxo_scaling/lookup/hit/500000
                        time:   [414.86 µs 415.69 µs 417.21 µs]
                        change: [+1.9620% +2.2643% +2.5974%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_scaling/lookup/hit/1000000
Benchmarking utxo_scaling/lookup/hit/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/lookup/hit/1000000: Collecting 10 samples in estimated 5.0020 s (11k iterations)
Benchmarking utxo_scaling/lookup/hit/1000000: Analyzing
utxo_scaling/lookup/hit/1000000
                        time:   [439.60 µs 440.40 µs 441.83 µs]
                        change: [-0.4521% -0.1684% +0.1303%] (p = 0.30 > 0.05)
                        No change in performance detected.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) high mild
  1 (10.00%) high severe

Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Collecting 10 samples in estimated 6.4573 s (30 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/100000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/100000
                        time:   [10.568 ms 11.310 ms 12.284 ms]
                        change: [-6.0490% +0.4701% +9.8986%] (p = 0.92 > 0.05)
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 12.4s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Collecting 10 samples in estimated 12.354 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/500000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/500000
                        time:   [130.94 ms 138.28 ms 145.13 ms]
                        change: [+12.934% +20.810% +29.753%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 26.5s.
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Collecting 10 samples in estimated 26.484 s (10 iterations)
Benchmarking utxo_scaling/apply_tx/block_50tx_3in_2out/1000000: Analyzing
utxo_scaling/apply_tx/block_50tx_3in_2out/1000000
                        time:   [288.80 ms 295.49 ms 302.74 ms]
                        change: [+9.0315% +12.964% +17.512%] (p = 0.00 < 0.05)
                        Performance has regressed.

Benchmarking utxo_scaling/total_lovelace/scan/100000
Benchmarking utxo_scaling/total_lovelace/scan/100000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/100000: Collecting 10 samples in estimated 6.5988 s (220 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/100000: Analyzing
utxo_scaling/total_lovelace/scan/100000
                        time:   [30.253 ms 30.456 ms 30.575 ms]
                        change: [+4.1441% +4.8417% +5.4604%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_scaling/total_lovelace/scan/500000
Benchmarking utxo_scaling/total_lovelace/scan/500000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.2s or enable flat sampling.
Benchmarking utxo_scaling/total_lovelace/scan/500000: Collecting 10 samples in estimated 8.2135 s (55 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/500000: Analyzing
utxo_scaling/total_lovelace/scan/500000
                        time:   [149.51 ms 149.97 ms 150.36 ms]
                        change: [+4.5078% +5.0383% +5.5569%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 3 outliers among 10 measurements (30.00%)
  1 (10.00%) low severe
  2 (20.00%) high severe
Benchmarking utxo_scaling/total_lovelace/scan/1000000
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Warming up for 3.0000 s
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Collecting 10 samples in estimated 6.5588 s (20 iterations)
Benchmarking utxo_scaling/total_lovelace/scan/1000000: Analyzing
utxo_scaling/total_lovelace/scan/1000000
                        time:   [320.50 ms 330.66 ms 339.62 ms]
                        change: [+6.8183% +10.444% +13.467%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low severe
  1 (10.00%) high severe

Benchmarking utxo_large_scale/insert/default/5000000
Benchmarking utxo_large_scale/insert/default/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 161.2s.
Benchmarking utxo_large_scale/insert/default/5000000: Collecting 10 samples in estimated 161.24 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/5000000: Analyzing
utxo_large_scale/insert/default/5000000
                        time:   [16.088 s 16.126 s 16.164 s]
                        change: [+6.3896% +6.7533% +7.1065%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_large_scale/insert/default/10000000
Benchmarking utxo_large_scale/insert/default/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 355.9s.
Benchmarking utxo_large_scale/insert/default/10000000: Collecting 10 samples in estimated 355.88 s (10 iterations)
Benchmarking utxo_large_scale/insert/default/10000000: Analyzing
utxo_large_scale/insert/default/10000000
                        time:   [34.240 s 34.345 s 34.458 s]
                        change: [+7.0429% +7.4726% +7.8713%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_large_scale/lookup/hit/5000000
Benchmarking utxo_large_scale/lookup/hit/5000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/5000000: Collecting 10 samples in estimated 5.0578 s (3905 iterations)
Benchmarking utxo_large_scale/lookup/hit/5000000: Analyzing
utxo_large_scale/lookup/hit/5000000
                        time:   [1.2992 ms 1.3003 ms 1.3017 ms]
                        change: [+5.0217% +5.6045% +6.2158%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking utxo_large_scale/lookup/hit/10000000
Benchmarking utxo_large_scale/lookup/hit/10000000: Warming up for 3.0000 s
Benchmarking utxo_large_scale/lookup/hit/10000000: Collecting 10 samples in estimated 5.0700 s (3135 iterations)
Benchmarking utxo_large_scale/lookup/hit/10000000: Analyzing
utxo_large_scale/lookup/hit/10000000
                        time:   [1.6048 ms 1.6094 ms 1.6159 ms]
                        change: [+6.4534% +7.0515% +7.7570%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild

Benchmarking utxo_large_scale/total_lovelace/scan/5000000
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 15.5s.
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Collecting 10 samples in estimated 15.526 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/5000000: Analyzing
utxo_large_scale/total_lovelace/scan/5000000
                        time:   [1.5930 s 1.6177 s 1.6581 s]
                        change: [+4.5112% +6.1473% +8.5987%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking utxo_large_scale/total_lovelace/scan/10000000
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 30.4s.
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Collecting 10 samples in estimated 30.364 s (10 iterations)
Benchmarking utxo_large_scale/total_lovelace/scan/10000000: Analyzing
utxo_large_scale/total_lovelace/scan/10000000
                        time:   [3.1763 s 3.1881 s 3.1999 s]
                        change: [+2.7984% +3.7843% +4.6547%] (p = 0.00 < 0.05)
                        Performance has regressed.

```

## LSM Stress Tests
```
[1m[92m   Compiling[0m dugite-lsm v0.4.6-alpha (/home/runner/work/dugite/dugite/crates/dugite-lsm)
[1m[92m    Finished[0m `release` profile [optimized] target(s) in 10.34s
[1m[92m     Running[0m unittests src/lib.rs (target/release/deps/dugite_lsm-e9460fbb829b3490)

running 3 tests
test tree::mainnet_scale_tests::test_mainnet_scale_wal_crash_recovery ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_insert_read ... ok
test tree::mainnet_scale_tests::test_mainnet_scale_delete_amplification ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 93 filtered out; finished in 8.34s

[1m[92m   Doc-tests[0m dugite_lsm

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

```
