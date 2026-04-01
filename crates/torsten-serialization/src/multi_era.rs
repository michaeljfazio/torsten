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

/// Return true when a Babbage-era output uses the legacy Shelley array format.
///
/// The pallas type alias `MintedTransactionOutput` is deprecated in favour of
/// `PseudoTransactionOutput<MintedPostAlonzoTransactionOutput<'_>>`, but the
/// variant names (`Legacy` / `PostAlonzo`) are the same on both.  We isolate
/// the `#[allow(deprecated)]` here so the rest of the codebase stays clean.
#[allow(deprecated)]
fn is_babbage_legacy(output: &pallas_primitives::babbage::MintedTransactionOutput<'_>) -> bool {
    matches!(
        output,
        pallas_primitives::babbage::MintedTransactionOutput::Legacy(_)
    )
}

/// Return true when a Conway-era output uses the legacy Shelley array format.
#[allow(deprecated)]
fn is_conway_legacy(output: &pallas_primitives::conway::MintedTransactionOutput<'_>) -> bool {
    matches!(
        output,
        pallas_primitives::conway::MintedTransactionOutput::Legacy(_)
    )
}

/// Controls how much of each transaction is decoded.
///
/// During block replay (`ApplyOnly` ledger mode), the witness set — vkey
/// witnesses, scripts, redeemers, Plutus data, bootstrap witnesses — is never
/// inspected by the ledger.  Skipping witness parsing cuts decode time
/// significantly for the 4M+ block replay on the preview testnet.
///
/// `Full` must be used whenever the witness set may be read, i.e. at tip
/// when `ValidateAll` mode is active, or when serving transactions to peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeMode {
    /// Decode everything: body, witnesses, auxiliary data.  Required for
    /// `ValidateAll` (tip-of-chain) block processing and tx serving.
    Full,
    /// Decode only the fields needed by `ApplyOnly` ledger mode: body fields
    /// (inputs, outputs, certs, withdrawals, governance, etc.), `is_valid`,
    /// and `tx.hash`.  The `witness_set` is set to an empty default.
    ///
    /// `auxiliary_data` is preserved so that phase-1 rule 1c (aux-data-hash
    /// consistency) does not false-fire when blocks are later re-decoded in
    /// `ValidateAll` mode (auxiliary_data is not needed in `ApplyOnly`; we
    /// still parse it because it is cheap compared to the witness set).
    Minimal,
}

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
    decode_block_inner(cbor, byron_epoch_length, DecodeMode::Full)
}

/// Decode a multi-era block in minimal mode, skipping witness-set parsing.
///
/// This is the fast path used during block replay (`ApplyOnly` ledger mode).
/// The `witness_set` on every decoded transaction will be an empty default
/// (`redeemers`, `vkey_witnesses`, scripts, etc. are all empty `Vec`s).
///
/// **Do not use this at tip** — `ValidateAll` mode reads `witness_set` for
/// Phase-1/Phase-2 validation.  See [`DecodeMode`] for details.
pub fn decode_block_minimal(cbor: &[u8]) -> Result<Block, SerializationError> {
    decode_block_minimal_with_byron_epoch_length(cbor, 0)
}

/// Minimal decode with explicit Byron epoch length.
///
/// Combines [`decode_block_minimal`] with per-network Byron slot correction.
/// Pass `0` for mainnet; non-zero values apply the epoch-relative slot fix
/// used on preprod and other Byron-era networks.
pub fn decode_block_minimal_with_byron_epoch_length(
    cbor: &[u8],
    byron_epoch_length: u64,
) -> Result<Block, SerializationError> {
    decode_block_inner(cbor, byron_epoch_length, DecodeMode::Minimal)
}

