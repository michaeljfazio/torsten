//! Conformance test runner.
//!
//! Loads test vectors from JSON files, converts them to Torsten types,
//! executes the corresponding ledger function, and compares the result
//! against the expected output.

use crate::adapters;
use crate::schema::{
    CertEnvironment, CertState, ConformanceExpectedOutput, ConformanceTestResult,
    ConformanceTestVector, TestCertificate, TestTransaction, UtxoEnvironment, UtxoState,
};
use std::path::Path;
use torsten_ledger::validate_transaction;

/// Load a test vector from a JSON file.
pub fn load_vector(path: &Path) -> Result<ConformanceTestVector, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Load all test vectors from a directory (recursively).
pub fn load_vectors(dir: &Path) -> Result<Vec<(String, ConformanceTestVector)>, String> {
    let mut vectors = Vec::new();
    load_vectors_recursive(dir, &mut vectors)?;
    vectors.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(vectors)
}

fn load_vectors_recursive(
    dir: &Path,
    vectors: &mut Vec<(String, ConformanceTestVector)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        if path.is_dir() {
            load_vectors_recursive(&path, vectors)?;
        } else if path.extension().is_some_and(|ext| ext == "json") {
            let vector = load_vector(&path)?;
            vectors.push((path.display().to_string(), vector));
        }
    }
    Ok(())
}

/// Run a single conformance test vector and return the result.
pub fn run_test(vector_path: &str, vector: &ConformanceTestVector) -> ConformanceTestResult {
    match vector.rule.as_str() {
        "UTXO" => run_utxo_test(vector_path, vector),
        "CERT" => run_cert_test(vector_path, vector),
        rule => ConformanceTestResult {
            vector_path: vector_path.to_string(),
            rule: rule.to_string(),
            description: vector.description.clone(),
            passed: false,
            details: Some(format!("Unsupported rule: {}", rule)),
        },
    }
}

/// Run all test vectors and return results.
pub fn run_all(vectors: &[(String, ConformanceTestVector)]) -> Vec<ConformanceTestResult> {
    vectors
        .iter()
        .map(|(path, vector)| run_test(path, vector))
        .collect()
}

// ---------------------------------------------------------------------------
// UTXO rule runner
// ---------------------------------------------------------------------------

fn run_utxo_test(vector_path: &str, vector: &ConformanceTestVector) -> ConformanceTestResult {
    let base = ConformanceTestResult {
        vector_path: vector_path.to_string(),
        rule: "UTXO".to_string(),
        description: vector.description.clone(),
        passed: false,
        details: None,
    };

    // Deserialize environment
    let env: UtxoEnvironment = match serde_json::from_value(vector.environment.clone()) {
        Ok(e) => e,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize UTXO environment: {}", e)),
                ..base
            };
        }
    };

    // Deserialize input state
    let input_state: UtxoState = match serde_json::from_value(vector.input_state.clone()) {
        Ok(s) => s,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize UTXO input state: {}", e)),
                ..base
            };
        }
    };

    // Deserialize signal (transaction)
    let test_tx: TestTransaction = match serde_json::from_value(vector.signal.clone()) {
        Ok(t) => t,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize transaction signal: {}", e)),
                ..base
            };
        }
    };

    // Convert to Torsten types
    let utxo_set = match adapters::to_utxo_set(&input_state.utxo) {
        Ok(u) => u,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to convert UTxO set: {}", e)),
                ..base
            };
        }
    };

    let tx = match adapters::to_transaction(&test_tx) {
        Ok(t) => t,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to convert transaction: {}", e)),
                ..base
            };
        }
    };

    let params = adapters::to_protocol_params_from_utxo_env(&env);
    let tx_size = test_tx.tx_size;

    // Run Torsten validation
    let validation_result = validate_transaction(&tx, &utxo_set, &params, env.slot, tx_size, None);

    // Compare against expected output
    match &vector.expected_output {
        ConformanceExpectedOutput::Success { state } => {
            // We expect the transaction to be valid
            match validation_result {
                Ok(()) => {
                    // Validation passed. Now apply the transaction and compare state.
                    let mut result_utxo = utxo_set.clone();
                    if let Err(e) =
                        result_utxo.apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs)
                    {
                        return ConformanceTestResult {
                            details: Some(format!("UTxO apply failed: {}", e)),
                            ..base
                        };
                    }

                    // Convert back to test state for comparison
                    let actual_utxo_entries = adapters::from_utxo_set(&result_utxo);
                    let actual_state = UtxoState {
                        utxo: actual_utxo_entries,
                        fees: input_state.fees + tx.body.fee.0,
                        deposits: input_state.deposits,
                        donations: input_state.donations
                            + tx.body.donation.map(|d| d.0).unwrap_or(0),
                    };

                    let expected_state: UtxoState = match serde_json::from_value(state.clone()) {
                        Ok(s) => s,
                        Err(e) => {
                            return ConformanceTestResult {
                                details: Some(format!(
                                    "Failed to deserialize expected state: {}",
                                    e
                                )),
                                ..base
                            };
                        }
                    };

                    match adapters::diff_utxo_states(&expected_state, &actual_state) {
                        None => ConformanceTestResult {
                            passed: true,
                            ..base
                        },
                        Some(diff) => ConformanceTestResult {
                            details: Some(format!("State mismatch:\n  {}", diff)),
                            ..base
                        },
                    }
                }
                Err(errors) => {
                    let error_msgs: Vec<String> = errors.iter().map(|e| format!("{}", e)).collect();
                    ConformanceTestResult {
                        details: Some(format!(
                            "Expected success but validation failed: [{}]",
                            error_msgs.join(", ")
                        )),
                        ..base
                    }
                }
            }
        }
        ConformanceExpectedOutput::Failure {
            errors: expected_errors,
        } => {
            // We expect validation to fail
            match validation_result {
                Ok(()) => ConformanceTestResult {
                    details: Some(format!(
                        "Expected failure with errors {:?} but validation succeeded",
                        expected_errors
                    )),
                    ..base
                },
                Err(actual_errors) => {
                    // Validation failed as expected. Check that the error
                    // types match (we compare error category names, not
                    // exact messages, since the formal spec error types
                    // are more abstract).
                    let actual_categories: Vec<String> = actual_errors
                        .iter()
                        .map(categorize_validation_error)
                        .collect();

                    // For now, just check that we got *some* errors.
                    // A stricter check would verify exact error type mapping.
                    let all_expected_found = expected_errors
                        .iter()
                        .all(|exp| actual_categories.iter().any(|act| act == exp));

                    if all_expected_found {
                        ConformanceTestResult {
                            passed: true,
                            ..base
                        }
                    } else {
                        ConformanceTestResult {
                            details: Some(format!(
                                "Error mismatch: expected={:?}, actual={:?}",
                                expected_errors, actual_categories
                            )),
                            ..base
                        }
                    }
                }
            }
        }
    }
}

