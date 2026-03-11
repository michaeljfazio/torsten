use crate::error::SerializationError;
use pallas_traverse::MultiEraBlock as PallasBlock;
use pallas_traverse::MultiEraCert;
use pallas_traverse::MultiEraInput as PallasInput;
use pallas_traverse::MultiEraOutput as PallasOutput;
use pallas_traverse::MultiEraTx as PallasTx;
use pallas_traverse::MultiEraWithdrawals;
use std::collections::BTreeMap;
use torsten_primitives::address::Address;
use torsten_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::{Hash, Hash28, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_primitives::transaction::*;
use torsten_primitives::value::{AssetName, Lovelace, Value};

/// Decode a transaction from raw CBOR bytes.
///
/// The `era_id` corresponds to the Cardano era encoding:
/// 0 = Byron, 1 = Shelley, 2 = Allegra, 3 = Mary, 4 = Alonzo, 5 = Babbage, 6 = Conway
pub fn decode_transaction(era_id: u16, tx_cbor: &[u8]) -> Result<Transaction, SerializationError> {
    use pallas_traverse::Era as PallasEra;

    let pallas_era = match era_id {
        0 => PallasEra::Byron,
        1 => PallasEra::Shelley,
        2 => PallasEra::Allegra,
        3 => PallasEra::Mary,
        4 => PallasEra::Alonzo,
        5 => PallasEra::Babbage,
        6 => PallasEra::Conway,
        _ => {
            return Err(SerializationError::CborDecode(format!(
                "unknown era id: {era_id}"
            )))
        }
    };

    let pallas_tx = PallasTx::decode_for_era(pallas_era, tx_cbor)
        .map_err(|e| SerializationError::CborDecode(format!("tx decode: {e}")))?;

    decode_transaction_from_pallas(&pallas_tx)
}

/// Decode a multi-era block from raw CBOR bytes into a torsten Block.
pub fn decode_block(cbor: &[u8]) -> Result<Block, SerializationError> {
    decode_block_with_byron_epoch_length(cbor, 0)
}

/// Decode a multi-era block, using the given Byron epoch length (10*k) for
/// correct slot computation on non-mainnet networks. Pass 0 for mainnet.
pub fn decode_block_with_byron_epoch_length(
    cbor: &[u8],
    byron_epoch_length: u64,
) -> Result<Block, SerializationError> {
    let pallas_block = PallasBlock::decode(cbor)
        .map_err(|e| SerializationError::CborDecode(format!("block decode: {e}")))?;

    let era = convert_era(pallas_block.era());
    let header = decode_block_header(&pallas_block, byron_epoch_length)?;
    let transactions = pallas_block
        .txs()
        .iter()
        .map(decode_transaction_from_pallas)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Block {
        header,
        transactions,
        era,
        raw_cbor: Some(cbor.to_vec()),
    })
}

fn decode_block_header(
    block: &PallasBlock,
    byron_epoch_length: u64,
) -> Result<BlockHeader, SerializationError> {
    // For Byron blocks on non-mainnet networks, compute the correct absolute
    // slot from the raw epoch/relative-slot. Pallas hardcodes mainnet values
    // in GenesisValues::default() which gives wrong slots on other networks.
    let slot = if byron_epoch_length > 0 {
        if let Some(byron) = block.as_byron() {
            let epoch = byron.header.consensus_data.0.epoch;
            let rel_slot = byron.header.consensus_data.0.slot;
            SlotNo(epoch * byron_epoch_length + rel_slot)
        } else {
            SlotNo(block.slot())
        }
    } else {
        SlotNo(block.slot())
    };
    let block_number = BlockNo(block.number());
    let header_hash = pallas_hash_to_torsten32(&block.hash());
    let pallas_header = block.header();

    let prev_hash = pallas_header
        .previous_hash()
        .map(|h| pallas_hash_to_torsten32(&h))
        .unwrap_or(Hash32::ZERO);

    let issuer_vkey = pallas_header
        .issuer_vkey()
        .map(|v| v.to_vec())
        .unwrap_or_default();

    let vrf_vkey = pallas_header
        .vrf_vkey()
        .map(|v| v.to_vec())
        .unwrap_or_default();

    let body_size = block.body_size().unwrap_or(0) as u64;

    // Extract era-specific header body fields
    let (vrf_result, body_hash, op_cert, protocol_version, kes_signature) =
        if let Some(babbage) = pallas_header.as_babbage() {
            let hb = &babbage.header_body;
            (
                VrfOutput {
                    output: hb.vrf_result.0.to_vec(),
                    proof: hb.vrf_result.1.to_vec(),
                },
                pallas_hash_to_torsten32(&hb.block_body_hash),
                OperationalCert {
                    hot_vkey: hb.operational_cert.operational_cert_hot_vkey.to_vec(),
                    sequence_number: hb.operational_cert.operational_cert_sequence_number,
                    kes_period: hb.operational_cert.operational_cert_kes_period,
                    sigma: hb.operational_cert.operational_cert_sigma.to_vec(),
                },
                ProtocolVersion {
                    major: hb.protocol_version.0,
                    minor: hb.protocol_version.1,
                },
                babbage.body_signature.to_vec(),
            )
        } else if let Some(alonzo) = pallas_header.as_alonzo() {
            let hb = &alonzo.header_body;
            (
                VrfOutput {
                    output: hb.leader_vrf.0.to_vec(),
                    proof: hb.leader_vrf.1.to_vec(),
                },
                pallas_hash_to_torsten32(&hb.block_body_hash),
                OperationalCert {
                    hot_vkey: hb.operational_cert_hot_vkey.to_vec(),
                    sequence_number: hb.operational_cert_sequence_number,
                    kes_period: hb.operational_cert_kes_period,
                    sigma: hb.operational_cert_sigma.to_vec(),
                },
                ProtocolVersion {
                    major: hb.protocol_major,
                    minor: hb.protocol_minor,
                },
                alonzo.body_signature.to_vec(),
            )
        } else {
            // Byron
            (
                VrfOutput {
                    output: pallas_header
                        .nonce_vrf_output()
                        .map(|o| o.to_vec())
                        .unwrap_or_default(),
                    proof: Vec::new(),
                },
                Hash32::ZERO,
                OperationalCert {
                    hot_vkey: Vec::new(),
                    sequence_number: 0,
                    kes_period: 0,
                    sigma: Vec::new(),
                },
                ProtocolVersion { major: 1, minor: 0 },
                Vec::new(), // Byron has no KES signature
            )
        };

    Ok(BlockHeader {
        header_hash,
        prev_hash,
        issuer_vkey,
        vrf_vkey,
        vrf_result,
        block_number,
        slot,
        epoch_nonce: Hash32::ZERO,
        body_size,
        body_hash,
        operational_cert: op_cert,
        protocol_version,
        kes_signature,
    })
}

