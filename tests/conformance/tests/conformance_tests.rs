//! Integration tests that run all conformance test vectors.
//!
//! These tests load JSON test vectors from the `vectors/` directory and
//! validate them against Torsten's ledger implementation.
//!
//! Run with: `cargo test -p torsten-conformance`

use std::path::PathBuf;
use torsten_conformance::runner;
use torsten_conformance::schema::ConformanceTestResult;

fn vectors_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("vectors");
    path
}

#[test]
fn conformance_utxo_vectors() {
    let dir = vectors_dir().join("utxo");
    let vectors = runner::load_vectors(&dir).expect("Failed to load UTXO test vectors");
    assert!(!vectors.is_empty(), "No UTXO test vectors found");

    let results = runner::run_all(&vectors);
    print_results(&results);

    let failed: Vec<&ConformanceTestResult> = results.iter().filter(|r| !r.passed).collect();
    assert!(
        failed.is_empty(),
        "{}/{} UTXO conformance tests failed",
        failed.len(),
        results.len()
    );
}

#[test]
fn conformance_cert_vectors() {
    let dir = vectors_dir().join("cert");
    let vectors = runner::load_vectors(&dir).expect("Failed to load CERT test vectors");
    assert!(!vectors.is_empty(), "No CERT test vectors found");

    let results = runner::run_all(&vectors);
    print_results(&results);

    let failed: Vec<&ConformanceTestResult> = results.iter().filter(|r| !r.passed).collect();
    assert!(
        failed.is_empty(),
        "{}/{} CERT conformance tests failed",
        failed.len(),
        results.len()
    );
}

#[test]
fn conformance_all_vectors() {
    let dir = vectors_dir();
    let vectors = runner::load_vectors(&dir).expect("Failed to load test vectors");
    assert!(!vectors.is_empty(), "No test vectors found");

    let results = runner::run_all(&vectors);
    print_results(&results);

    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();
    let failed: Vec<&ConformanceTestResult> = results.iter().filter(|r| !r.passed).collect();

    println!("\nConformance test summary: {}/{} passed", passed, total);

    assert!(
        failed.is_empty(),
        "{}/{} conformance tests failed",
        failed.len(),
        total
    );
}

fn print_results(results: &[ConformanceTestResult]) {
    for result in results {
        println!("{}", result);
    }
}