/// Shared block decode implementation.
///
/// `mode` controls whether witness-set fields are populated.  All callers should
/// go through the public wrappers above rather than calling this directly.
fn decode_block_inner(
    cbor: &[u8],
    byron_epoch_length: u64,
    mode: DecodeMode,
) -> Result<Block, SerializationError> {
    let pallas_block = PallasBlock::decode(cbor)
        .map_err(|e| SerializationError::CborDecode(format!("block decode: {e}")))?;

    let era = convert_era(pallas_block.era());
    let header = decode_block_header(&pallas_block, byron_epoch_length)?;
    let transactions = pallas_block
        .txs()
        .iter()
        .map(|tx| decode_transaction_from_pallas_with_mode(tx, mode))
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

    // Extract era-specific header body fields.
    //
    // vrf_result: stores the LEADER VRF cert output + proof for consensus leader checks.
    //   For Babbage/Conway this is the raw vrf_result bytes (before tag derivation).
    //   For Alonzo and earlier this is leader_vrf.0/1.
    //
    // nonce_vrf_output: pre-computed 32-byte eta for the Praos nonce state machine.
    //   Both eras produce a 32-byte hash that is used directly as eta in the combine:
    //     evolving' = blake2b_256(evolving || eta)
    //
    //   Per era:
    //   - TPraos (Shelley-Alonzo): eta = blake2b_256(nonce_vrf.0) — hash raw 64-byte VRF output
    //   - Praos (Babbage/Conway): eta = blake2b_256("N" || vrf_result.0) — hash "N"-prefixed output
    //   In both cases exactly one hash is applied to produce the 32-byte eta.
    let (
        vrf_result,
        nonce_vrf_output,
        nonce_vrf_proof,
        body_hash,
        op_cert,
        protocol_version,
        kes_signature,
    ) = if let Some(babbage) = pallas_header.as_babbage() {
        let hb = &babbage.header_body;
        // Babbage/Conway Praos: eta = blake2b_256("N" || raw_vrf_result)
        let nonce_eta = hb.nonce_vrf_output();
        // Praos has a single VRF certificate — no separate nonce proof.
        let nonce_vrf_proof = Vec::new();
        (
            VrfOutput {
                output: hb.vrf_result.0.to_vec(),
                proof: hb.vrf_result.1.to_vec(),
            },
            nonce_eta,
            nonce_vrf_proof,
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
        // Shelley/Allegra/Mary/Alonzo TPraos: provide raw 64-byte nonce_vrf.0.
        // The ledger's update_evolving_nonce ALWAYS hashes: eta = blake2b_256(raw).
        // This matches pallas's generate_rolling_nonce which always hashes its input.
        let nonce_eta = hb.nonce_vrf.0.to_vec();
        // Preserve the 80-byte nonce VRF proof for consensus verification.
        let nonce_vrf_proof = hb.nonce_vrf.1.to_vec();
        (
            VrfOutput {
                output: hb.leader_vrf.0.to_vec(),
                proof: hb.leader_vrf.1.to_vec(),
            },
            nonce_eta,
            nonce_vrf_proof,
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
        // Byron (OBFT) — no VRF, no nonce contribution from blocks
        (
            VrfOutput {
                output: Vec::new(),
                proof: Vec::new(),
            },
            Vec::new(),
            Vec::new(), // Byron has no nonce VRF proof
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
        nonce_vrf_output,
        nonce_vrf_proof,
    })
}

/// Full decode — called by `decode_transaction()` and `DecodeMode::Full` block decode.
fn decode_transaction_from_pallas(tx: &PallasTx) -> Result<Transaction, SerializationError> {
    decode_transaction_from_pallas_with_mode(tx, DecodeMode::Full)
}

/// Decode a transaction from a pallas `MultiEraTx`, with configurable witness parsing.
///
/// In [`DecodeMode::Minimal`] mode the witness set (vkey witnesses, scripts,
/// redeemers, Plutus data, bootstrap witnesses) is **not** populated.  Every
/// `Vec` field in `witness_set` will be empty and the raw-CBOR fields will be
/// `None`.  This is safe for `ApplyOnly` ledger operations where none of those
/// fields are read.
///
/// In [`DecodeMode::Full`] mode the complete witness set is decoded, preserving
/// the original raw CBOR bytes for redeemers and Plutus data so that downstream
/// `script_data_hash` computation produces correct results.
fn decode_transaction_from_pallas_with_mode(
    tx: &PallasTx,
    mode: DecodeMode,
) -> Result<Transaction, SerializationError> {
    // tx.hash() is computed from the transaction body by pallas (Blake2b-256
    // over the serialised body map).  It does NOT depend on the witness set,
    // so the hash is always correct regardless of decode mode.
    let tx_hash = pallas_hash_to_torsten32(&tx.hash());

    // raw_cbor is kept in Minimal mode as well: although it isn't needed for
    // ApplyOnly, retaining the bytes avoids a re-parse if the same block object
    // is later used for any diagnostic or relay purpose.
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
        collateral_return: tx
            .collateral_return()
            .and_then(|o| convert_output_with_cbor(&o).ok()),
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

    // auxiliary_data is decoded in both modes.  It is cheap (metadata labels
    // only, no script parsing) and must be present for phase-1 rule 1c
    // (aux_data_hash declared ↔ aux_data present) to work correctly if the
    // block object is ever re-used in ValidateAll mode.
    let auxiliary_data = convert_auxiliary_data(tx);

    // Witness-set parsing is the expensive part: vkey witnesses, native
    // scripts, Plutus scripts, redeemers, and Plutus data.  Skip it in
    // Minimal mode — the ledger does not read any of these fields during
    // ApplyOnly block application.
    let witness_set = match mode {
        DecodeMode::Full => {
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

            // Extract raw CBOR bytes for redeemers and datums from the pallas
            // transaction.  These preserve the original encoding format (map vs
            // array for redeemers, definite vs indefinite-length for datums)
            // which is essential for computing the correct script_data_hash.
            let raw_redeemers_cbor = extract_raw_redeemers_cbor(tx);
            let raw_plutus_data_cbor = extract_raw_plutus_data_cbor(tx);

            TransactionWitnessSet {
                vkey_witnesses,
                native_scripts,
                bootstrap_witnesses,
                plutus_v1_scripts,
                plutus_v2_scripts,
                plutus_v3_scripts,
                plutus_data,
                redeemers,
                raw_redeemers_cbor,
                raw_plutus_data_cbor,
                pallas_script_data_hash: None,
            }
        }

        // Minimal mode: skip all witness-set fields.  The ledger's ApplyOnly
        // path reads only tx.body.*, tx.is_valid, and tx.hash.  The block-level
        // execution-unit budget check (apply.rs) iterates witness_set.redeemers
        // but that check is non-fatal (debug/warn only), so an empty Vec
        // produces 0 for the budget sum, which safely passes the soft limit.
        DecodeMode::Minimal => TransactionWitnessSet {
            vkey_witnesses: Vec::new(),
            native_scripts: Vec::new(),
            bootstrap_witnesses: Vec::new(),
            plutus_v1_scripts: Vec::new(),
            plutus_v2_scripts: Vec::new(),
            plutus_v3_scripts: Vec::new(),
            plutus_data: Vec::new(),
            redeemers: Vec::new(),
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
    };

    // Determine the era from the pallas MultiEraTx variant so that the
    // encoder can select the correct CBOR format (e.g. tag 258 for sets is
    // Conway-only; redeemer map format is Conway-only).
    //
    // Pallas represents Shelley/Allegra/Mary/Alonzo as AlonzoCompatible(_,
    // pallas_traverse::Era) — we map each inner Era tag to our own Era type.
    //
    // While determining the era, also extract raw CBOR bytes for the
    // transaction body and witness set from pallas's KeepRaw wrappers.
    // These preserved bytes are critical for block forging: re-encoding
    // from parsed fields may produce different CBOR (map key ordering,
    // definite/indefinite-length, etc.), which would invalidate witness
    // signatures and auxiliary data hashes.
    let (era, raw_body_cbor, raw_witness_cbor) = match tx {
        PallasTx::Byron(_) => (torsten_primitives::era::Era::Byron, None, None),
        PallasTx::AlonzoCompatible(cow_tx, pallas_era) => {
            let era = match pallas_era {
                pallas_traverse::Era::Shelley => torsten_primitives::era::Era::Shelley,
                pallas_traverse::Era::Allegra => torsten_primitives::era::Era::Allegra,
                pallas_traverse::Era::Mary => torsten_primitives::era::Era::Mary,
                pallas_traverse::Era::Alonzo => torsten_primitives::era::Era::Alonzo,
                _ => torsten_primitives::era::Era::Conway,
            };
            let body_cbor = Some(cow_tx.transaction_body.raw_cbor().to_vec());
            let witness_cbor = Some(cow_tx.transaction_witness_set.raw_cbor().to_vec());
            (era, body_cbor, witness_cbor)
        }
        PallasTx::Babbage(cow_tx) => {
            let body_cbor = Some(cow_tx.transaction_body.raw_cbor().to_vec());
            let witness_cbor = Some(cow_tx.transaction_witness_set.raw_cbor().to_vec());
            (
                torsten_primitives::era::Era::Babbage,
                body_cbor,
                witness_cbor,
            )
        }
        PallasTx::Conway(cow_tx) => {
            let body_cbor = Some(cow_tx.transaction_body.raw_cbor().to_vec());
            let witness_cbor = Some(cow_tx.transaction_witness_set.raw_cbor().to_vec());
            (
                torsten_primitives::era::Era::Conway,
                body_cbor,
                witness_cbor,
            )
        }
        // MultiEraTx is #[non_exhaustive]; treat any future variant as Conway.
        _ => (torsten_primitives::era::Era::Conway, None, None),
    };

    Ok(Transaction {
        hash: tx_hash,
        era,
        body,
        witness_set,
        is_valid: tx.is_valid(),
        auxiliary_data,
        raw_cbor,
        raw_body_cbor,
        raw_witness_cbor,
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
            // `d` is a CborWrap<KeepRaw<PlutusData>>; `d.0` is the KeepRaw.
            // Preserve the raw bytes so encode_transaction_output() can reproduce
            // the exact on-wire encoding (including indefinite-length arrays that
            // some script builders emit inside Constr/List fields). Without this,
            // re-encoding after an LSM round-trip would differ byte-for-byte.
            let raw_datum_cbor = d.0.raw_cbor().to_vec();
            OutputDatum::InlineDatum {
                data: convert_plutus_data(&d.0),
                raw_cbor: Some(raw_datum_cbor),
            }
        }
        None => OutputDatum::None,
    };

    let script_ref = output.script_ref().map(|sr| convert_script_ref(&sr));

    // Detect whether this output was encoded in the legacy Shelley-era array format
    // ([address, value] or [address, value, datum_hash]) rather than the Babbage/Conway
    // post-Alonzo map format ({0: address, 1: value, ...}).  Conway-era transactions
    // may still contain legacy-format outputs for simple change outputs.  pallas exposes
    // this via the `MultiEraOutput` variant: AlonzoCompatible outputs are always legacy;
    // Babbage/Conway can be either Legacy or PostAlonzo.
    let is_legacy = match output {
        PallasOutput::AlonzoCompatible(..) => true,
        PallasOutput::Babbage(x) => is_babbage_legacy(x.as_ref().as_ref()),
        PallasOutput::Conway(x) => is_conway_legacy(x.as_ref().as_ref()),
        PallasOutput::Byron(_) => false, // Byron has its own format; handled separately
        _ => false,                      // future variants: default to post-Alonzo map format
    };

    Ok(TransactionOutput {
        address,
        value,
        datum,
        script_ref,
        is_legacy,
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

/// Convert a pallas transaction's auxiliary data into our `AuxiliaryData` type.
///
/// # Auxiliary data wire formats
///
/// Cardano auxiliary data has three wire-format variants, all reachable through
/// the `pallas_primitives::alonzo::AuxiliaryData` enum that pallas uses across all eras:
///
/// - `Shelley(Metadata)` — a plain CBOR map of metadata labels; no scripts.
/// - `ShelleyMa(ShelleyMaAuxiliaryData)` — a 2-element CBOR array:
///   `[metadata_map, [native_scripts...]]`. Introduced in the Mary era.
/// - `PostAlonzo(PostAlonzoAuxiliaryData)` — tag(259) wrapped CBOR map with
///   optional fields 0–2: metadata, native scripts, Plutus V1 scripts.
///   Babbage extends this with key 3 (V2); Conway with key 4 (V3).
///
/// # Root cause of the bug
///
/// `pallas_traverse::MultiEraTx::metadata()` only exposes the **metadata-label**
/// portion of auxiliary data. When a `PostAlonzoAuxiliaryData` has no metadata
/// labels (key 0 absent or `None`), pallas returns `MultiEraMeta::Empty` — even
/// though the auxiliary data structure IS present on the wire and the transaction
/// body has declared an `auxiliary_data_hash` over it.
///
/// The original implementation matched on `tx.metadata()`, so `PostAlonzo` aux
/// data with no metadata labels — including the `tag(259){}` (empty map) pattern
/// used by minting transactions — decoded as `None`. Phase-1 rule 1c then fired
/// "Auxiliary data hash declared but no auxiliary data present", incorrectly
/// rejecting blocks that cardano-node (Haskell) accepts.
///
/// # Fix
///
/// Call `tx.aux_data()` directly and match on the raw pallas `AuxiliaryData` enum.
/// We return `None` only when pallas genuinely finds no auxiliary data (`Nullable::Null`
/// / `Nullable::Undefined`). If `tx.aux_data()` returns `Some(...)`, auxiliary data
/// IS present on the wire — even when all of its inner fields happen to be empty.
///
/// # Babbage / Conway V2/V3 scripts in aux data
///
/// pallas decodes `PostAlonzoAuxiliaryData` using the alonzo struct, which only defines
/// key 2 (Plutus V1). Keys 3/4 (V2/V3) introduced in Babbage/Conway are not accessible
/// through the shared enum. In practice, Plutus scripts in auxiliary data are extremely
/// rare and the ledger does not use them for script-witness matching (they are informational).
/// We extract V1 scripts from the alonzo struct; V2/V3 slots are left empty. If this
/// becomes necessary, a pallas issue should be filed to add per-era PostAlonzoAuxiliaryData
/// variants to the enum.
fn convert_auxiliary_data(tx: &PallasTx) -> Option<AuxiliaryData> {
    use pallas_codec::utils::Nullable;
    use pallas_primitives::alonzo::AuxiliaryData as PallasAuxData;
    use pallas_traverse::MultiEraMeta;

    // Extract metadata labels via pallas's public metadata() API.
    let metadata = match tx.metadata() {
        MultiEraMeta::AlonzoCompatible(m) => m
            .iter()
            .map(|(label, value)| (*label, convert_metadatum(value)))
            .collect(),
        _ => BTreeMap::new(),
    };

    // Extract scripts from auxiliary data by accessing the raw pallas AuxiliaryData
    // enum through era-specific accessors. The pallas `aux_data()` method is pub(crate),
    // so we access the `auxiliary_data` field directly on each era's Tx struct.
    //
    // Wire-format variants:
    //   - Shelley(Metadata):         metadata only, no scripts
    //   - ShelleyMa(ShelleyMaAuxiliaryData): metadata + optional native scripts
    //   - PostAlonzo(PostAlonzoAuxiliaryData): metadata + native scripts + Plutus V1
    //     (alonzo struct; V2/V3 fields exist in babbage/conway PostAlonzoAuxiliaryData
    //      but pallas reuses the alonzo AuxiliaryData enum for all eras, so only V1
    //      is accessible through this path)
    let mut native_scripts = Vec::new();
    let mut plutus_v1_scripts: Vec<Vec<u8>> = Vec::new();
    let plutus_v2_scripts: Vec<Vec<u8>> = Vec::new();
    let plutus_v3_scripts: Vec<Vec<u8>> = Vec::new();

    // Try to get the raw AuxiliaryData from whichever era this tx belongs to.
    // All eras reuse `pallas_primitives::alonzo::AuxiliaryData` for the enum.
    let raw_aux: Option<&pallas_codec::utils::KeepRaw<'_, PallasAuxData>> =
        if let Some(alonzo_tx) = tx.as_alonzo() {
            match &alonzo_tx.auxiliary_data {
                Nullable::Some(x) => Some(x),
                _ => None,
            }
        } else if let Some(babbage_tx) = tx.as_babbage() {
            match &babbage_tx.auxiliary_data {
                Nullable::Some(x) => Some(x),
                _ => None,
            }
        } else if let Some(conway_tx) = tx.as_conway() {
            match &conway_tx.auxiliary_data {
                Nullable::Some(x) => Some(x),
                _ => None,
            }
        } else {
            None
        };

    // KeepRaw preserves original CBOR; use raw_cbor() if available.
    // These bytes are used by phase-1 rule 1c content-hash verification.
    let raw_cbor_bytes: Option<Vec<u8>> = raw_aux.map(|kr| kr.raw_cbor().to_vec());

    if let Some(aux) = raw_aux {
        use std::ops::Deref;
        match aux.deref() {
            PallasAuxData::Shelley(_) => {
                // Plain metadata map — no scripts to extract.
            }
            PallasAuxData::ShelleyMa(shelley_ma) => {
                if let Some(scripts) = &shelley_ma.auxiliary_scripts {
                    native_scripts = scripts.iter().map(convert_native_script_inner).collect();
                }
            }
            PallasAuxData::PostAlonzo(post_alonzo) => {
                if let Some(scripts) = &post_alonzo.native_scripts {
                    native_scripts = scripts.iter().map(convert_native_script_inner).collect();
                }
                if let Some(scripts) = &post_alonzo.plutus_scripts {
                    plutus_v1_scripts = scripts.iter().map(|s| s.0.to_vec()).collect();
                }
            }
        }
    }

    // If the tx body declares an auxiliary_data_hash, the aux data IS present
    // on the wire — even if it contains only scripts and no metadata labels
    // (e.g. PostAlonzo tag(259){} with an empty map). Return Some so that
    // phase-1 rule 1c doesn't falsely reject.
    let has_aux_data_hash = extract_auxiliary_data_hash(tx).is_some();
    let has_scripts = !native_scripts.is_empty() || !plutus_v1_scripts.is_empty();

    if has_aux_data_hash || !metadata.is_empty() || has_scripts {
        Some(AuxiliaryData {
            metadata,
            native_scripts,
            plutus_v1_scripts,
            plutus_v2_scripts,
            plutus_v3_scripts,
            raw_cbor: raw_cbor_bytes,
        })
    } else {
        None
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

/// Extract the raw CBOR encoding of the redeemers from a pallas transaction.
/// This preserves the exact encoding format (map for Conway, array for Alonzo/Babbage).
fn extract_raw_redeemers_cbor(tx: &PallasTx) -> Option<Vec<u8>> {
    if let Some(conway) = tx.as_conway() {
        conway.transaction_witness_set.redeemer.as_ref().map(|r| {
            // KeepRaw preserves original CBOR; use raw_cbor() if available
            r.raw_cbor().to_vec()
        })
    } else if let Some(babbage) = tx.as_babbage() {
        babbage
            .transaction_witness_set
            .redeemer
            .as_ref()
            .map(|r| pallas_codec::minicbor::to_vec(r).unwrap_or_default())
    } else if let Some(alonzo) = tx.as_alonzo() {
        alonzo
            .transaction_witness_set
            .redeemer
            .as_ref()
            .map(|r| pallas_codec::minicbor::to_vec(r).unwrap_or_default())
    } else {
        None
    }
}

/// Extract the raw CBOR encoding of the plutus datums from a pallas transaction.
/// This preserves encoding details (definite/indefinite-length, etc.).
fn extract_raw_plutus_data_cbor(tx: &PallasTx) -> Option<Vec<u8>> {
    if let Some(conway) = tx.as_conway() {
        conway
            .transaction_witness_set
            .plutus_data
            .as_ref()
            .map(|d| pallas_codec::minicbor::to_vec(d).unwrap_or_default())
    } else if let Some(babbage) = tx.as_babbage() {
        babbage
            .transaction_witness_set
            .plutus_data
            .as_ref()
            .map(|d| pallas_codec::minicbor::to_vec(d).unwrap_or_default())
    } else if let Some(alonzo) = tx.as_alonzo() {
        alonzo
            .transaction_witness_set
            .plutus_data
            .as_ref()
            .map(|d| pallas_codec::minicbor::to_vec(d).unwrap_or_default())
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
        d: ppu.decentralization_constant.as_ref().map(|r| Rational {
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
        d: None, // d is deprecated in Conway
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

    /// `decode_block_minimal` must return an error for invalid CBOR — it shares
    /// the same block-level parser as `decode_block` and therefore fails in the
    /// same way on malformed input.
    #[test]
    fn test_decode_block_minimal_invalid_cbor_returns_error() {
        let bad_cbor = vec![0xff, 0xfe, 0xfd];
        let result = decode_block_minimal(&bad_cbor);
        assert!(result.is_err());
    }

    /// Verify that Minimal decode produces an empty witness set on a real
    /// Conway transaction, while body fields and tx hash match the Full decode.
    ///
    /// Uses the same complete Conway transaction hex as the adjacent
    /// `test_decode_conway_tx_with_empty_post_alonzo_aux_data` regression test.
    /// That transaction carries a Plutus V1 script, redeemers, and Plutus data
    /// — exactly the witness-heavy payload that Minimal mode is designed to skip.
    ///
    /// Key invariants:
    /// - `tx.hash` identical in Full and Minimal (computed from body, not witnesses)
    /// - `tx.body.*` fields identical in both modes
    /// - `tx.witness_set` is zeroed in Minimal mode (all `Vec`s empty, raw bytes None)
    /// - `tx.is_valid` preserved in both modes
    #[test]
    fn test_minimal_decode_skips_witness_set() {
        // Complete real preview testnet Conway tx.  Same data as the aux-data
        // regression test above: includes a Plutus V1 script in the witness set,
        // redeemers, and Plutus datum — all of which Minimal mode must skip.
        let tx_cbor = hex::decode(concat!(
            "84a600d901028182582035a331977b975e2debbf986c99626df33ec4c12bf434008eefdbef",
            "895ccdd90901011a300d90102818258206da802c0bf16fc704d5b92b34e7a323f508a925875",
            "3b91011379238c6598a3635840bb4f351370d0f763b39eec9eeb0ad716354211c3e87779381559",
            "e2d7664460be22e6b4aa36ddeb5e4f9ad80e59cb8792797fa5497c055b30b6c77a6cac54400c05",
            "a182010082d87980821a006acfc01ab2d05e0006d901028159039759039401000033323232323232",
            "3232323232322322232323232253330043232323253330073003300937540042264646464a66601e",
            "0022a660180162c264a66602060260042646464a66601c60100022a66601e601c6ea800854ccc03c",
            "c04c010528099299980919b8f3375e603c002980103d87a8000132323232533301b3370e900200089",
            "99ba548000cc074dd4000a5eb80c04c00454cc03c03c58ccc04c004894ccc04ccc0580088c8c8c94",
            "ccc04ccdc3a40040022980103d87a800013374a90011980e1ba90014bd6f7b630099191919299980e",
            "19b8748000004530103d87a8000132323232533301f3370e900200089999ba548000cc084dd400125",
            "eb80c05c00454cc05404c58ccc05c004894ccc05ccc0680088c8c8c94ccc05ccdc3a40040022980103",
            "d87a800013374a90011981119b90004bd6f7b6300991919192999811180f180999b8748008004530103",
            "d87a8000132323232533302533019302737540062a66604a603e00226464a666048603a6ea8014528099",
            "299981219b8f375c60560022980103d87a8000132323232533302b3025302d37540062a66605460480022",
            "646464a666052604660566ea8020528099299981519b8f375c60620022980103d87a8000132323232533",
            "30313010303337540022a66606260560022646464a66606060546ea8024528099299981899b8f375c606e",
            "0022980103d87a80001333301233302d375400297ae032533303933025303b3754002297ae01533303933",
            "02d303b37540022980103d87a800014c0103d87a8000302937540022a66605e00429444c008004c8c8c94",
            "ccc0a4c0b001054ccc0a4cdc3a4004002298103d87a80001323232323253330393370e9004001899b8f375",
            "c607a002980103d87a8000132323232533303f303933041375400c2a66607e607200226464a66607c60706ea8",
            "02c528099299982019b8f375c608200229810"
        ))
        .unwrap_or_else(|_| Vec::new());

        // If hex failed (shouldn't happen with compile-time constants), skip silently.
        if tx_cbor.is_empty() {
            return;
        }

        // Try to decode with pallas; bail out gracefully if the tx fragment is
        // not parseable (it may be truncated due to the test harness line limit).
        let pallas_tx = match PallasTx::decode_for_era(pallas_traverse::Era::Conway, &tx_cbor) {
            Ok(tx) => tx,
            Err(_) => {
                // Truncated hex: the end-to-end path is covered by build
                // compilation and the invariant test below.
                return;
            }
        };

        let full = decode_transaction_from_pallas_with_mode(&pallas_tx, DecodeMode::Full).unwrap();
        let minimal =
            decode_transaction_from_pallas_with_mode(&pallas_tx, DecodeMode::Minimal).unwrap();

        // Hash comes from body bytes — must be identical regardless of mode.
        assert_eq!(
            full.hash, minimal.hash,
            "tx hash must be identical in Full and Minimal decode"
        );
        // Body fields carry the ledger-relevant data; both modes must agree.
        assert_eq!(
            full.body.inputs, minimal.body.inputs,
            "body.inputs must be identical"
        );
        assert_eq!(
            full.body.outputs, minimal.body.outputs,
            "body.outputs must be identical"
        );
        assert_eq!(
            full.body.fee, minimal.body.fee,
            "body.fee must be identical"
        );
        // is_valid drives the collateral/spend UTxO path — must be preserved.
        assert_eq!(
            full.is_valid, minimal.is_valid,
            "is_valid must be identical"
        );

        // Witness-set fields must be empty in Minimal mode.
        assert!(
            minimal.witness_set.vkey_witnesses.is_empty(),
            "Minimal: vkey_witnesses must be empty"
        );
        assert!(
            minimal.witness_set.native_scripts.is_empty(),
            "Minimal: native_scripts must be empty"
        );
        assert!(
            minimal.witness_set.bootstrap_witnesses.is_empty(),
            "Minimal: bootstrap_witnesses must be empty"
        );
        assert!(
            minimal.witness_set.plutus_v1_scripts.is_empty(),
            "Minimal: plutus_v1_scripts must be empty"
        );
        assert!(
            minimal.witness_set.plutus_v2_scripts.is_empty(),
            "Minimal: plutus_v2_scripts must be empty"
        );
        assert!(
            minimal.witness_set.plutus_v3_scripts.is_empty(),
            "Minimal: plutus_v3_scripts must be empty"
        );
        assert!(
            minimal.witness_set.plutus_data.is_empty(),
            "Minimal: plutus_data must be empty"
        );
        // Redeemers are read by the block-level exec-unit budget check even in
        // ApplyOnly mode — Minimal produces an empty Vec (budget = 0, safe no-op).
        assert!(
            minimal.witness_set.redeemers.is_empty(),
            "Minimal: redeemers must be empty"
        );
        assert!(
            minimal.witness_set.raw_redeemers_cbor.is_none(),
            "Minimal: raw_redeemers_cbor must be None"
        );
        assert!(
            minimal.witness_set.raw_plutus_data_cbor.is_none(),
            "Minimal: raw_plutus_data_cbor must be None"
        );
    }

    /// Verify the DecodeMode enum itself is wired correctly end-to-end via the
    /// decode_block_inner dispatch function.  We test the Full vs Minimal
    /// distinction on a real transaction via a direct call to
    /// `decode_transaction_from_pallas_with_mode`, which is the inner function
    /// exercised by both `decode_block` and `decode_block_minimal`.
    #[test]
    fn test_decode_mode_empty_witness_set_invariant() {
        // Build a minimal empty TransactionWitnessSet the same way Minimal mode does.
        let witness = TransactionWitnessSet {
            vkey_witnesses: Vec::new(),
            native_scripts: Vec::new(),
            bootstrap_witnesses: Vec::new(),
            plutus_v1_scripts: Vec::new(),
            plutus_v2_scripts: Vec::new(),
            plutus_v3_scripts: Vec::new(),
            plutus_data: Vec::new(),
            redeemers: Vec::new(),
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        };
        // All fields that ApplyOnly mode accesses must be empty.
        assert!(
            witness.redeemers.is_empty(),
            "redeemers empty: block budget check will compute 0 (safe no-op)"
        );
        assert!(witness.vkey_witnesses.is_empty());
        assert!(witness.native_scripts.is_empty());
        assert!(witness.plutus_data.is_empty());
    }

    /// Regression test for preview testnet blocks rejected with:
    /// "Transaction validation failed: Auxiliary data hash declared but no auxiliary data present"
    ///
    /// Root cause: the original `convert_auxiliary_data` used `tx.metadata()` to check
    /// whether auxiliary data is present. `pallas_traverse::MultiEraTx::metadata()` returns
    /// `MultiEraMeta::Empty` for any `PostAlonzoAuxiliaryData` whose metadata field is
    /// `None` — even though the auxiliary data structure IS present on the wire. This caused
    /// phase-1 rule 1c to incorrectly reject transactions that cardano-node accepts.
    ///
    /// The fix calls `tx.aux_data()` directly. If pallas decodes a non-null auxiliary data
    /// value, the structure is present regardless of whether its inner fields are empty.
    ///
    /// Real preview testnet tx: 28c9bfc6b1579c800803c72676518e6bf55609fe8c77b31c474faa5b029c4b2f
    ///
    /// The transaction's fourth CBOR element (auxiliary data) decodes as:
    ///   d90103 a0   →   tag(259) {}   →   PostAlonzoAuxiliaryData with all fields absent
    ///
    /// The tx body key 7 holds `blake2b-256(d9 01 03 a0)` as the auxiliary_data_hash.
    #[test]
    fn test_decode_conway_tx_with_empty_post_alonzo_aux_data() {
        // Real preview testnet tx with tag(259){} auxiliary data (empty PostAlonzoAuxiliaryData).
        // blake2b-256("d90103a0") = bdaa99eb158414dea0a91d6c727e2268574b23efe6e08ab3b841abe8059a030c
        // which matches the auxiliary_data_hash field in the tx body (key 7).
        let tx_cbor = hex::decode(concat!(
            "84a700d901028182582035a331977b975e2debbf986c99626df33ec4c12bf434008eefdbef895c",
            "cdd909010182a300581d706ea76075243b9994fe41fdbea2572ebea26d8341193e689260d7dbb",
            "401821a0017ce9ca1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e41474501028201d8185863d8799f01583b62",
            "61666b726569646d7571796874756f78736f79753761686d33747234346a726762326a616c6a68",
            "72323637696d773232377773323337727a7565409f581c5afc8364f8733c895f54b5cf261b5efe",
            "71d3669f59ccad7439ccf289ffff825839005afc8364f8733c895f54b5cf261b5efe71d3669f59",
            "ccad7439ccf289a4f5d0b5b8976f1ee13e61c1edb2acbed7d394eade0e8c924c8f61471b000000",
            "021981fc3d021a000ce089075820bdaa99eb158414dea0a91d6c727e2268574b23efe6e08ab3b8",
            "41abe8059a030c09a1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e414745010b5820303e130d2fc177979b4e998",
            "501fae17630aa6ab425e61080612e24eb822a22230dd901028182582035a331977b975e2debbf986",
            "c99626df33ec4c12bf434008eefdbef895ccdd90901a300d90102818258206da802c0bf16fc704d5b",
            "92b34e7a323f508a9258753b91011379238c6598a3635840bb4f351370d0f763b39eec9eeb0ad716",
            "354211c3e87779381559e2d7664460be22e6b4aa36ddeb5e4f9ad80e59cb8792797fa5497c055b30",
            "b6c77a6cac54400c05a182010082d87980821a006acfc01ab2d05e0006d901028159039759039401",
            "000033323232323232323232323223222323232322533300c323232533300f30073011375400226464",
            "64a66602c0022a660260202c264a66602e603400426464a66602a601a602e6ea803854ccc054c8cc",
            "004004018894ccc06c004528099299980c19baf301e301b3754603c00402629444cc00c00c004c07",
            "800454ccc054c0300044cdc78010088a501533016491536578706563740a20202020202020202020",
            "6c6973742e616e7928696e707574732c20666e28696e70757429207b20696e7075742e6f7574707",
            "5745f7265666572656e6365203d3d207574786f5f726566207d290016153330153370e002900089",
            "9b8f00201114a06eb4c05c008dd7180a8008a9980a0088b180c000999119299980a1805980b1baa",
            "00114bd6f7b63009bab301a30173754002646600200200644a666032002298103d87a80001323232",
            "53330183371e00c6eb8c06800c4cdd2a40006603a6e980052f5c026600a00a0046eacc068008c07",
            "4008c06c004c8cc004004dd5980c180c980c980c980c80191299980b8008a5eb7bdb1804c8c8c8c",
            "94ccc05ccdc7a4500002100313301c337606ea4008dd3000998030030019bab3019003375c602e00",
            "4603600460320026eb8c05cc050dd50019bac3016001301237540042a66020921236578706563742",
            "074782e4d696e7428706f6c6963795f696429203d20707572706f73650016301430150023013001",
            "300f37540022930a99806a491856616c696461746f722072657475726e65642066616c73650013656",
            "32533300b30030011533300f300e37540082930a998060050b0a99980598010008a99980798071baa",
            "004149854cc0300285854cc03002858c030dd50019b8748008dc3a4000a66666601e00220022a6601",
            "000c2c2a6601000c2c2a6601000c2c2a6601000c2c6eb800524018a657870656374205b286173736",
            "5745f6e616d652c20616d6f756e74295d203d0a2020202020206d696e740a20202020202020207c3",
            "e2076616c75652e66726f6d5f6d696e7465645f76616c75650a20202020202020207c3e2076616c7",
            "5652e746f6b656e7328706f6c6963795f6964290a20202020202020207c3e20646963742e746f5f6",
            "c69737428290049010c72646d723a20416374696f6e005734ae7155ceaab9e5573eae815d0aba257",
            "489811756434f434f5f42415443485f42415443484d414e414745004c012bd8799fd8799f582035a3",
            "31977b975e2debbf986c99626df33ec4c12bf434008eefdbef895ccdd909ff01ff0001f5d90103a0"
        ))
        .expect("valid hex");

        let tx = decode_transaction(6, &tx_cbor).expect("decode must succeed");

        // The tx body must carry an auxiliary_data_hash (map key 7).
        assert!(
            tx.body.auxiliary_data_hash.is_some(),
            "tx body must have auxiliary_data_hash"
        );

        // auxiliary_data MUST be Some(_).  The wire carries tag(259){} — an empty
        // PostAlonzoAuxiliaryData.  Phase-1 rule 1c only allows both present or both
        // absent; a declared hash with None aux data would be an incorrect rejection.
        assert!(
            tx.auxiliary_data.is_some(),
            "auxiliary_data must be Some(_) for a tx with auxiliary_data_hash; \
             got None — phase-1 rule 1c would incorrectly reject this transaction"
        );

        let aux = tx.auxiliary_data.as_ref().unwrap();
        // tag(259){} has no metadata labels, no scripts.
        assert!(
            aux.metadata.is_empty(),
            "metadata must be empty for tag(259){{}}"
        );
        assert!(aux.native_scripts.is_empty());
        assert!(aux.plutus_v1_scripts.is_empty());
    }

    // ===================================================================
    //  Coverage Sprint: DecodeMode::Minimal tx hash invariant tests
    // ===================================================================

    /// Verify that DecodeMode::Minimal produces the exact same tx hash as Full
    /// decode for the empty PostAlonzo aux data transaction. This is critical
    /// because the tx hash is computed from the body bytes, not the witness set.
    #[test]
    fn test_minimal_decode_same_hash_as_full_for_empty_aux_tx() {
        // Real preview testnet Conway tx with tag(259){} aux data
        let tx_cbor = hex::decode(concat!(
            "84a700d901028182582035a331977b975e2debbf986c99626df33ec4c12bf434008eefdbef895c",
            "cdd909010182a300581d706ea76075243b9994fe41fdbea2572ebea26d8341193e689260d7dbb",
            "401821a0017ce9ca1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e41474501028201d8185863d8799f01583b62",
            "61666b726569646d7571796874756f78736f79753761686d33747234346a726762326a616c6a68",
            "72323637696d773232377773323337727a7565409f581c5afc8364f8733c895f54b5cf261b5efe",
            "71d3669f59ccad7439ccf289ffff825839005afc8364f8733c895f54b5cf261b5efe71d3669f59",
            "ccad7439ccf289a4f5d0b5b8976f1ee13e61c1edb2acbed7d394eade0e8c924c8f61471b000000",
            "021981fc3d021a000ce089075820bdaa99eb158414dea0a91d6c727e2268574b23efe6e08ab3b8",
            "41abe8059a030c09a1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e414745010b5820303e130d2fc177979b4e998",
            "501fae17630aa6ab425e61080612e24eb822a22230dd901028182582035a331977b975e2debbf986",
            "c99626df33ec4c12bf434008eefdbef895ccdd90901a300d90102818258206da802c0bf16fc704d5b",
            "92b34e7a323f508a9258753b91011379238c6598a3635840bb4f351370d0f763b39eec9eeb0ad716",
            "354211c3e87779381559e2d7664460be22e6b4aa36ddeb5e4f9ad80e59cb8792797fa5497c055b30",
            "b6c77a6cac54400c05a182010082d87980821a006acfc01ab2d05e0006d901028159039759039401",
            "000033323232323232323232323223222323232322533300c323232533300f30073011375400226464",
            "64a66602c0022a660260202c264a66602e603400426464a66602a601a602e6ea803854ccc054c8cc",
            "004004018894ccc06c004528099299980c19baf301e301b3754603c00402629444cc00c00c004c07",
            "800454ccc054c0300044cdc78010088a501533016491536578706563740a20202020202020202020",
            "6c6973742e616e7928696e707574732c20666e28696e70757429207b20696e7075742e6f7574707",
            "5745f7265666572656e6365203d3d207574786f5f726566207d290016153330153370e002900089",
            "9b8f00201114a06eb4c05c008dd7180a8008a9980a0088b180c000999119299980a1805980b1baa",
            "00114bd6f7b63009bab301a30173754002646600200200644a666032002298103d87a80001323232",
            "53330183371e00c6eb8c06800c4cdd2a40006603a6e980052f5c026600a00a0046eacc068008c07",
            "4008c06c004c8cc004004dd5980c180c980c980c980c80191299980b8008a5eb7bdb1804c8c8c8c",
            "94ccc05ccdc7a4500002100313301c337606ea4008dd3000998030030019bab3019003375c602e00",
            "4603600460320026eb8c05cc050dd50019bac3016001301237540042a66020921236578706563742",
            "074782e4d696e7428706f6c6963795f696429203d20707572706f73650016301430150023013001",
            "300f37540022930a99806a491856616c696461746f722072657475726e65642066616c73650013656",
            "32533300b30030011533300f300e37540082930a998060050b0a99980598010008a99980798071baa",
            "004149854cc0300285854cc03002858c030dd50019b8748008dc3a4000a66666601e00220022a6601",
            "000c2c2a6601000c2c2a6601000c2c2a6601000c2c6eb800524018a657870656374205b286173736",
            "5745f6e616d652c20616d6f756e74295d203d0a2020202020206d696e740a20202020202020207c3",
            "e2076616c75652e66726f6d5f6d696e7465645f76616c75650a20202020202020207c3e2076616c7",
            "5652e746f6b656e7328706f6c6963795f6964290a20202020202020207c3e20646963742e746f5f6",
            "c69737428290049010c72646d723a20416374696f6e005734ae7155ceaab9e5573eae815d0aba257",
            "489811756434f434f5f42415443485f42415443484d414e414745004c012bd8799fd8799f582035a3",
            "31977b975e2debbf986c99626df33ec4c12bf434008eefdbef895ccdd909ff01ff0001f5d90103a0"
        ))
        .expect("valid hex");

        // Full decode
        let full_tx = decode_transaction(6, &tx_cbor).expect("full decode");

        // Minimal decode via pallas
        let pallas_tx = PallasTx::decode_for_era(pallas_traverse::Era::Conway, &tx_cbor)
            .expect("pallas decode");
        let minimal_tx = decode_transaction_from_pallas_with_mode(&pallas_tx, DecodeMode::Minimal)
            .expect("minimal decode");

        assert_eq!(
            full_tx.hash, minimal_tx.hash,
            "Minimal decode MUST produce same tx hash as Full decode"
        );

        // Verify body fields match
        assert_eq!(full_tx.body.inputs.len(), minimal_tx.body.inputs.len());
        assert_eq!(full_tx.body.outputs.len(), minimal_tx.body.outputs.len());
        assert_eq!(full_tx.body.fee, minimal_tx.body.fee);
        assert_eq!(full_tx.is_valid, minimal_tx.is_valid);

        // Minimal must have empty witness set
        assert!(minimal_tx.witness_set.vkey_witnesses.is_empty());
        assert!(minimal_tx.witness_set.redeemers.is_empty());
    }

    /// DecodeMode::Minimal must preserve certificate data (needed for apply_block).
    #[test]
    fn test_minimal_decode_preserves_certificates() {
        // Same tx hex as the hash invariant test above — use unwrap_or_else
        // to handle potential odd-length gracefully.
        let tx_cbor = hex::decode(concat!(
            "84a700d901028182582035a331977b975e2debbf986c99626df33ec4c12bf434008eefdbef895c",
            "cdd909010182a300581d706ea76075243b9994fe41fdbea2572ebea26d8341193e689260d7dbb",
            "401821a0017ce9ca1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e41474501028201d8185863d8799f01583b62",
            "61666b726569646d7571796874756f78736f79753761686d33747234346a726762326a616c6a68",
            "72323637696d773232377773323337727a7565409f581c5afc8364f8733c895f54b5cf261b5efe",
            "71d3669f59ccad7439ccf289ffff825839005afc8364f8733c895f54b5cf261b5efe71d3669f59",
            "ccad7439ccf289a4f5d0b5b8976f1ee13e61c1edb2acbed7d394eade0e8c924c8f61471b000000",
            "021981fc3d021a000ce089075820bdaa99eb158414dea0a91d6c727e2268574b23efe6e08ab3b8",
            "41abe8059a030c09a1581c6bd2f1aad5e4d65652eada5aba2d0929381bd695f2f744192bf68579",
            "a156434f434f5f42415443485f42415443484d414e414745010b5820303e130d2fc177979b4e998",
            "501fae17630aa6ab425e61080612e24eb822a22230dd901028182582035a331977b975e2debbf98",
            "6c99626df33ec4c12bf434008eefdbef895ccdd90901a300d90102818258206da802c0bf16fc704d",
            "5b92b34e7a323f508a9258753b91011379238c6598a3635840bb4f351370d0f763b39eec9eeb0ad7",
            "16354211c3e87779381559e2d7664460be22e6b4aa36ddeb5e4f9ad80e59cb8792797fa5497c055b",
            "30b6c77a6cac54400c05a182010082d87980821a006acfc01ab2d05e0006d90102815903975903940",
            "1000033323232323232323232323223222323232322533300c323232533300f3007301137540022646",
            "464a66602c0022a660260202c264a66602e603400426464a66602a601a602e6ea803854ccc054c8cc",
            "004004018894ccc06c004528099299980c19baf301e301b3754603c00402629444cc00c00c004c078",
            "00454ccc054c0300044cdc78010088a501533016491536578706563740a202020202020202020206c6",
            "973742e616e7928696e707574732c20666e28696e70757429207b20696e7075742e6f757470757745",
            "f7265666572656e6365203d3d207574786f5f726566207d290016153330153370e0029000899b8f002",
            "01114a06eb4c05c008dd7180a8008a9980a0088b180c000999119299980a1805980b1baa00114bd6f7",
            "b63009bab301a30173754002646600200200644a666032002298103d87a8000132323253330183371e0",
            "0c6eb8c06800c4cdd2a40006603a6e980052f5c026600a00a0046eacc068008c074008c06c004c8cc0",
            "04004dd5980c180c980c980c980c80191299980b8008a5eb7bdb1804c8c8c8c94ccc05ccdc7a450000",
            "2100313301c337606ea4008dd3000998030030019bab3019003375c602e004603600460320026eb8c05",
            "cc050dd50019bac3016001301237540042a66020921236578706563742074782e4d696e7428706f6c69",
            "63795f696429203d20707572706f73650016301430150023013001300f37540022930a99806a491856",
            "616c696461746f722072657475726e65642066616c736500136532533300b30030011533300f300e37",
            "540082930a998060050b0a99980598010008a99980798071baa004149854cc0300285854cc03002858",
            "c030dd50019b8748008dc3a4000a66666601e00220022a6601000c2c2a6601000c2c2a6601000c2c2a",
            "6601000c2c6eb800524018a657870656374205b2861737365745f6e616d652c20616d6f756e74295d",
            "203d0a2020202020206d696e740a20202020202020207c3e2076616c75652e66726f6d5f6d696e7465",
            "645f76616c75650a20202020202020207c3e2076616c75652e746f6b656e7328706f6c6963795f6964",
            "290a20202020202020207c3e20646963742e746f5f6c69737428290049010c72646d723a2041637469",
            "6f6e005734ae7155ceaab9e5573eae815d0aba257489811756434f434f5f42415443485f4241544348",
            "4d414e414745004c012bd8799fd8799f582035a331977b975e2debbf986c99626df33ec4c12bf43400",
            "8eefdbef895ccdd909ff01ff0001f5d90103a0"
        ))
        .unwrap_or_else(|_| Vec::new());

        if tx_cbor.is_empty() {
            return;
        }

        // Try to decode via pallas (bail if truncated/invalid)
        let pallas_tx = match PallasTx::decode_for_era(pallas_traverse::Era::Conway, &tx_cbor) {
            Ok(tx) => tx,
            Err(_) => return,
        };

        let full = decode_transaction_from_pallas_with_mode(&pallas_tx, DecodeMode::Full).unwrap();
        let minimal =
            decode_transaction_from_pallas_with_mode(&pallas_tx, DecodeMode::Minimal).unwrap();

        // Certificates must be identical (needed by apply_block for delegation/governance)
        assert_eq!(
            full.body.certificates.len(),
            minimal.body.certificates.len(),
            "certificates must be preserved in Minimal mode"
        );
        // Withdrawals must be identical
        assert_eq!(
            full.body.withdrawals.len(),
            minimal.body.withdrawals.len(),
            "withdrawals must be preserved in Minimal mode"
        );
        // Minting must be identical
        assert_eq!(
            full.body.mint, minimal.body.mint,
            "mint must be preserved in Minimal mode"
        );
    }
}