fn decode_transaction_from_pallas(tx: &PallasTx) -> Result<Transaction, SerializationError> {
    let tx_hash = pallas_hash_to_torsten32(&tx.hash());
    let raw_cbor = Some(tx.encode());
    let inputs = tx.inputs().iter().map(convert_input).collect();

    let outputs = tx
        .outputs()
        .iter()
        .map(|o| convert_output_with_cbor(o))
        .collect::<Result<Vec<_>, _>>()?;

    let fee = Lovelace(tx.fee().unwrap_or(0));

    let mint = convert_mint(tx);

    let collateral: Vec<TransactionInput> = tx.collateral().iter().map(convert_input).collect();

    let required_signers = convert_required_signers(tx);

    let reference_inputs: Vec<TransactionInput> =
        tx.reference_inputs().iter().map(convert_input).collect();

    let ttl = tx.ttl().map(SlotNo);
    let validity_interval_start = tx.validity_start().map(SlotNo);

    let certificates = tx
        .certs()
        .iter()
        .filter_map(|c| convert_certificate(c))
        .collect();

    let withdrawals = convert_withdrawals(tx);

    let body = TransactionBody {
        inputs,
        outputs,
        fee,
        ttl,
        certificates,
        withdrawals,
        auxiliary_data_hash: extract_auxiliary_data_hash(tx),
        validity_interval_start,
        mint,
        script_data_hash: extract_script_data_hash(tx),
        collateral,
        required_signers,
        network_id: tx.network_id().map(|n| match n {
            pallas_primitives::NetworkId::Testnet => 0,
            pallas_primitives::NetworkId::Mainnet => 1,
        }),
        collateral_return: tx.collateral_return().and_then(|o| convert_output(&o).ok()),
        total_collateral: tx.total_collateral().map(Lovelace),
        reference_inputs,
        update: convert_update_proposal(tx),
        voting_procedures: convert_voting_procedures(tx),
        proposal_procedures: convert_proposal_procedures(tx),
        treasury_value: tx
            .as_conway()
            .and_then(|ct| ct.transaction_body.treasury_value)
            .map(Lovelace),
        donation: tx
            .as_conway()
            .and_then(|ct| ct.transaction_body.donation.map(|d| Lovelace(u64::from(d)))),
    };

    let vkey_witnesses = tx
        .vkey_witnesses()
        .iter()
        .map(|w| VKeyWitness {
            vkey: w.vkey.to_vec(),
            signature: w.signature.to_vec(),
        })
        .collect();

    let native_scripts = tx
        .native_scripts()
        .iter()
        .map(|s| convert_native_script(s))
        .collect();

    let bootstrap_witnesses = tx
        .bootstrap_witnesses()
        .iter()
        .map(|bw| BootstrapWitness {
            vkey: bw.public_key.to_vec(),
            signature: bw.signature.to_vec(),
            chain_code: bw.chain_code.to_vec(),
            attributes: bw.attributes.to_vec(),
        })
        .collect();

    let plutus_v1_scripts = tx
        .plutus_v1_scripts()
        .iter()
        .map(|s| s.0.to_vec())
        .collect();

    let plutus_v2_scripts = tx
        .plutus_v2_scripts()
        .iter()
        .map(|s| s.0.to_vec())
        .collect();

    let plutus_v3_scripts = tx
        .plutus_v3_scripts()
        .iter()
        .map(|s| s.0.to_vec())
        .collect();

    let plutus_data = tx
        .plutus_data()
        .iter()
        .map(|d| convert_plutus_data(d))
        .collect();

    let redeemers = tx.redeemers().iter().map(|r| convert_redeemer(r)).collect();

    let witness_set = TransactionWitnessSet {
        vkey_witnesses,
        native_scripts,
        bootstrap_witnesses,
        plutus_v1_scripts,
        plutus_v2_scripts,
        plutus_v3_scripts,
        plutus_data,
        redeemers,
    };

    let auxiliary_data = convert_auxiliary_data(tx);

    Ok(Transaction {
        hash: tx_hash,
        body,
        witness_set,
        is_valid: tx.is_valid(),
        auxiliary_data,
        raw_cbor,
    })
}

