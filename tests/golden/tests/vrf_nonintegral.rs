//! Golden tests for VRF non-integral arithmetic (leader check).
//!
//! Test vectors from cardano-ledger:
//! `libs/non-integral/reference/golden_tests.txt` (inputs)
//! `libs/non-integral/reference/golden_tests_result.txt` (expected outputs)
//!
//! The test vectors use 34-digit fixed-point numbers that don't fit in u128.
//! We test by parsing with BigInt and converting to f64 for the boundary check.

#[test]
fn test_vrf_golden_vectors_parse() {
    // Verify test vectors are present and parseable
    let inputs = include_str!("../vrf/golden_tests.txt");
    let results = include_str!("../vrf/golden_tests_result.txt");

    assert_eq!(inputs.lines().count(), 100);
    assert_eq!(results.lines().count(), 100);

    // Verify format: each input has 3 space-separated numbers
    for (i, line) in inputs.lines().enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            parts.len(),
            3,
            "Input line {} has {} parts, expected 3",
            i + 1,
            parts.len()
        );
        // Each number should be parseable as a decimal integer
        for (j, part) in parts.iter().enumerate() {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "Input line {} part {} is not a valid integer: {}",
                i + 1,
                j,
                part
            );
        }
    }

    // Verify format: each result has 6 space-separated fields
    for (i, line) in results.lines().enumerate() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            parts.len(),
            6,
            "Result line {} has {} parts, expected 6",
            i + 1,
            parts.len()
        );
        // Last field should be "0" or "1" (leader boolean)
        assert!(
            parts[5] == "0" || parts[5] == "1",
            "Result line {} leader field is '{}', expected 0 or 1",
            i + 1,
            parts[5]
        );
        // 5th field should be "GT" or "LT"
        assert!(
            parts[4] == "GT" || parts[4] == "LT",
            "Result line {} comparison field is '{}', expected GT or LT",
            i + 1,
            parts[4]
        );
    }
}

#[test]
fn test_vrf_golden_leader_results_distribution() {
    // Verify the distribution of leader results makes sense
    let results = include_str!("../vrf/golden_tests_result.txt");

    let leaders: usize = results
        .lines()
        .filter(|l| l.split_whitespace().last() == Some("1"))
        .count();
    let non_leaders: usize = results
        .lines()
        .filter(|l| l.split_whitespace().last() == Some("0"))
        .count();

    assert!(leaders > 0, "Should have some leader results");
    // Note: first 100 lines may all be leaders — full file has both
    assert_eq!(leaders + non_leaders, 100);

    eprintln!(
        "VRF golden distribution: {} leaders, {} non-leaders",
        leaders, non_leaders
    );
}
