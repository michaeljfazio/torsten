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

/// Decode a multi-era block from raw CBOR bytes into a torsten Block.
pub fn decode_block(cbor: &[u8]) -> Result<Block, SerializationError> {
    let pallas_block = PallasBlock::decode(cbor)
        .map_err(|e| SerializationError::CborDecode(format!("block decode: {e}")))?;

    let era = convert_era(pallas_block.era());
    let header = decode_block_header(&pallas_block)?;
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

fn decode_block_header(block: &PallasBlock) -> Result<BlockHeader, SerializationError> {
    let slot = SlotNo(block.slot());
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

    let vrf_result = pallas_header
        .nonce_vrf_output()
        .map(|output| VrfOutput {
            output: output.to_vec(),
            proof: Vec::new(),
        })
        .unwrap_or(VrfOutput {
            output: Vec::new(),
            proof: Vec::new(),
        });

    let body_size = block.body_size().unwrap_or(0) as u64;

    // Extract era-specific header body fields
    let (body_hash, op_cert, protocol_version) = if let Some(babbage) = pallas_header.as_babbage() {
        let hb = &babbage.header_body;
        (
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
        )
    } else if let Some(alonzo) = pallas_header.as_alonzo() {
        let hb = &alonzo.header_body;
        (
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
        )
    } else {
        // Byron
        (
            Hash32::ZERO,
            OperationalCert {
                hot_vkey: Vec::new(),
                sequence_number: 0,
                kes_period: 0,
                sigma: Vec::new(),
            },
            ProtocolVersion { major: 1, minor: 0 },
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
    })
}

fn decode_transaction_from_pallas(tx: &PallasTx) -> Result<Transaction, SerializationError> {
    let tx_hash = pallas_hash_to_torsten32(&tx.hash());
    let inputs = tx.inputs().iter().map(convert_input).collect();

    let outputs = tx
        .outputs()
        .iter()
        .map(convert_output)
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
        auxiliary_data_hash: None,
        validity_interval_start,
        mint,
        script_data_hash: None,
        collateral,
        required_signers,
        network_id: None,
        collateral_return: None,
        total_collateral: None,
        reference_inputs,
        voting_procedures: BTreeMap::new(),
        proposal_procedures: Vec::new(),
        treasury_value: None,
        donation: None,
    };

    let vkey_witnesses = tx
        .vkey_witnesses()
        .iter()
        .map(|w| VKeyWitness {
            vkey: w.vkey.to_vec(),
            signature: w.signature.to_vec(),
        })
        .collect();

    let witness_set = TransactionWitnessSet {
        vkey_witnesses,
        native_scripts: Vec::new(),
        bootstrap_witnesses: Vec::new(),
        plutus_v1_scripts: Vec::new(),
        plutus_v2_scripts: Vec::new(),
        plutus_v3_scripts: Vec::new(),
        plutus_data: Vec::new(),
        redeemers: Vec::new(),
    };

    Ok(Transaction {
        hash: tx_hash,
        body,
        witness_set,
        is_valid: tx.is_valid(),
        auxiliary_data: None,
    })
}

fn convert_required_signers(tx: &PallasTx) -> Vec<Hash32> {
    use pallas_traverse::MultiEraSigners;
    match tx.required_signers() {
        MultiEraSigners::AlonzoCompatible(signers) => signers
            .iter()
            .map(|h| pallas_hash_to_torsten32(&pallas_crypto::hash::Hash::from(h.as_ref())))
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

    Ok(TransactionOutput {
        address,
        value,
        datum,
        script_ref: None,
    })
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
                hash: pallas_hash_to_torsten32(&pallas_crypto::hash::Hash::from(m.hash.as_ref())),
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
        _ => None, // GenesisKeyDelegation, MoveInstantaneousRewards
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
                hash: pallas_hash_to_torsten32(&pallas_crypto::hash::Hash::from(m.hash.as_ref())),
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
        CC::Reg(cred, _deposit) => Some(Certificate::StakeRegistration(
            convert_pallas_stake_credential(cred),
        )),
        CC::UnReg(cred, _refund) => Some(Certificate::StakeDeregistration(
            convert_pallas_stake_credential(cred),
        )),
        _ => None, // Other Conway governance certs handled later
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