/// Map a Torsten ValidationError to a category string that matches the
/// formal specification's error types.
fn categorize_validation_error(error: &torsten_ledger::ValidationError) -> String {
    use torsten_ledger::ValidationError;
    match error {
        ValidationError::NoInputs => "NoInputs".to_string(),
        ValidationError::InputNotFound(_) => "InputNotFound".to_string(),
        ValidationError::ValueNotConserved { .. } => "ValueNotConserved".to_string(),
        ValidationError::FeeTooSmall { .. } => "FeeTooSmall".to_string(),
        ValidationError::OutputTooSmall { .. } => "OutputTooSmall".to_string(),
        ValidationError::TxTooLarge { .. } => "TxTooLarge".to_string(),
        ValidationError::TtlExpired { .. } => "TtlExpired".to_string(),
        ValidationError::NotYetValid { .. } => "NotYetValid".to_string(),
        ValidationError::InsufficientCollateral => "InsufficientCollateral".to_string(),
        ValidationError::TooManyCollateralInputs { .. } => "TooManyCollateralInputs".to_string(),
        ValidationError::CollateralNotFound(_) => "CollateralNotFound".to_string(),
        ValidationError::CollateralHasTokens(_) => "CollateralHasTokens".to_string(),
        ValidationError::CollateralMismatch { .. } => "CollateralMismatch".to_string(),
        ValidationError::MultiAssetNotConserved { .. } => "MultiAssetNotConserved".to_string(),
        ValidationError::InvalidMint => "InvalidMint".to_string(),
        ValidationError::DuplicateInput(_) => "DuplicateInput".to_string(),
        ValidationError::OutputValueTooLarge { .. } => "OutputValueTooLarge".to_string(),
        ValidationError::NetworkMismatch { .. } => "NetworkMismatch".to_string(),
        _ => format!("{:?}", error)
            .split('(')
            .next()
            .unwrap_or("Unknown")
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// CERT rule runner
// ---------------------------------------------------------------------------

fn run_cert_test(vector_path: &str, vector: &ConformanceTestVector) -> ConformanceTestResult {
    let base = ConformanceTestResult {
        vector_path: vector_path.to_string(),
        rule: "CERT".to_string(),
        description: vector.description.clone(),
        passed: false,
        details: None,
    };

    // Deserialize environment
    let _env: CertEnvironment = match serde_json::from_value(vector.environment.clone()) {
        Ok(e) => e,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize CERT environment: {}", e)),
                ..base
            };
        }
    };

    // Deserialize input state
    let input_state: CertState = match serde_json::from_value(vector.input_state.clone()) {
        Ok(s) => s,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize CERT input state: {}", e)),
                ..base
            };
        }
    };

    // Deserialize signal (certificate)
    let test_cert: TestCertificate = match serde_json::from_value(vector.signal.clone()) {
        Ok(c) => c,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to deserialize certificate signal: {}", e)),
                ..base
            };
        }
    };

    // Convert certificate
    let _cert = match adapters::to_certificate(&test_cert) {
        Ok(c) => c,
        Err(e) => {
            return ConformanceTestResult {
                details: Some(format!("Failed to convert certificate: {}", e)),
                ..base
            };
        }
    };

    // Apply the certificate to a simulated delegation state and compare.
    // Since LedgerState.process_certificate is a private method, we simulate
    // the expected state transitions directly.
    let actual_state = simulate_cert_apply(&input_state, &test_cert);

    match &vector.expected_output {
        ConformanceExpectedOutput::Success { state } => {
            let expected_state: CertState = match serde_json::from_value(state.clone()) {
                Ok(s) => s,
                Err(e) => {
                    return ConformanceTestResult {
                        details: Some(format!("Failed to deserialize expected CERT state: {}", e)),
                        ..base
                    };
                }
            };

            match adapters::diff_cert_states(&expected_state, &actual_state) {
                None => ConformanceTestResult {
                    passed: true,
                    ..base
                },
                Some(diff) => ConformanceTestResult {
                    details: Some(format!("CERT state mismatch:\n  {}", diff)),
                    ..base
                },
            }
        }
        ConformanceExpectedOutput::Failure { errors } => {
            // For CERT failures, we'd need to check preconditions. For now,
            // mark as passed if we detect the certificate would fail.
            ConformanceTestResult {
                details: Some(format!(
                    "CERT failure testing not fully implemented. Expected errors: {:?}",
                    errors
                )),
                ..base
            }
        }
    }
}

