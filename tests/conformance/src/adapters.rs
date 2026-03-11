//! Adapters that convert between conformance test vector types and Torsten types.
//!
//! The formal Agda specification uses abstract/simplified types (e.g., addresses
//! as tagged enums, TxId as hex strings). This module bridges the gap by
//! converting the JSON-deserialized test types into Torsten's concrete types,
//! and converting Torsten outputs back into comparable test types.

use crate::schema::{
    CertState, PoolSubState, TestAddress, TestCertificate, TestCredential, TestDRep, TestInput,
    TestPoolParams, TestTransaction, TxOutput, UtxoEntry, UtxoEnvironment, UtxoState,
};
use std::collections::{BTreeMap, HashSet};
use torsten_ledger::UtxoSet;
use torsten_primitives::address::{
    Address, BaseAddress, ByronAddress, EnterpriseAddress, RewardAddress,
};
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::network::NetworkId;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::{
    self, Certificate, ExUnits, Rational, Transaction, TransactionBody, TransactionInput,
    TransactionOutput, TransactionWitnessSet,
};
use torsten_primitives::value::{AssetName, Lovelace, Value};

/// Error type for adapter conversions.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("Invalid hex string: {0}")]
    InvalidHex(String),
    #[error("Invalid hash length: expected {expected}, got {actual}")]
    InvalidHashLength { expected: usize, actual: usize },
    #[error("Unknown credential type: {0}")]
    UnknownCredentialType(String),
    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

// ---------------------------------------------------------------------------
// Hex / Hash helpers
// ---------------------------------------------------------------------------

/// Parse a hex-encoded 32-byte hash.
pub fn parse_hash32(hex_str: &str) -> Result<Hash32, AdapterError> {
    Hash32::from_hex(hex_str).map_err(|_| AdapterError::InvalidHex(hex_str.to_string()))
}

/// Parse a hex-encoded 28-byte hash.
pub fn parse_hash28(hex_str: &str) -> Result<Hash28, AdapterError> {
    Hash28::from_hex(hex_str).map_err(|_| AdapterError::InvalidHex(hex_str.to_string()))
}

// ---------------------------------------------------------------------------
// Credential conversion
// ---------------------------------------------------------------------------

/// Convert a test credential to a Torsten credential.
pub fn to_credential(tc: &TestCredential) -> Result<Credential, AdapterError> {
    match tc {
        TestCredential::VKey { hash } => {
            let h = parse_hash28(hash)?;
            Ok(Credential::VerificationKey(h))
        }
        TestCredential::Script { hash } => {
            let h = parse_hash28(hash)?;
            Ok(Credential::Script(h))
        }
    }
}

/// Convert a Torsten credential to a test credential.
pub fn from_credential(cred: &Credential) -> TestCredential {
    match cred {
        Credential::VerificationKey(h) => TestCredential::VKey { hash: h.to_hex() },
        Credential::Script(h) => TestCredential::Script { hash: h.to_hex() },
    }
}

// ---------------------------------------------------------------------------
// Address conversion
// ---------------------------------------------------------------------------

fn network_from_u8(n: u8) -> NetworkId {
    if n == 1 {
        NetworkId::Mainnet
    } else {
        NetworkId::Testnet
    }
}

fn network_to_u8(n: NetworkId) -> u8 {
    match n {
        NetworkId::Mainnet => 1,
        NetworkId::Testnet => 0,
    }
}

/// Convert a test address to a Torsten address.
pub fn to_address(addr: &TestAddress) -> Result<Address, AdapterError> {
    match addr {
        TestAddress::Base {
            network,
            payment,
            stake,
        } => Ok(Address::Base(BaseAddress {
            network: network_from_u8(*network),
            payment: to_credential(payment)?,
            stake: to_credential(stake)?,
        })),
        TestAddress::Enterprise { network, payment } => {
            Ok(Address::Enterprise(EnterpriseAddress {
                network: network_from_u8(*network),
                payment: to_credential(payment)?,
            }))
        }
        TestAddress::Reward { network, stake } => Ok(Address::Reward(RewardAddress {
            network: network_from_u8(*network),
            stake: to_credential(stake)?,
        })),
        TestAddress::Byron { payload_hex } => {
            let payload = hex::decode(payload_hex)
                .map_err(|_| AdapterError::InvalidHex(payload_hex.clone()))?;
            Ok(Address::Byron(ByronAddress { payload }))
        }
    }
}