fn convert_required_signers(tx: &PallasTx) -> Vec<Hash32> {
    use pallas_traverse::MultiEraSigners;
    match tx.required_signers() {
        MultiEraSigners::AlonzoCompatible(signers) => signers
            .iter()
            .map(|h| {
                // Required signers are AddrKeyhash (28 bytes); pad to Hash32
                let mut bytes = [0u8; 32];
                let slice = h.as_ref();
                bytes[..slice.len()].copy_from_slice(slice);
                Hash32::from_bytes(bytes)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn convert_input(input: &PallasInput) -> TransactionInput {
    TransactionInput {
        transaction_id: pallas_hash_to_torsten32(input.hash()),
        index: input.index() as u32,
    }
}

fn convert_output(output: &PallasOutput) -> Result<TransactionOutput, SerializationError> {
    convert_output_inner(output, None)
}

fn convert_output_with_cbor(
    output: &PallasOutput,
) -> Result<TransactionOutput, SerializationError> {
    let raw_cbor = Some(output.encode());
    convert_output_inner(output, raw_cbor)
}

fn convert_output_inner(
    output: &PallasOutput,
    raw_cbor: Option<Vec<u8>>,
) -> Result<TransactionOutput, SerializationError> {
    let address = convert_address(output)?;

    let multi_era_value = output.value();
    let lovelace = multi_era_value.coin();
    let multi_asset = convert_value_assets(&multi_era_value);

    let value = if multi_asset.is_empty() {
        Value::lovelace(lovelace)
    } else {
        Value {
            coin: Lovelace(lovelace),
            multi_asset,
        }
    };

    let datum = match output.datum() {
        Some(pallas_primitives::conway::DatumOption::Hash(h)) => {
            OutputDatum::DatumHash(pallas_hash_to_torsten32(&h))
        }
        Some(pallas_primitives::conway::DatumOption::Data(d)) => {
            OutputDatum::InlineDatum(convert_plutus_data(&d.0))
        }
        None => OutputDatum::None,
    };

    let script_ref = output.script_ref().map(|sr| convert_script_ref(&sr));

    Ok(TransactionOutput {
        address,
        value,
        datum,
        script_ref,
        raw_cbor,
    })
}

fn convert_script_ref(sr: &pallas_primitives::conway::ScriptRef) -> ScriptRef {
    use pallas_primitives::conway::ScriptRef as PSR;
    match sr {
        PSR::NativeScript(ns) => ScriptRef::NativeScript(convert_native_script_inner(ns)),
        PSR::PlutusV1Script(s) => ScriptRef::PlutusV1(s.0.to_vec()),
        PSR::PlutusV2Script(s) => ScriptRef::PlutusV2(s.0.to_vec()),
        PSR::PlutusV3Script(s) => ScriptRef::PlutusV3(s.0.to_vec()),
    }
}

fn convert_address(output: &PallasOutput) -> Result<Address, SerializationError> {
    let pallas_addr = output
        .address()
        .map_err(|e| SerializationError::InvalidData(format!("address decode: {e}")))?;

    let raw = pallas_addr.to_vec();
    Address::from_bytes(&raw)
        .map_err(|e| SerializationError::InvalidData(format!("address from bytes: {e}")))
}

fn convert_value_assets(
    value: &pallas_traverse::MultiEraValue,
) -> BTreeMap<Hash28, BTreeMap<AssetName, u64>> {
    let mut result = BTreeMap::new();

    for policy_assets in value.assets() {
        let policy_bytes: &[u8] = policy_assets.policy().as_ref();
        if let Ok(policy) = Hash28::try_from(policy_bytes) {
            let assets_entry = result.entry(policy).or_insert_with(BTreeMap::new);
            for asset in policy_assets.assets() {
                let asset_name = AssetName(asset.name().to_vec());
                if let Some(qty) = asset.output_coin() {
                    assets_entry.insert(asset_name, qty);
                }
            }
        }
    }

    result
}

fn convert_mint(tx: &PallasTx) -> BTreeMap<Hash28, BTreeMap<AssetName, i64>> {
    let mut result = BTreeMap::new();

    for policy_assets in tx.mints() {
        let policy_bytes: &[u8] = policy_assets.policy().as_ref();
        if let Ok(policy) = Hash28::try_from(policy_bytes) {
            let assets_entry = result.entry(policy).or_insert_with(BTreeMap::new);
            for asset in policy_assets.assets() {
                let asset_name = AssetName(asset.name().to_vec());
                if let Some(qty) = asset.mint_coin() {
                    assets_entry.insert(asset_name, qty);
                }
            }
        }
    }

    result
}

fn convert_auxiliary_data(tx: &PallasTx) -> Option<AuxiliaryData> {
    use pallas_traverse::MultiEraMeta;
    match tx.metadata() {
        MultiEraMeta::Empty | MultiEraMeta::NotApplicable => None,
        MultiEraMeta::AlonzoCompatible(metadata) => {
            let converted: BTreeMap<u64, TransactionMetadatum> = metadata
                .iter()
                .map(|(label, value)| (*label, convert_metadatum(value)))
                .collect();
            if converted.is_empty() {
                None
            } else {
                Some(AuxiliaryData {
                    metadata: converted,
                    native_scripts: Vec::new(),
                    plutus_v1_scripts: Vec::new(),
                    plutus_v2_scripts: Vec::new(),
                    plutus_v3_scripts: Vec::new(),
                })
            }
        }
        _ => None,
    }
}

fn convert_metadatum(m: &pallas_primitives::Metadatum) -> TransactionMetadatum {
    use pallas_primitives::Metadatum as PM;
    match m {
        PM::Int(i) => {
            let val: i128 = (*i).into();
            TransactionMetadatum::Int(val)
        }
        PM::Bytes(b) => TransactionMetadatum::Bytes(b.to_vec()),
        PM::Text(s) => TransactionMetadatum::Text(s.clone()),
        PM::Array(items) => {
            TransactionMetadatum::List(items.iter().map(convert_metadatum).collect())
        }
        PM::Map(entries) => TransactionMetadatum::Map(
            entries
                .iter()
                .map(|(k, v)| (convert_metadatum(k), convert_metadatum(v)))
                .collect(),
        ),
    }
}

fn extract_auxiliary_data_hash(tx: &PallasTx) -> Option<Hash32> {
    if let Some(alonzo) = tx.as_alonzo() {
        alonzo
            .transaction_body
            .auxiliary_data_hash
            .as_ref()
            .map(pallas_hash_to_torsten32)
    } else if let Some(babbage) = tx.as_babbage() {
        babbage
            .transaction_body
            .auxiliary_data_hash
            .as_ref()
            .map(|b| {
                let mut bytes = [0u8; 32];
                let len = b.len().min(32);
                bytes[..len].copy_from_slice(&b[..len]);
                Hash32::from_bytes(bytes)
            })
    } else if let Some(conway) = tx.as_conway() {
        conway
            .transaction_body
            .auxiliary_data_hash
            .as_ref()
            .map(pallas_hash_to_torsten32)
    } else {
        None
    }
}

fn extract_script_data_hash(tx: &PallasTx) -> Option<Hash32> {
    if let Some(babbage) = tx.as_babbage() {
        babbage
            .transaction_body
            .script_data_hash
            .as_ref()
            .map(pallas_hash_to_torsten32)
    } else if let Some(conway) = tx.as_conway() {
        conway
            .transaction_body
            .script_data_hash
            .as_ref()
            .map(pallas_hash_to_torsten32)
    } else if let Some(alonzo) = tx.as_alonzo() {
        alonzo
            .transaction_body
            .script_data_hash
            .as_ref()
            .map(pallas_hash_to_torsten32)
    } else {
        None
    }
}

fn convert_native_script(
    script: &pallas_codec::utils::KeepRaw<pallas_primitives::alonzo::NativeScript>,
) -> NativeScript {
    convert_native_script_inner(script)
}

fn convert_native_script_inner(script: &pallas_primitives::alonzo::NativeScript) -> NativeScript {
    use pallas_primitives::alonzo::NativeScript as PNS;
    match script {
        PNS::ScriptPubkey(h) => {
            // ScriptPubkey contains AddrKeyhash (28 bytes); pad to Hash32
            NativeScript::ScriptPubkey(pallas_hash_to_torsten28(h).to_hash32_padded())
        }
        PNS::ScriptAll(scripts) => {
            NativeScript::ScriptAll(scripts.iter().map(convert_native_script_inner).collect())
        }
        PNS::ScriptAny(scripts) => {
            NativeScript::ScriptAny(scripts.iter().map(convert_native_script_inner).collect())
        }
        PNS::ScriptNOfK(n, scripts) => NativeScript::ScriptNOfK(
            *n,
            scripts.iter().map(convert_native_script_inner).collect(),
        ),
        PNS::InvalidBefore(slot) => NativeScript::InvalidBefore(SlotNo(*slot)),
        PNS::InvalidHereafter(slot) => NativeScript::InvalidHereafter(SlotNo(*slot)),
    }
}

fn convert_redeemer(r: &pallas_traverse::MultiEraRedeemer) -> Redeemer {
    use pallas_primitives::conway::RedeemerTag as PRT;
    let tag = match r.tag() {
        PRT::Spend => RedeemerTag::Spend,
        PRT::Mint => RedeemerTag::Mint,
        PRT::Cert => RedeemerTag::Cert,
        PRT::Reward => RedeemerTag::Reward,
        PRT::Vote => RedeemerTag::Vote,
        PRT::Propose => RedeemerTag::Propose,
    };
    let ex = r.ex_units();
    Redeemer {
        tag,
        index: r.index(),
        data: convert_plutus_data(r.data()),
        ex_units: ExUnits {
            mem: ex.mem,
            steps: ex.steps,
        },
    }
}

fn convert_plutus_data(data: &pallas_primitives::conway::PlutusData) -> PlutusData {
    use pallas_primitives::conway::PlutusData as PD;
    match data {
        PD::BigInt(bi) => {
            let val: i128 = match bi {
                pallas_primitives::conway::BigInt::Int(n) => (*n).into(),
                pallas_primitives::conway::BigInt::BigUInt(b) => {
                    let bytes: &[u8] = b;
                    let mut val: i128 = 0;
                    for byte in bytes {
                        val = (val << 8) | (*byte as i128);
                    }
                    val
                }
                pallas_primitives::conway::BigInt::BigNInt(b) => {
                    let bytes: &[u8] = b;
                    let mut val: i128 = 0;
                    for byte in bytes {
                        val = (val << 8) | (*byte as i128);
                    }
                    -1 - val
                }
            };
            PlutusData::Integer(val)
        }
        PD::BoundedBytes(b) => PlutusData::Bytes(b.to_vec()),
        PD::Constr(constr) => {
            let tag = constr.tag;
            let constructor = if (121..=127).contains(&tag) {
                tag - 121
            } else if (1280..=1400).contains(&tag) {
                tag - 1280 + 7
            } else {
                tag
            };
            let fields: Vec<PlutusData> = constr.fields.iter().map(convert_plutus_data).collect();
            PlutusData::Constr(constructor, fields)
        }
        PD::Map(entries) => {
            let converted: Vec<(PlutusData, PlutusData)> = entries
                .iter()
                .map(|(k, v)| (convert_plutus_data(k), convert_plutus_data(v)))
                .collect();
            PlutusData::Map(converted)
        }
        PD::Array(items) => {
            let converted: Vec<PlutusData> = items.iter().map(convert_plutus_data).collect();
            PlutusData::List(converted)
        }
    }
}

/// Safely convert a byte slice to Hash32, padding with zeros if shorter than 32 bytes.
fn bytes_to_hash32(bytes: &[u8]) -> Hash32 {
    let mut buf = [0u8; 32];
    let len = bytes.len().min(32);
    buf[..len].copy_from_slice(&bytes[..len]);
    Hash32::from_bytes(buf)
}

/// Convert a pallas Hash<32> to a torsten Hash32
pub fn pallas_hash_to_torsten32(hash: &pallas_crypto::hash::Hash<32>) -> Hash32 {
    let bytes: &[u8; 32] = hash;
    Hash::from_bytes(*bytes)
}

/// Convert a pallas Hash<28> to a torsten Hash28
pub fn pallas_hash_to_torsten28(hash: &pallas_crypto::hash::Hash<28>) -> Hash28 {
    let bytes: &[u8; 28] = hash;
    Hash::from_bytes(*bytes)
}

/// Convert a torsten Hash32 to a pallas Hash<32>
pub fn torsten_hash_to_pallas32(hash: &Hash32) -> pallas_crypto::hash::Hash<32> {
    pallas_crypto::hash::Hash::from(*hash.as_bytes())
}

/// Convert a torsten Hash28 to a pallas Hash<28>
pub fn torsten_hash_to_pallas28(hash: &Hash28) -> pallas_crypto::hash::Hash<28> {
    pallas_crypto::hash::Hash::from(*hash.as_bytes())
}

fn convert_pallas_stake_credential(cred: &pallas_primitives::StakeCredential) -> Credential {
    match cred {
        pallas_primitives::StakeCredential::AddrKeyhash(h) => {
            Credential::VerificationKey(pallas_hash_to_torsten28(h))
        }
        pallas_primitives::StakeCredential::ScriptHash(h) => {
            Credential::Script(pallas_hash_to_torsten28(h))
        }
    }
}

fn convert_certificate(cert: &MultiEraCert) -> Option<Certificate> {
    if let Some(alonzo_cert) = cert.as_alonzo() {
        return convert_alonzo_certificate(alonzo_cert);
    }
    if let Some(conway_cert) = cert.as_conway() {
        return convert_conway_certificate(conway_cert);
    }
    None
}

fn convert_alonzo_certificate(
    cert: &pallas_primitives::alonzo::Certificate,
) -> Option<Certificate> {
    use pallas_primitives::alonzo::Certificate as AC;
    match cert {
        AC::StakeRegistration(cred) => Some(Certificate::StakeRegistration(
            convert_pallas_stake_credential(cred),
        )),
        AC::StakeDeregistration(cred) => Some(Certificate::StakeDeregistration(
            convert_pallas_stake_credential(cred),
        )),
        AC::StakeDelegation(cred, pool_hash) => Some(Certificate::StakeDelegation {
            credential: convert_pallas_stake_credential(cred),
            pool_hash: pallas_hash_to_torsten28(pool_hash),
        }),
        AC::PoolRegistration {
            operator,
            vrf_keyhash,
            pledge,
            cost,
            margin,
            reward_account,
            pool_owners,
            relays,
            pool_metadata,
        } => {
            let owners = pool_owners.iter().map(pallas_hash_to_torsten28).collect();
            let pool_relays = relays.iter().filter_map(convert_relay).collect();
            let metadata = pool_metadata.clone();
            let metadata = metadata.map(|m| PoolMetadata {
                url: m.url.clone(),
                hash: bytes_to_hash32(m.hash.as_ref()),
            });

            Some(Certificate::PoolRegistration(PoolParams {
                operator: pallas_hash_to_torsten28(operator),
                vrf_keyhash: pallas_hash_to_torsten32(vrf_keyhash),
                pledge: Lovelace(*pledge),
                cost: Lovelace(*cost),
                margin: Rational {
                    numerator: margin.numerator,
                    denominator: margin.denominator,
                },
                reward_account: reward_account.to_vec(),
                pool_owners: owners,
                relays: pool_relays,
                pool_metadata: metadata,
            }))
        }
        AC::PoolRetirement(pool_hash, epoch) => Some(Certificate::PoolRetirement {
            pool_hash: pallas_hash_to_torsten28(pool_hash),
            epoch: *epoch,
        }),
        AC::GenesisKeyDelegation(genesis_hash, delegate_hash, vrf_keyhash) => {
            Some(Certificate::GenesisKeyDelegation {
                genesis_hash: bytes_to_hash32(genesis_hash),
                genesis_delegate_hash: bytes_to_hash32(delegate_hash),
                vrf_keyhash: pallas_hash_to_torsten32(vrf_keyhash),
            })
        }
        AC::MoveInstantaneousRewardsCert(mir) => {
            use pallas_primitives::alonzo::{InstantaneousRewardSource, InstantaneousRewardTarget};
            let source = match mir.source {
                InstantaneousRewardSource::Reserves => MIRSource::Reserves,
                InstantaneousRewardSource::Treasury => MIRSource::Treasury,
            };
            let target = match &mir.target {
                InstantaneousRewardTarget::StakeCredentials(creds) => {
                    let entries = creds
                        .iter()
                        .map(|(cred, amount)| (convert_pallas_stake_credential(cred), *amount))
                        .collect();
                    MIRTarget::StakeCredentials(entries)
                }
                InstantaneousRewardTarget::OtherAccountingPot(coin) => {
                    MIRTarget::OtherAccountingPot(*coin)
                }
            };
            Some(Certificate::MoveInstantaneousRewards { source, target })
        }
    }
}

fn convert_conway_certificate(
    cert: &pallas_primitives::conway::Certificate,
) -> Option<Certificate> {
    use pallas_primitives::conway::Certificate as CC;
    match cert {
        CC::StakeRegistration(cred) => Some(Certificate::StakeRegistration(
            convert_pallas_stake_credential(cred),
        )),
        CC::StakeDeregistration(cred) => Some(Certificate::StakeDeregistration(
            convert_pallas_stake_credential(cred),
        )),
        CC::StakeDelegation(cred, pool_hash) => Some(Certificate::StakeDelegation {
            credential: convert_pallas_stake_credential(cred),
            pool_hash: pallas_hash_to_torsten28(pool_hash),
        }),
        CC::PoolRegistration {
            operator,
            vrf_keyhash,
            pledge,
            cost,
            margin,
            reward_account,
            pool_owners,
            relays,
            pool_metadata,
        } => {
            let owners = pool_owners.iter().map(pallas_hash_to_torsten28).collect();
            let pool_relays = relays.iter().filter_map(convert_relay).collect();
            let metadata = pool_metadata.clone();
            let metadata = metadata.map(|m| PoolMetadata {
                url: m.url.clone(),
                hash: bytes_to_hash32(m.hash.as_ref()),
            });

            Some(Certificate::PoolRegistration(PoolParams {
                operator: pallas_hash_to_torsten28(operator),
                vrf_keyhash: pallas_hash_to_torsten32(vrf_keyhash),
                pledge: Lovelace(*pledge),
                cost: Lovelace(*cost),
                margin: Rational {
                    numerator: margin.numerator,
                    denominator: margin.denominator,
                },
                reward_account: reward_account.to_vec(),
                pool_owners: owners,
                relays: pool_relays,
                pool_metadata: metadata,
            }))
        }
        CC::PoolRetirement(pool_hash, epoch) => Some(Certificate::PoolRetirement {
            pool_hash: pallas_hash_to_torsten28(pool_hash),
            epoch: *epoch,
        }),
        CC::StakeRegDeleg(cred, pool_hash, deposit) => Some(Certificate::RegStakeDeleg {
            credential: convert_pallas_stake_credential(cred),
            pool_hash: pallas_hash_to_torsten28(pool_hash),
            deposit: Lovelace(*deposit),
        }),
        CC::Reg(cred, deposit) => Some(Certificate::ConwayStakeRegistration {
            credential: convert_pallas_stake_credential(cred),
            deposit: Lovelace(*deposit),
        }),
        CC::UnReg(cred, refund) => Some(Certificate::ConwayStakeDeregistration {
            credential: convert_pallas_stake_credential(cred),
            refund: Lovelace(*refund),
        }),
        CC::VoteDeleg(cred, drep) => Some(Certificate::VoteDelegation {
            credential: convert_pallas_stake_credential(cred),
            drep: convert_pallas_drep(drep),
        }),
        CC::StakeVoteDeleg(cred, pool_hash, drep) => Some(Certificate::StakeVoteDelegation {
            credential: convert_pallas_stake_credential(cred),
            pool_hash: pallas_hash_to_torsten28(pool_hash),
            drep: convert_pallas_drep(drep),
        }),
        CC::RegDRepCert(cred, deposit, anchor) => Some(Certificate::RegDRep {
            credential: convert_pallas_stake_credential(cred),
            deposit: Lovelace(*deposit),
            anchor: anchor.as_ref().map(convert_pallas_anchor),
        }),
        CC::UnRegDRepCert(cred, refund) => Some(Certificate::UnregDRep {
            credential: convert_pallas_stake_credential(cred),
            refund: Lovelace(*refund),
        }),
        CC::UpdateDRepCert(cred, anchor) => Some(Certificate::UpdateDRep {
            credential: convert_pallas_stake_credential(cred),
            anchor: anchor.as_ref().map(convert_pallas_anchor),
        }),
        CC::AuthCommitteeHot(cold_cred, hot_cred) => Some(Certificate::CommitteeHotAuth {
            cold_credential: convert_pallas_stake_credential(cold_cred),
            hot_credential: convert_pallas_stake_credential(hot_cred),
        }),
        CC::ResignCommitteeCold(cold_cred, anchor) => Some(Certificate::CommitteeColdResign {
            cold_credential: convert_pallas_stake_credential(cold_cred),
            anchor: anchor.as_ref().map(convert_pallas_anchor),
        }),
        CC::StakeVoteRegDeleg(cred, pool_hash, drep, deposit) => {
            Some(Certificate::RegStakeVoteDeleg {
                credential: convert_pallas_stake_credential(cred),
                pool_hash: pallas_hash_to_torsten28(pool_hash),
                drep: convert_pallas_drep(drep),
                deposit: Lovelace(*deposit),
            })
        }
        CC::VoteRegDeleg(cred, drep, deposit) => Some(Certificate::VoteRegDeleg {
            credential: convert_pallas_stake_credential(cred),
            drep: convert_pallas_drep(drep),
            deposit: Lovelace(*deposit),
        }),
    }
}

fn convert_pallas_drep(drep: &pallas_primitives::conway::DRep) -> DRep {
    use pallas_primitives::conway::DRep as PD;
    match drep {
        PD::Key(h) => {
            // DRep key hash is 28 bytes; pad to Hash32
            DRep::KeyHash(pallas_hash_to_torsten28(h).to_hash32_padded())
        }
        PD::Script(h) => DRep::ScriptHash(pallas_hash_to_torsten28(h)),
        PD::Abstain => DRep::Abstain,
        PD::NoConfidence => DRep::NoConfidence,
    }
}

fn convert_pallas_anchor(anchor: &pallas_primitives::conway::Anchor) -> Anchor {
    Anchor {
        url: anchor.url.clone(),
        data_hash: pallas_hash_to_torsten32(&anchor.content_hash),
    }
}

fn convert_relay(relay: &pallas_primitives::Relay) -> Option<Relay> {
    use pallas_primitives::Relay as PR;
    match relay {
        PR::SingleHostAddr(port, ipv4, ipv6) => Some(Relay::SingleHostAddr {
            port: port.map(|p| p as u16),
            ipv4: ipv4.clone().map(|v| {
                let bytes = v.to_vec();
                let mut arr = [0u8; 4];
                let len = bytes.len().min(4);
                arr[..len].copy_from_slice(&bytes[..len]);
                arr
            }),
            ipv6: ipv6.clone().map(|v| {
                let bytes = v.to_vec();
                let mut arr = [0u8; 16];
                let len = bytes.len().min(16);
                arr[..len].copy_from_slice(&bytes[..len]);
                arr
            }),
        }),
        PR::SingleHostName(port, dns) => Some(Relay::SingleHostName {
            port: port.map(|p| p as u16),
            dns_name: dns.clone(),
        }),
        PR::MultiHostName(dns) => Some(Relay::MultiHostName {
            dns_name: dns.clone(),
        }),
    }
}

fn convert_withdrawals(tx: &PallasTx) -> BTreeMap<Vec<u8>, Lovelace> {
    let mut result = BTreeMap::new();
    match tx.withdrawals() {
        MultiEraWithdrawals::NotApplicable | MultiEraWithdrawals::Empty => {}
        MultiEraWithdrawals::AlonzoCompatible(w) => {
            for (account, amount) in w.iter() {
                result.insert(account.to_vec(), Lovelace(*amount));
            }
        }
        MultiEraWithdrawals::Conway(w) => {
            for (account, amount) in w.iter() {
                result.insert(account.to_vec(), Lovelace(*amount));
            }
        }
        _ => {}
    }
    result
}

/// Extract pre-Conway update proposal from a transaction (field 6 in CDDL)
fn convert_update_proposal(tx: &PallasTx) -> Option<UpdateProposal> {
    let update = tx.update()?;
    match update {
        pallas_traverse::MultiEraUpdate::AlonzoCompatible(u) => {
            let proposed_updates = u
                .proposed_protocol_parameter_updates
                .iter()
                .map(|(genesis_hash, ppu)| {
                    (
                        bytes_to_hash32(genesis_hash),
                        convert_pallas_ppup_alonzo(ppu),
                    )
                })
                .collect();
            Some(UpdateProposal {
                proposed_updates,
                epoch: u.epoch,
            })
        }
        pallas_traverse::MultiEraUpdate::Babbage(u) => {
            let proposed_updates = u
                .proposed_protocol_parameter_updates
                .iter()
                .map(|(genesis_hash, ppu)| {
                    (
                        bytes_to_hash32(genesis_hash),
                        convert_pallas_ppup_babbage(ppu),
                    )
                })
                .collect();
            Some(UpdateProposal {
                proposed_updates,
                epoch: u.epoch,
            })
        }
        _ => None, // Byron/Conway handled differently
    }
}

/// Convert Alonzo-era ProtocolParamUpdate to our type
fn convert_pallas_ppup_alonzo(
    ppu: &pallas_primitives::alonzo::ProtocolParamUpdate,
) -> ProtocolParamUpdate {
    ProtocolParamUpdate {
        min_fee_a: ppu.minfee_a.map(|v| v as u64),
        min_fee_b: ppu.minfee_b.map(|v| v as u64),
        max_block_body_size: ppu.max_block_body_size.map(|v| v as u64),
        max_tx_size: ppu.max_transaction_size.map(|v| v as u64),
        max_block_header_size: ppu.max_block_header_size.map(|v| v as u64),
        key_deposit: ppu.key_deposit.map(Lovelace),
        pool_deposit: ppu.pool_deposit.map(Lovelace),
        e_max: ppu.maximum_epoch,
        n_opt: ppu.desired_number_of_stake_pools.map(|v| v as u64),
        a0: ppu.pool_pledge_influence.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        rho: ppu.expansion_rate.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        tau: ppu.treasury_growth_rate.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        protocol_version_major: ppu.protocol_version.as_ref().map(|v| v.0),
        protocol_version_minor: ppu.protocol_version.as_ref().map(|v| v.1),
        min_pool_cost: ppu.min_pool_cost.map(Lovelace),
        ada_per_utxo_byte: ppu.ada_per_utxo_byte.map(Lovelace),
        max_tx_ex_units: ppu.max_tx_ex_units.as_ref().map(|eu| ExUnits {
            mem: eu.mem,
            steps: eu.steps,
        }),
        max_block_ex_units: ppu.max_block_ex_units.as_ref().map(|eu| ExUnits {
            mem: eu.mem,
            steps: eu.steps,
        }),
        max_val_size: ppu.max_value_size.map(|v| v as u64),
        collateral_percentage: ppu.collateral_percentage.map(|v| v as u64),
        max_collateral_inputs: ppu.max_collateral_inputs.map(|v| v as u64),
        ..Default::default()
    }
}

/// Convert Babbage-era ProtocolParamUpdate to our type
fn convert_pallas_ppup_babbage(
    ppu: &pallas_primitives::babbage::ProtocolParamUpdate,
) -> ProtocolParamUpdate {
    ProtocolParamUpdate {
        min_fee_a: ppu.minfee_a.map(|v| v as u64),
        min_fee_b: ppu.minfee_b.map(|v| v as u64),
        max_block_body_size: ppu.max_block_body_size.map(|v| v as u64),
        max_tx_size: ppu.max_transaction_size.map(|v| v as u64),
        max_block_header_size: ppu.max_block_header_size.map(|v| v as u64),
        key_deposit: ppu.key_deposit.map(Lovelace),
        pool_deposit: ppu.pool_deposit.map(Lovelace),
        e_max: ppu.maximum_epoch,
        n_opt: ppu.desired_number_of_stake_pools.map(|v| v as u64),
        a0: ppu.pool_pledge_influence.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        rho: ppu.expansion_rate.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        tau: ppu.treasury_growth_rate.as_ref().map(|r| Rational {
            numerator: r.numerator,
            denominator: r.denominator,
        }),
        protocol_version_major: ppu.protocol_version.as_ref().map(|v| v.0),
        protocol_version_minor: ppu.protocol_version.as_ref().map(|v| v.1),
        min_pool_cost: ppu.min_pool_cost.map(Lovelace),
        ada_per_utxo_byte: ppu.ada_per_utxo_byte.map(Lovelace),
        max_tx_ex_units: ppu.max_tx_ex_units.as_ref().map(|eu| ExUnits {
            mem: eu.mem,
            steps: eu.steps,
        }),
        max_block_ex_units: ppu.max_block_ex_units.as_ref().map(|eu| ExUnits {
            mem: eu.mem,
            steps: eu.steps,
        }),
        max_val_size: ppu.max_value_size.map(|v| v as u64),
        collateral_percentage: ppu.collateral_percentage.map(|v| v as u64),
        max_collateral_inputs: ppu.max_collateral_inputs.map(|v| v as u64),
        ..Default::default()
    }
}

fn convert_voting_procedures(
    tx: &PallasTx,
) -> BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> {
    let mut result = BTreeMap::new();

    if let Some(conway_tx) = tx.as_conway() {
        if let Some(voting_procs) = &conway_tx.transaction_body.voting_procedures {
            for (pallas_voter, votes_by_action) in voting_procs.iter() {
                let voter = convert_pallas_voter(pallas_voter);
                let mut action_votes = BTreeMap::new();
                for (pallas_action_id, pallas_proc) in votes_by_action.iter() {
                    let action_id = GovActionId {
                        transaction_id: pallas_hash_to_torsten32(&pallas_action_id.transaction_id),
                        action_index: pallas_action_id.action_index,
                    };
                    let procedure = VotingProcedure {
                        vote: convert_pallas_vote(&pallas_proc.vote),
                        anchor: pallas_proc.anchor.as_ref().map(convert_pallas_anchor),
                    };
                    action_votes.insert(action_id, procedure);
                }
                result.insert(voter, action_votes);
            }
        }
    }

    result
}

fn convert_proposal_procedures(tx: &PallasTx) -> Vec<ProposalProcedure> {
    tx.gov_proposals()
        .iter()
        .filter_map(|proposal| {
            let conway_prop = proposal.as_conway()?;
            Some(ProposalProcedure {
                deposit: Lovelace(conway_prop.deposit),
                return_addr: conway_prop.reward_account.to_vec(),
                gov_action: convert_pallas_gov_action(&conway_prop.gov_action),
                anchor: convert_pallas_anchor(&conway_prop.anchor),
            })
        })
        .collect()
}

fn convert_pallas_voter(voter: &pallas_primitives::conway::Voter) -> Voter {
    use pallas_primitives::conway::Voter as PV;
    match voter {
        PV::ConstitutionalCommitteeKey(h) => {
            Voter::ConstitutionalCommittee(Credential::VerificationKey(pallas_hash_to_torsten28(h)))
        }
        PV::ConstitutionalCommitteeScript(h) => {
            Voter::ConstitutionalCommittee(Credential::Script(pallas_hash_to_torsten28(h)))
        }
        PV::DRepKey(h) => Voter::DRep(Credential::VerificationKey(pallas_hash_to_torsten28(h))),
        PV::DRepScript(h) => Voter::DRep(Credential::Script(pallas_hash_to_torsten28(h))),
        PV::StakePoolKey(h) => {
            // Pool key hash is 28 bytes; pad to Hash32
            Voter::StakePool(pallas_hash_to_torsten28(h).to_hash32_padded())
        }
    }
}

fn convert_pallas_vote(vote: &pallas_primitives::conway::Vote) -> Vote {
    use pallas_primitives::conway::Vote as PV;
    match vote {
        PV::No => Vote::No,
        PV::Yes => Vote::Yes,
        PV::Abstain => Vote::Abstain,
    }
}

fn convert_pallas_gov_action(action: &pallas_primitives::conway::GovAction) -> GovAction {
    use pallas_primitives::conway::GovAction as PGA;
    let convert_prev = |prev_id: &Option<pallas_primitives::conway::GovActionId>| {
        prev_id.as_ref().map(|id| GovActionId {
            transaction_id: pallas_hash_to_torsten32(&id.transaction_id),
            action_index: id.action_index,
        })
    };
    match action {
        PGA::ParameterChange(prev_id, update, script) => GovAction::ParameterChange {
            prev_action_id: convert_prev(prev_id),
            protocol_param_update: Box::new(convert_pallas_protocol_param_update(update)),
            policy_hash: script.as_ref().map(pallas_hash_to_torsten28),
        },
        PGA::HardForkInitiation(prev_id, version) => GovAction::HardForkInitiation {
            prev_action_id: convert_prev(prev_id),
            protocol_version: (version.0, version.1),
        },
        PGA::TreasuryWithdrawals(withdrawals, script) => {
            let mut converted = BTreeMap::new();
            for (account, amount) in withdrawals.iter() {
                converted.insert(account.to_vec(), Lovelace(*amount));
            }
            GovAction::TreasuryWithdrawals {
                withdrawals: converted,
                policy_hash: script.as_ref().map(pallas_hash_to_torsten28),
            }
        }
        PGA::NoConfidence(prev_id) => GovAction::NoConfidence {
            prev_action_id: convert_prev(prev_id),
        },
        PGA::UpdateCommittee(prev_id, remove, add, threshold) => {
            let members_to_remove = remove.iter().map(convert_pallas_stake_credential).collect();
            let mut members_to_add = BTreeMap::new();
            for (cred, epoch) in add.iter() {
                members_to_add.insert(convert_pallas_stake_credential(cred), *epoch);
            }
            GovAction::UpdateCommittee {
                prev_action_id: convert_prev(prev_id),
                members_to_remove,
                members_to_add,
                threshold: Rational {
                    numerator: threshold.numerator,
                    denominator: threshold.denominator,
                },
            }
        }
        PGA::NewConstitution(prev_id, constitution) => GovAction::NewConstitution {
            prev_action_id: convert_prev(prev_id),
            constitution: Constitution {
                anchor: convert_pallas_anchor(&constitution.anchor),
                script_hash: constitution
                    .guardrail_script
                    .map(|h| pallas_hash_to_torsten28(&h)),
            },
        },
        PGA::Information => GovAction::InfoAction,
    }
}

fn convert_pallas_protocol_param_update(
    update: &pallas_primitives::conway::ProtocolParamUpdate,
) -> ProtocolParamUpdate {
    let convert_rational = |r: &pallas_primitives::RationalNumber| Rational {
        numerator: r.numerator,
        denominator: r.denominator,
    };
    ProtocolParamUpdate {
        min_fee_a: update.minfee_a,
        min_fee_b: update.minfee_b,
        max_block_body_size: update.max_block_body_size,
        max_tx_size: update.max_transaction_size,
        max_block_header_size: update.max_block_header_size,
        key_deposit: update.key_deposit.map(Lovelace),
        pool_deposit: update.pool_deposit.map(Lovelace),
        e_max: update.maximum_epoch,
        n_opt: update.desired_number_of_stake_pools,
        a0: update.pool_pledge_influence.as_ref().map(convert_rational),
        rho: update.expansion_rate.as_ref().map(convert_rational),
        tau: update.treasury_growth_rate.as_ref().map(convert_rational),
        min_pool_cost: update.min_pool_cost.map(Lovelace),
        ada_per_utxo_byte: update.ada_per_utxo_byte.map(Lovelace),
        cost_models: update
            .cost_models_for_script_languages
            .as_ref()
            .map(|cm| CostModels {
                plutus_v1: cm.plutus_v1.clone(),
                plutus_v2: cm.plutus_v2.clone(),
                plutus_v3: cm.plutus_v3.clone(),
            }),
        execution_costs: update.execution_costs.as_ref().map(|ec| ExUnitPrices {
            mem_price: Rational {
                numerator: ec.mem_price.numerator,
                denominator: ec.mem_price.denominator,
            },
            step_price: Rational {
                numerator: ec.step_price.numerator,
                denominator: ec.step_price.denominator,
            },
        }),
        max_tx_ex_units: update.max_tx_ex_units.as_ref().map(|e| ExUnits {
            mem: e.mem,
            steps: e.steps,
        }),
        max_block_ex_units: update.max_block_ex_units.as_ref().map(|e| ExUnits {
            mem: e.mem,
            steps: e.steps,
        }),
        max_val_size: update.max_value_size,
        collateral_percentage: update.collateral_percentage,
        max_collateral_inputs: update.max_collateral_inputs,
        min_fee_ref_script_cost_per_byte: update.minfee_refscript_cost_per_byte.as_ref().map(|r| {
            // Convert rational to integer (numerator/denominator)
            if r.denominator > 0 {
                r.numerator / r.denominator
            } else {
                15 // default
            }
        }),
        drep_deposit: update.drep_deposit.map(Lovelace),
        gov_action_deposit: update.governance_action_deposit.map(Lovelace),
        gov_action_lifetime: update.governance_action_validity_period,
        dvt_pp_network_group: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.pp_network_group)),
        dvt_pp_economic_group: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.pp_economic_group)),
        dvt_pp_technical_group: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.pp_technical_group)),
        dvt_pp_gov_group: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.pp_governance_group)),
        dvt_hard_fork: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.hard_fork_initiation)),
        dvt_no_confidence: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.motion_no_confidence)),
        dvt_committee_normal: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.committee_normal)),
        dvt_committee_no_confidence: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.committee_no_confidence)),
        dvt_constitution: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.update_constitution)),
        dvt_treasury_withdrawal: update
            .drep_voting_thresholds
            .as_ref()
            .map(|d| convert_rational(&d.treasury_withdrawal)),
        pvt_motion_no_confidence: update
            .pool_voting_thresholds
            .as_ref()
            .map(|p| convert_rational(&p.motion_no_confidence)),
        pvt_committee_normal: update
            .pool_voting_thresholds
            .as_ref()
            .map(|p| convert_rational(&p.committee_normal)),
        pvt_committee_no_confidence: update
            .pool_voting_thresholds
            .as_ref()
            .map(|p| convert_rational(&p.committee_no_confidence)),
        pvt_hard_fork: update
            .pool_voting_thresholds
            .as_ref()
            .map(|p| convert_rational(&p.hard_fork_initiation)),
        pvt_pp_security_group: update
            .pool_voting_thresholds
            .as_ref()
            .map(|p| convert_rational(&p.security_voting_threshold)),
        min_committee_size: update.min_committee_size,
        committee_term_limit: update.committee_term_limit,
        drep_activity: update.drep_inactivity_period,
        // Conway doesn't have protocol_version in PPU (uses HardForkInitiation instead)
        protocol_version_major: None,
        protocol_version_minor: None,
    }
}