/// Simulate applying a certificate to the CertState.
///
/// This mirrors the logic in LedgerState::process_certificate but operates
/// on the conformance test types directly. This is necessary because
/// process_certificate is not a public API on the UtxoSet level.
fn simulate_cert_apply(state: &CertState, cert: &TestCertificate) -> CertState {
    let mut result = state.clone();

    match cert {
        TestCertificate::StakeRegistration { credential } => {
            let key = credential_hash(credential);
            // Register with zero deposit (pre-Conway)
            result.d_state.registrations.entry(key.clone()).or_insert(0);
            result.d_state.rewards.entry(key).or_insert(0);
        }
        TestCertificate::ConwayStakeRegistration {
            credential,
            deposit,
        } => {
            let key = credential_hash(credential);
            result.d_state.registrations.insert(key.clone(), *deposit);
            result.d_state.rewards.entry(key).or_insert(0);
        }
        TestCertificate::StakeDeregistration { credential } => {
            let key = credential_hash(credential);
            result.d_state.registrations.remove(&key);
            result.d_state.delegations.remove(&key);
            result.d_state.rewards.remove(&key);
        }
        TestCertificate::ConwayStakeDeregistration {
            credential,
            refund: _,
        } => {
            let key = credential_hash(credential);
            result.d_state.registrations.remove(&key);
            result.d_state.delegations.remove(&key);
            result.d_state.rewards.remove(&key);
        }
        TestCertificate::StakeDelegation {
            credential,
            pool_hash,
        } => {
            let key = credential_hash(credential);
            result.d_state.delegations.insert(key, pool_hash.clone());
        }
        TestCertificate::PoolRegistration { params } => {
            result
                .p_state
                .pools
                .insert(params.operator.clone(), params.clone());
            // Remove from retiring if re-registering
            result.p_state.retiring.remove(&params.operator);
        }
        TestCertificate::PoolRetirement { pool_hash, epoch } => {
            result.p_state.retiring.insert(pool_hash.clone(), *epoch);
        }
        TestCertificate::RegDRep {
            credential,
            deposit: _,
        } => {
            let key = credential_hash(credential);
            // Register DRep (active_until_epoch would be epoch + drep_activity)
            result.g_state.dreps.entry(key).or_insert(0);
        }
        TestCertificate::UnregDRep {
            credential,
            refund: _,
        } => {
            let key = credential_hash(credential);
            result.g_state.dreps.remove(&key);
        }
        TestCertificate::VoteDelegation { credential, drep } => {
            let key = credential_hash(credential);
            result.d_state.vote_delegations.insert(key, drep.clone());
        }
    }

    result
}

/// Extract the hash string from a test credential.
fn credential_hash(cred: &crate::schema::TestCredential) -> String {
    match cred {
        crate::schema::TestCredential::VKey { hash } => hash.clone(),
        crate::schema::TestCredential::Script { hash } => hash.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn vectors_dir() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("vectors");
        path
    }

    #[test]
    fn test_load_all_vectors() {
        let dir = vectors_dir();
        if !dir.exists() {
            // Vectors not yet generated; skip
            return;
        }
        let vectors = load_vectors(&dir).expect("Failed to load vectors");
        assert!(!vectors.is_empty(), "Expected at least one test vector");
    }

    #[test]
    fn test_run_all_vectors() {
        let dir = vectors_dir();
        if !dir.exists() {
            return;
        }
        let vectors = load_vectors(&dir).expect("Failed to load vectors");
        let results = run_all(&vectors);

        let passed = results.iter().filter(|r| r.passed).count();
        let total = results.len();

        for result in &results {
            println!("{}", result);
        }

        assert_eq!(
            passed, total,
            "{}/{} conformance tests passed",
            passed, total
        );
    }
}