/// Convert a Torsten address to a test address.
pub fn from_address(addr: &Address) -> TestAddress {
    match addr {
        Address::Base(b) => TestAddress::Base {
            network: network_to_u8(b.network),
            payment: from_credential(&b.payment),
            stake: from_credential(&b.stake),
        },
        Address::Enterprise(e) => TestAddress::Enterprise {
            network: network_to_u8(e.network),
            payment: from_credential(&e.payment),
        },
        Address::Reward(r) => TestAddress::Reward {
            network: network_to_u8(r.network),
            stake: from_credential(&r.stake),
        },
        Address::Byron(b) => TestAddress::Byron {
            payload_hex: hex::encode(&b.payload),
        },
        Address::Pointer(p) => {
            // Pointer addresses are rare in test vectors; map to enterprise
            TestAddress::Enterprise {
                network: network_to_u8(p.network),
                payment: from_credential(&p.payment),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Convert a test output to a Torsten transaction output.
pub fn to_tx_output(out: &TxOutput) -> Result<TransactionOutput, AdapterError> {
    let address = to_address(&out.address)?;

    let mut multi_asset = BTreeMap::new();
    for (policy_hex, assets) in &out.assets {
        let policy_id = parse_hash28(policy_hex)?;
        let mut asset_map = BTreeMap::new();
        for (name_hex, qty) in assets {
            let name_bytes =
                hex::decode(name_hex).map_err(|_| AdapterError::InvalidHex(name_hex.clone()))?;
            let asset_name = AssetName(name_bytes);
            // UTxO output quantities are always non-negative; cast safely
            asset_map.insert(asset_name, (*qty).max(0) as u64);
        }
        multi_asset.insert(policy_id, asset_map);
    }

    let value = if multi_asset.is_empty() {
        Value::lovelace(out.lovelace)
    } else {
        Value {
            coin: Lovelace(out.lovelace),
            multi_asset,
        }
    };

    Ok(TransactionOutput {
        address,
        value,
        datum: transaction::OutputDatum::None,
        script_ref: None,
        raw_cbor: None,
    })
}

/// Convert a Torsten transaction output to a test output.
pub fn from_tx_output(out: &TransactionOutput) -> TxOutput {
    let mut assets = BTreeMap::new();
    for (policy_id, asset_map) in &out.value.multi_asset {
        let mut inner = BTreeMap::new();
        for (name, qty) in asset_map {
            inner.insert(hex::encode(&name.0), *qty as i64);
        }
        assets.insert(policy_id.to_hex(), inner);
    }

    TxOutput {
        address: from_address(&out.address),
        lovelace: out.value.coin.0,
        assets,
    }
}

// ---------------------------------------------------------------------------
// UTxO set conversion
// ---------------------------------------------------------------------------

/// Build a Torsten UTxO set from test vector entries.
pub fn to_utxo_set(entries: &[UtxoEntry]) -> Result<UtxoSet, AdapterError> {
    let mut utxo_set = UtxoSet::new();
    for entry in entries {
        let tx_hash = parse_hash32(&entry.tx_hash)?;
        let input = TransactionInput {
            transaction_id: tx_hash,
            index: entry.index,
        };
        let output = to_tx_output(&entry.output)?;
        utxo_set.insert(input, output);
    }
    Ok(utxo_set)
}

/// Convert a Torsten UTxO set to test vector entries.
pub fn from_utxo_set(utxo_set: &UtxoSet) -> Vec<UtxoEntry> {
    let mut entries: Vec<UtxoEntry> = utxo_set
        .iter()
        .map(|(input, output)| UtxoEntry {
            tx_hash: input.transaction_id.to_hex(),
            index: input.index,
            output: from_tx_output(output),
        })
        .collect();
    // Sort for deterministic comparison
    entries.sort_by(|a, b| a.tx_hash.cmp(&b.tx_hash).then(a.index.cmp(&b.index)));
    entries
}

// ---------------------------------------------------------------------------
// Transaction conversion
// ---------------------------------------------------------------------------

/// Convert a test transaction to a Torsten transaction.
pub fn to_transaction(tx: &TestTransaction) -> Result<Transaction, AdapterError> {
    let hash = parse_hash32(&tx.hash)?;

    let inputs: Vec<TransactionInput> = tx
        .inputs
        .iter()
        .map(to_tx_input)
        .collect::<Result<_, _>>()?;

    let outputs: Vec<TransactionOutput> = tx
        .outputs
        .iter()
        .map(to_tx_output)
        .collect::<Result<_, _>>()?;

    let certificates: Vec<Certificate> = tx
        .certificates
        .iter()
        .map(to_certificate)
        .collect::<Result<_, _>>()?;

    let mut withdrawals = BTreeMap::new();
    for (addr_hex, amount) in &tx.withdrawals {
        let addr_bytes =
            hex::decode(addr_hex).map_err(|_| AdapterError::InvalidHex(addr_hex.clone()))?;
        withdrawals.insert(addr_bytes, Lovelace(*amount));
    }

    let mut mint = BTreeMap::new();
    for (policy_hex, assets) in &tx.mint {
        let policy_id = parse_hash28(policy_hex)?;
        let mut asset_map = BTreeMap::new();
        for (name_hex, qty) in assets {
            let name_bytes =
                hex::decode(name_hex).map_err(|_| AdapterError::InvalidHex(name_hex.clone()))?;
            let asset_name = AssetName(name_bytes);
            asset_map.insert(asset_name, *qty);
        }
        mint.insert(policy_id, asset_map);
    }

    let collateral: Vec<TransactionInput> = tx
        .collateral
        .iter()
        .map(to_tx_input)
        .collect::<Result<_, _>>()?;

    let required_signers: Vec<Hash32> = tx
        .required_signers
        .iter()
        .map(|h| parse_hash32(h))
        .collect::<Result<_, _>>()?;

    let reference_inputs: Vec<TransactionInput> = tx
        .reference_inputs
        .iter()
        .map(to_tx_input)
        .collect::<Result<_, _>>()?;

    let collateral_return = match &tx.collateral_return {
        Some(out) => Some(to_tx_output(out)?),
        None => None,
    };

    let body = TransactionBody {
        inputs,
        outputs,
        fee: Lovelace(tx.fee),
        ttl: tx.ttl.map(torsten_primitives::time::SlotNo),
        certificates,
        withdrawals,
        auxiliary_data_hash: None,
        validity_interval_start: tx.validity_start.map(torsten_primitives::time::SlotNo),
        mint,
        script_data_hash: None,
        collateral,
        required_signers,
        network_id: None,
        collateral_return,
        total_collateral: tx.total_collateral.map(Lovelace),
        reference_inputs,
        update: None,
        voting_procedures: BTreeMap::new(),
        proposal_procedures: Vec::new(),
        treasury_value: None,
        donation: tx.donation.map(Lovelace),
    };

    Ok(Transaction {
        hash,
        body,
        witness_set: TransactionWitnessSet {
            vkey_witnesses: Vec::new(),
            native_scripts: Vec::new(),
            bootstrap_witnesses: Vec::new(),
            plutus_v1_scripts: Vec::new(),
            plutus_v2_scripts: Vec::new(),
            plutus_v3_scripts: Vec::new(),
            plutus_data: Vec::new(),
            redeemers: Vec::new(),
        },
        is_valid: tx.is_valid,
        auxiliary_data: None,
        raw_cbor: None,
    })
}

fn to_tx_input(input: &TestInput) -> Result<TransactionInput, AdapterError> {
    let tx_hash = parse_hash32(&input.tx_hash)?;
    Ok(TransactionInput {
        transaction_id: tx_hash,
        index: input.index,
    })
}

// ---------------------------------------------------------------------------
// Certificate conversion
// ---------------------------------------------------------------------------

/// Convert a test certificate to a Torsten certificate.
pub fn to_certificate(cert: &TestCertificate) -> Result<Certificate, AdapterError> {
    match cert {
        TestCertificate::StakeRegistration { credential } => {
            Ok(Certificate::StakeRegistration(to_credential(credential)?))
        }
        TestCertificate::StakeDeregistration { credential } => {
            Ok(Certificate::StakeDeregistration(to_credential(credential)?))
        }
        TestCertificate::StakeDelegation {
            credential,
            pool_hash,
        } => {
            let cred = to_credential(credential)?;
            let pool = parse_hash28(pool_hash)?;
            Ok(Certificate::StakeDelegation {
                credential: cred,
                pool_hash: pool,
            })
        }
        TestCertificate::PoolRegistration { params } => {
            let pool_params = to_pool_params(params)?;
            Ok(Certificate::PoolRegistration(pool_params))
        }
        TestCertificate::PoolRetirement { pool_hash, epoch } => {
            let pool = parse_hash28(pool_hash)?;
            Ok(Certificate::PoolRetirement {
                pool_hash: pool,
                epoch: *epoch,
            })
        }
        TestCertificate::RegDRep {
            credential,
            deposit,
        } => Ok(Certificate::RegDRep {
            credential: to_credential(credential)?,
            deposit: Lovelace(*deposit),
            anchor: None,
        }),
        TestCertificate::UnregDRep { credential, refund } => Ok(Certificate::UnregDRep {
            credential: to_credential(credential)?,
            refund: Lovelace(*refund),
        }),
        TestCertificate::VoteDelegation { credential, drep } => Ok(Certificate::VoteDelegation {
            credential: to_credential(credential)?,
            drep: to_drep(drep)?,
        }),
        TestCertificate::ConwayStakeRegistration {
            credential,
            deposit,
        } => Ok(Certificate::ConwayStakeRegistration {
            credential: to_credential(credential)?,
            deposit: Lovelace(*deposit),
        }),
        TestCertificate::ConwayStakeDeregistration { credential, refund } => {
            Ok(Certificate::ConwayStakeDeregistration {
                credential: to_credential(credential)?,
                refund: Lovelace(*refund),
            })
        }
    }
}

/// Convert a test DRep to a Torsten DRep.
pub fn to_drep(drep: &TestDRep) -> Result<transaction::DRep, AdapterError> {
    match drep {
        TestDRep::KeyHash { hash } => {
            let h = parse_hash32(hash)?;
            Ok(transaction::DRep::KeyHash(h))
        }
        TestDRep::ScriptHash { hash } => {
            let h = parse_hash28(hash)?;
            Ok(transaction::DRep::ScriptHash(h))
        }
        TestDRep::Abstain => Ok(transaction::DRep::Abstain),
        TestDRep::NoConfidence => Ok(transaction::DRep::NoConfidence),
    }
}

/// Convert a test pool params to a Torsten pool params.
pub fn to_pool_params(params: &TestPoolParams) -> Result<transaction::PoolParams, AdapterError> {
    let operator = parse_hash28(&params.operator)?;
    let vrf_keyhash = parse_hash32(&params.vrf_keyhash)?;
    let reward_account = hex::decode(&params.reward_account)
        .map_err(|_| AdapterError::InvalidHex(params.reward_account.clone()))?;
    let pool_owners: Vec<Hash28> = params
        .owners
        .iter()
        .map(|h| parse_hash28(h))
        .collect::<Result<_, _>>()?;

    Ok(transaction::PoolParams {
        operator,
        vrf_keyhash,
        pledge: Lovelace(params.pledge),
        cost: Lovelace(params.cost),
        margin: Rational {
            numerator: params.margin[0],
            denominator: params.margin[1],
        },
        reward_account,
        pool_owners,
        relays: Vec::new(),
        pool_metadata: None,
    })
}

// ---------------------------------------------------------------------------
// Protocol parameters conversion
// ---------------------------------------------------------------------------

/// Build Torsten ProtocolParameters from a UTXO environment.
///
/// Fills in required fields not present in the test vector with sensible
/// defaults (the formal spec only uses a subset of protocol parameters for
/// each rule).
pub fn to_protocol_params_from_utxo_env(env: &UtxoEnvironment) -> ProtocolParameters {
    make_protocol_params(&env.protocol_params)
}

fn make_protocol_params(pp: &crate::schema::UtxoProtocolParams) -> ProtocolParameters {
    ProtocolParameters {
        min_fee_a: pp.min_fee_a,
        min_fee_b: pp.min_fee_b,
        max_block_body_size: 90_112,
        max_tx_size: pp.max_tx_size,
        max_block_header_size: 1100,
        key_deposit: Lovelace(pp.key_deposit),
        pool_deposit: Lovelace(pp.pool_deposit),
        e_max: 18,
        n_opt: 500,
        a0: Rational {
            numerator: 3,
            denominator: 10,
        },
        rho: Rational {
            numerator: 3,
            denominator: 1000,
        },
        tau: Rational {
            numerator: 2,
            denominator: 10,
        },
        min_pool_cost: Lovelace(170_000_000),
        ada_per_utxo_byte: Lovelace(pp.ada_per_utxo_byte),
        cost_models: transaction::CostModels {
            plutus_v1: None,
            plutus_v2: None,
            plutus_v3: None,
        },
        execution_costs: transaction::ExUnitPrices {
            mem_price: Rational {
                numerator: 577,
                denominator: 10_000,
            },
            step_price: Rational {
                numerator: 721,
                denominator: 10_000_000,
            },
        },
        max_tx_ex_units: ExUnits {
            mem: pp.max_tx_ex_mem,
            steps: pp.max_tx_ex_steps,
        },
        max_block_ex_units: ExUnits {
            mem: 62_000_000_000,
            steps: 20_000_000_000_000,
        },
        max_val_size: pp.max_val_size,
        collateral_percentage: pp.collateral_percentage,
        max_collateral_inputs: pp.max_collateral_inputs,
        min_fee_ref_script_cost_per_byte: pp.min_fee_ref_script_cost_per_byte,
        drep_deposit: Lovelace(pp.drep_deposit),
        drep_activity: 20,
        gov_action_deposit: Lovelace(pp.gov_action_deposit),
        gov_action_lifetime: 10,
        committee_min_size: 0,
        committee_max_term_length: 200,
        dvt_pp_network_group: Rational {
            numerator: 67,
            denominator: 100,
        },
        dvt_pp_economic_group: Rational {
            numerator: 67,
            denominator: 100,
        },
        dvt_pp_technical_group: Rational {
            numerator: 67,
            denominator: 100,
        },
        dvt_pp_gov_group: Rational {
            numerator: 75,
            denominator: 100,
        },
        dvt_hard_fork: Rational {
            numerator: 6,
            denominator: 10,
        },
        dvt_no_confidence: Rational {
            numerator: 67,
            denominator: 100,
        },
        dvt_committee_normal: Rational {
            numerator: 67,
            denominator: 100,
        },
        dvt_committee_no_confidence: Rational {
            numerator: 6,
            denominator: 10,
        },
        dvt_constitution: Rational {
            numerator: 75,
            denominator: 100,
        },
        dvt_treasury_withdrawal: Rational {
            numerator: 67,
            denominator: 100,
        },
        pvt_motion_no_confidence: Rational {
            numerator: 51,
            denominator: 100,
        },
        pvt_committee_normal: Rational {
            numerator: 51,
            denominator: 100,
        },
        pvt_committee_no_confidence: Rational {
            numerator: 51,
            denominator: 100,
        },
        pvt_hard_fork: Rational {
            numerator: 51,
            denominator: 100,
        },
        pvt_pp_security_group: Rational {
            numerator: 51,
            denominator: 100,
        },
        protocol_version_major: 9,
        protocol_version_minor: 0,
        active_slots_coeff: 0.05,
    }
}

// ---------------------------------------------------------------------------
// UTxO state comparison
// ---------------------------------------------------------------------------

/// Compare two UtxoState values and return a human-readable diff.
///
/// Returns `None` if they are equivalent, or `Some(diff)` with details.
pub fn diff_utxo_states(expected: &UtxoState, actual: &UtxoState) -> Option<String> {
    let mut diffs = Vec::new();

    if expected.fees != actual.fees {
        diffs.push(format!(
            "fees: expected={}, actual={}",
            expected.fees, actual.fees
        ));
    }

    if expected.deposits != actual.deposits {
        diffs.push(format!(
            "deposits: expected={}, actual={}",
            expected.deposits, actual.deposits
        ));
    }

    if expected.donations != actual.donations {
        diffs.push(format!(
            "donations: expected={}, actual={}",
            expected.donations, actual.donations
        ));
    }

    // Compare UTxO entries
    let mut expected_map: BTreeMap<(String, u32), &TxOutput> = BTreeMap::new();
    for entry in &expected.utxo {
        expected_map.insert((entry.tx_hash.clone(), entry.index), &entry.output);
    }

    let mut actual_map: BTreeMap<(String, u32), &TxOutput> = BTreeMap::new();
    for entry in &actual.utxo {
        actual_map.insert((entry.tx_hash.clone(), entry.index), &entry.output);
    }

    // Find entries in expected but not in actual
    for (key, exp_output) in &expected_map {
        match actual_map.get(key) {
            None => {
                diffs.push(format!(
                    "missing UTxO: {}#{} (expected {} lovelace)",
                    key.0, key.1, exp_output.lovelace
                ));
            }
            Some(act_output) => {
                if exp_output.lovelace != act_output.lovelace {
                    diffs.push(format!(
                        "UTxO {}#{} lovelace: expected={}, actual={}",
                        key.0, key.1, exp_output.lovelace, act_output.lovelace
                    ));
                }
            }
        }
    }

    // Find entries in actual but not in expected
    for key in actual_map.keys() {
        if !expected_map.contains_key(key) {
            diffs.push(format!("extra UTxO: {}#{}", key.0, key.1));
        }
    }

    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join("\n  "))
    }
}

/// Compare two CertState values and return a human-readable diff.
pub fn diff_cert_states(expected: &CertState, actual: &CertState) -> Option<String> {
    let mut diffs = Vec::new();

    // Compare registrations
    if expected.d_state.registrations != actual.d_state.registrations {
        diffs.push(format!(
            "d_state.registrations: expected={:?}, actual={:?}",
            expected.d_state.registrations, actual.d_state.registrations
        ));
    }

    // Compare delegations
    if expected.d_state.delegations != actual.d_state.delegations {
        diffs.push(format!(
            "d_state.delegations: expected={:?}, actual={:?}",
            expected.d_state.delegations, actual.d_state.delegations
        ));
    }

    // Compare rewards
    if expected.d_state.rewards != actual.d_state.rewards {
        diffs.push(format!(
            "d_state.rewards: expected={:?}, actual={:?}",
            expected.d_state.rewards, actual.d_state.rewards
        ));
    }

    // Compare pools
    for (pool_id, expected_params) in &expected.p_state.pools {
        match actual.p_state.pools.get(pool_id) {
            None => {
                diffs.push(format!("missing pool: {}", pool_id));
            }
            Some(actual_params) => {
                if expected_params.operator != actual_params.operator {
                    diffs.push(format!(
                        "pool {} operator: expected={}, actual={}",
                        pool_id, expected_params.operator, actual_params.operator
                    ));
                }
                if expected_params.pledge != actual_params.pledge {
                    diffs.push(format!(
                        "pool {} pledge: expected={}, actual={}",
                        pool_id, expected_params.pledge, actual_params.pledge
                    ));
                }
            }
        }
    }

    for pool_id in actual.p_state.pools.keys() {
        if !expected.p_state.pools.contains_key(pool_id) {
            diffs.push(format!("extra pool: {}", pool_id));
        }
    }

    // Compare retirements
    if expected.p_state.retiring != actual.p_state.retiring {
        diffs.push(format!(
            "p_state.retiring: expected={:?}, actual={:?}",
            expected.p_state.retiring, actual.p_state.retiring
        ));
    }

    // Compare governance sub-state
    if expected.g_state.dreps != actual.g_state.dreps {
        diffs.push(format!(
            "g_state.dreps: expected={:?}, actual={:?}",
            expected.g_state.dreps, actual.g_state.dreps
        ));
    }

    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join("\n  "))
    }
}

/// Build a HashSet of registered pool IDs from a PoolSubState.
pub fn registered_pool_set(p_state: &PoolSubState) -> Result<HashSet<Hash28>, AdapterError> {
    let mut set = HashSet::new();
    for pool_id_hex in p_state.pools.keys() {
        set.insert(parse_hash28(pool_id_hex)?);
    }
    Ok(set)
}