fn convert_era(era: pallas_traverse::Era) -> Era {
    match era {
        pallas_traverse::Era::Byron => Era::Byron,
        pallas_traverse::Era::Shelley => Era::Shelley,
        pallas_traverse::Era::Allegra => Era::Allegra,
        pallas_traverse::Era::Mary => Era::Mary,
        pallas_traverse::Era::Alonzo => Era::Alonzo,
        pallas_traverse::Era::Babbage => Era::Babbage,
        pallas_traverse::Era::Conway => Era::Conway,
        _ => Era::Conway,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::blake2b_256;

    #[test]
    fn test_hash32_conversion_roundtrip() {
        let torsten_hash = blake2b_256(b"test data");
        let pallas_hash = torsten_hash_to_pallas32(&torsten_hash);
        let back = pallas_hash_to_torsten32(&pallas_hash);
        assert_eq!(torsten_hash, back);
    }

    #[test]
    fn test_hash28_conversion_roundtrip() {
        let torsten_hash = Hash28::from_bytes([42u8; 28]);
        let pallas_hash = torsten_hash_to_pallas28(&torsten_hash);
        let back = pallas_hash_to_torsten28(&pallas_hash);
        assert_eq!(torsten_hash, back);
    }

    #[test]
    fn test_convert_era_all() {
        assert_eq!(convert_era(pallas_traverse::Era::Byron), Era::Byron);
        assert_eq!(convert_era(pallas_traverse::Era::Shelley), Era::Shelley);
        assert_eq!(convert_era(pallas_traverse::Era::Allegra), Era::Allegra);
        assert_eq!(convert_era(pallas_traverse::Era::Mary), Era::Mary);
        assert_eq!(convert_era(pallas_traverse::Era::Alonzo), Era::Alonzo);
        assert_eq!(convert_era(pallas_traverse::Era::Babbage), Era::Babbage);
        assert_eq!(convert_era(pallas_traverse::Era::Conway), Era::Conway);
    }

    #[test]
    fn test_convert_plutus_data_positive_int() {
        use pallas_primitives::conway::{BigInt, PlutusData as PD};
        let pd = PD::BigInt(BigInt::Int(42.into()));
        let converted = convert_plutus_data(&pd);
        assert_eq!(converted, PlutusData::Integer(42));
    }

    #[test]
    fn test_convert_plutus_data_negative_int() {
        use pallas_primitives::conway::{BigInt, PlutusData as PD};
        let pd = PD::BigInt(BigInt::Int((-7).into()));
        let converted = convert_plutus_data(&pd);
        assert_eq!(converted, PlutusData::Integer(-7));
    }

    #[test]
    fn test_convert_plutus_data_bytes() {
        use pallas_primitives::conway::PlutusData as PD;
        use pallas_primitives::BoundedBytes;
        let pd = PD::BoundedBytes(BoundedBytes::from(vec![0xde, 0xad]));
        let converted = convert_plutus_data(&pd);
        assert_eq!(converted, PlutusData::Bytes(vec![0xde, 0xad]));
    }

    #[test]
    fn test_convert_plutus_data_list() {
        use pallas_codec::utils::MaybeIndefArray;
        use pallas_primitives::conway::{BigInt, PlutusData as PD};
        let pd = PD::Array(MaybeIndefArray::Def(vec![
            PD::BigInt(BigInt::Int(1.into())),
            PD::BigInt(BigInt::Int(2.into())),
        ]));
        let converted = convert_plutus_data(&pd);
        assert_eq!(
            converted,
            PlutusData::List(vec![PlutusData::Integer(1), PlutusData::Integer(2)])
        );
    }

    #[test]
    fn test_convert_plutus_data_map() {
        use pallas_primitives::conway::{BigInt, PlutusData as PD};
        use pallas_primitives::BoundedBytes;
        let pd = PD::Map(pallas_codec::utils::KeyValuePairs::from(vec![(
            PD::BigInt(BigInt::Int(1.into())),
            PD::BoundedBytes(BoundedBytes::from(vec![0xff])),
        )]));
        let converted = convert_plutus_data(&pd);
        assert_eq!(
            converted,
            PlutusData::Map(vec![(
                PlutusData::Integer(1),
                PlutusData::Bytes(vec![0xff])
            )])
        );
    }

    #[test]
    fn test_decode_invalid_cbor_returns_error() {
        let bad_cbor = vec![0xff, 0xfe, 0xfd];
        let result = decode_block(&bad_cbor);
        assert!(result.is_err());
    }
}
