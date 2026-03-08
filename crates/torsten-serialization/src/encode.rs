use crate::cbor::*;
use std::collections::BTreeMap;
use torsten_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::hash::{blake2b_256, Hash28, Hash32};
use torsten_primitives::transaction::*;
use torsten_primitives::value::{AssetName, Value};

/// Encode a Value to CBOR.
///
/// Pure ADA: just the coin amount.
/// Multi-asset: [coin, {policy_id: {asset_name: quantity}, ...}]
pub fn encode_value(value: &Value) -> Vec<u8> {
    if value.is_pure_ada() {
        encode_uint(value.coin.0)
    } else {
        let mut buf = encode_array_header(2);
        buf.extend(encode_uint(value.coin.0));
        buf.extend(encode_multi_asset(&value.multi_asset));
        buf
    }
}

/// Encode multi-asset map: {policy_id: {asset_name: quantity}}
fn encode_multi_asset(multi_asset: &BTreeMap<Hash28, BTreeMap<AssetName, u64>>) -> Vec<u8> {
    let mut buf = encode_map_header(multi_asset.len());
    for (policy_id, assets) in multi_asset {
        buf.extend(encode_hash28(policy_id));
        buf.extend(encode_map_header(assets.len()));
        for (asset_name, qty) in assets {
            buf.extend(encode_bytes(&asset_name.0));
            buf.extend(encode_uint(*qty));
        }
    }
    buf
}

/// Encode mint map: {policy_id: {asset_name: i64}}
fn encode_mint(mint: &BTreeMap<Hash28, BTreeMap<AssetName, i64>>) -> Vec<u8> {
    let mut buf = encode_map_header(mint.len());
    for (policy_id, assets) in mint {
        buf.extend(encode_hash28(policy_id));
        buf.extend(encode_map_header(assets.len()));
        for (asset_name, qty) in assets {
            buf.extend(encode_bytes(&asset_name.0));
            buf.extend(encode_int(*qty as i128));
        }
    }
    buf
}

/// Encode a transaction output (Babbage/Conway post-Alonzo map format).
///
/// Map with keys: 0=address, 1=value, 2=datum_option, 3=script_ref
pub fn encode_transaction_output(output: &TransactionOutput) -> Vec<u8> {
    let mut count = 2; // address + value are always present
    if output.datum != OutputDatum::None {
        count += 1;
    }
    if output.script_ref.is_some() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    // 0: address
    buf.extend(encode_uint(0));
    buf.extend(encode_bytes(&output.address.to_bytes()));

    // 1: value
    buf.extend(encode_uint(1));
    buf.extend(encode_value(&output.value));

    // 2: datum_option
    match &output.datum {
        OutputDatum::None => {}
        OutputDatum::DatumHash(h) => {
            buf.extend(encode_uint(2));
            // [0, datum_hash]
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(0));
            buf.extend(encode_hash32(h));
        }
        OutputDatum::InlineDatum(data) => {
            buf.extend(encode_uint(2));
            // [1, #6.24(cbor_encoded_data)]
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(1));
            // Tag 24 (CBOR-encoded data item)
            buf.extend(encode_tag(24));
            let encoded_data = encode_plutus_data(data);
            buf.extend(encode_bytes(&encoded_data));
        }
    }

    // 3: script_ref
    if let Some(script_ref) = &output.script_ref {
        buf.extend(encode_uint(3));
        // Tag 24 (CBOR-encoded data item)
        buf.extend(encode_tag(24));
        let script_cbor = encode_script_ref(script_ref);
        buf.extend(encode_bytes(&script_cbor));
    }

    buf
}

/// Encode a script reference
fn encode_script_ref(script_ref: &ScriptRef) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match script_ref {
        ScriptRef::NativeScript(ns) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_native_script(ns));
        }
        ScriptRef::PlutusV1(script) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_bytes(script));
        }
        ScriptRef::PlutusV2(script) => {
            buf.extend(encode_uint(2));
            buf.extend(encode_bytes(script));
        }
        ScriptRef::PlutusV3(script) => {
            buf.extend(encode_uint(3));
            buf.extend(encode_bytes(script));
        }
    }
    buf
}

/// Encode a native script
pub fn encode_native_script(script: &NativeScript) -> Vec<u8> {
    match script {
        NativeScript::ScriptPubkey(hash) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(0));
            buf.extend(encode_hash32(hash));
            buf
        }
        NativeScript::ScriptAll(scripts) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(1));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::ScriptAny(scripts) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(2));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::ScriptNOfK(n, scripts) => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(3));
            buf.extend(encode_uint(*n as u64));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::InvalidBefore(slot) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(4));
            buf.extend(encode_uint(slot.0));
            buf
        }
        NativeScript::InvalidHereafter(slot) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(5));
            buf.extend(encode_uint(slot.0));
            buf
        }
    }
}

/// Encode a credential [type, hash]
fn encode_credential(cred: &torsten_primitives::credentials::Credential) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match cred {
        torsten_primitives::credentials::Credential::VerificationKey(h) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_hash28(h));
        }
        torsten_primitives::credentials::Credential::Script(h) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_hash28(h));
        }
    }
    buf
}

/// Encode an anchor [url, data_hash]
fn encode_anchor(anchor: &Anchor) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_text(&anchor.url));
    buf.extend(encode_hash32(&anchor.data_hash));
    buf
}

/// Encode optional anchor
fn encode_optional_anchor(anchor: &Option<Anchor>) -> Vec<u8> {
    match anchor {
        Some(a) => encode_anchor(a),
        None => encode_null(),
    }
}

/// Encode a DRep
fn encode_drep(drep: &DRep) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match drep {
        DRep::KeyHash(h) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_hash32(h));
        }
        DRep::ScriptHash(h) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_hash28(h));
        }
        DRep::Abstain => {
            // [2] - single element
            return vec![0x81, 0x02];
        }
        DRep::NoConfidence => {
            // [3] - single element
            return vec![0x81, 0x03];
        }
    }
    buf
}

/// Encode a Rational as CBOR tag 30 [numerator, denominator]
fn encode_rational(r: &Rational) -> Vec<u8> {
    let mut buf = encode_tag(30);
    buf.extend(encode_array_header(2));
    buf.extend(encode_uint(r.numerator));
    buf.extend(encode_uint(r.denominator));
    buf
}

/// Encode a relay
fn encode_relay(relay: &Relay) -> Vec<u8> {
    match relay {
        Relay::SingleHostAddr { port, ipv4, ipv6 } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(0));
            match port {
                Some(p) => buf.extend(encode_uint(*p as u64)),
                None => buf.extend(encode_null()),
            }
            match ipv4 {
                Some(ip) => buf.extend(encode_bytes(ip)),
                None => buf.extend(encode_null()),
            }
            match ipv6 {
                Some(ip) => buf.extend(encode_bytes(ip)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        Relay::SingleHostName { port, dns_name } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(1));
            match port {
                Some(p) => buf.extend(encode_uint(*p as u64)),
                None => buf.extend(encode_null()),
            }
            buf.extend(encode_text(dns_name));
            buf
        }
        Relay::MultiHostName { dns_name } => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(2));
            buf.extend(encode_text(dns_name));
            buf
        }
    }
}

/// Encode pool parameters
fn encode_pool_params(params: &PoolParams) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend(encode_hash28(&params.operator));
    buf.extend(encode_hash32(&params.vrf_keyhash));
    buf.extend(encode_uint(params.pledge.0));
    buf.extend(encode_uint(params.cost.0));
    buf.extend(encode_rational(&params.margin));
    buf.extend(encode_bytes(&params.reward_account));

    // pool_owners as set
    buf.extend(encode_array_header(params.pool_owners.len()));
    for owner in &params.pool_owners {
        buf.extend(encode_hash28(owner));
    }

    // relays
    buf.extend(encode_array_header(params.relays.len()));
    for relay in &params.relays {
        buf.extend(encode_relay(relay));
    }

    // pool_metadata
    match &params.pool_metadata {
        Some(meta) => {
            buf.extend(encode_array_header(2));
            buf.extend(encode_text(&meta.url));
            buf.extend(encode_hash32(&meta.hash));
        }
        None => buf.extend(encode_null()),
    }

    buf
}

/// Encode a certificate
pub fn encode_certificate(cert: &Certificate) -> Vec<u8> {
    match cert {
        Certificate::StakeRegistration(cred) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(0));
            buf.extend(encode_credential(cred));
            buf
        }
        Certificate::StakeDeregistration(cred) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(1));
            buf.extend(encode_credential(cred));
            buf
        }
        Certificate::StakeDelegation {
            credential,
            pool_hash,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(2));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf
        }
        Certificate::PoolRegistration(params) => {
            let mut buf = encode_array_header(10);
            buf.extend(encode_uint(3));
            buf.extend(encode_pool_params(params));
            buf
        }
        Certificate::PoolRetirement { pool_hash, epoch } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(4));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_uint(*epoch));
            buf
        }
        Certificate::RegDRep {
            credential,
            deposit,
            anchor,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(16));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(deposit.0));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::UnregDRep { credential, refund } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(17));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(refund.0));
            buf
        }
        Certificate::UpdateDRep { credential, anchor } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(18));
            buf.extend(encode_credential(credential));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::VoteDelegation { credential, drep } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(9));
            buf.extend(encode_credential(credential));
            buf.extend(encode_drep(drep));
            buf
        }
        Certificate::StakeVoteDelegation {
            credential,
            pool_hash,
            drep,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(10));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_drep(drep));
            buf
        }
        Certificate::RegStakeDeleg {
            credential,
            pool_hash,
            deposit,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(11));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::CommitteeHotAuth {
            cold_credential,
            hot_credential,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(14));
            buf.extend(encode_credential(cold_credential));
            buf.extend(encode_credential(hot_credential));
            buf
        }
        Certificate::CommitteeColdResign {
            cold_credential,
            anchor,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(15));
            buf.extend(encode_credential(cold_credential));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::RegStakeVoteDeleg {
            credential,
            pool_hash,
            drep,
            deposit,
        } => {
            let mut buf = encode_array_header(5);
            buf.extend(encode_uint(13));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_drep(drep));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::VoteRegDeleg {
            credential,
            drep,
            deposit,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(12));
            buf.extend(encode_credential(credential));
            buf.extend(encode_drep(drep));
            buf.extend(encode_uint(deposit.0));
            buf
        }
    }
}

/// Encode a redeemer tag
fn encode_redeemer_tag(tag: &RedeemerTag) -> Vec<u8> {
    encode_uint(match tag {
        RedeemerTag::Spend => 0,
        RedeemerTag::Mint => 1,
        RedeemerTag::Cert => 2,
        RedeemerTag::Reward => 3,
        RedeemerTag::Vote => 4,
        RedeemerTag::Propose => 5,
    })
}

/// Encode a redeemer [tag, index, data, ex_units]
fn encode_redeemer(redeemer: &Redeemer) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_redeemer_tag(&redeemer.tag));
    buf.extend(encode_uint(redeemer.index as u64));
    buf.extend(encode_plutus_data(&redeemer.data));
    buf.extend(encode_array_header(2));
    buf.extend(encode_uint(redeemer.ex_units.mem));
    buf.extend(encode_uint(redeemer.ex_units.steps));
    buf
}

/// Encode a VKey witness [vkey, signature]
fn encode_vkey_witness(w: &VKeyWitness) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_bytes(&w.vkey));
    buf.extend(encode_bytes(&w.signature));
    buf
}

/// Encode a bootstrap witness [vkey, signature, chain_code, attributes]
fn encode_bootstrap_witness(w: &BootstrapWitness) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_bytes(&w.vkey));
    buf.extend(encode_bytes(&w.signature));
    buf.extend(encode_bytes(&w.chain_code));
    buf.extend(encode_bytes(&w.attributes));
    buf
}

/// Encode a transaction witness set as CBOR map.
///
/// Map keys: 0=vkeywitnesses, 1=native_scripts, 2=bootstrap_witnesses,
///           3=plutus_v1, 4=plutus_data, 5=redeemers, 6=plutus_v2, 7=plutus_v3
pub fn encode_witness_set(ws: &TransactionWitnessSet) -> Vec<u8> {
    let mut count = 0;
    if !ws.vkey_witnesses.is_empty() {
        count += 1;
    }
    if !ws.native_scripts.is_empty() {
        count += 1;
    }
    if !ws.bootstrap_witnesses.is_empty() {
        count += 1;
    }
    if !ws.plutus_v1_scripts.is_empty() {
        count += 1;
    }
    if !ws.plutus_data.is_empty() {
        count += 1;
    }
    if !ws.redeemers.is_empty() {
        count += 1;
    }
    if !ws.plutus_v2_scripts.is_empty() {
        count += 1;
    }
    if !ws.plutus_v3_scripts.is_empty() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    if !ws.vkey_witnesses.is_empty() {
        buf.extend(encode_uint(0));
        buf.extend(encode_array_header(ws.vkey_witnesses.len()));
        for w in &ws.vkey_witnesses {
            buf.extend(encode_vkey_witness(w));
        }
    }

    if !ws.native_scripts.is_empty() {
        buf.extend(encode_uint(1));
        buf.extend(encode_array_header(ws.native_scripts.len()));
        for s in &ws.native_scripts {
            buf.extend(encode_native_script(s));
        }
    }

    if !ws.bootstrap_witnesses.is_empty() {
        buf.extend(encode_uint(2));
        buf.extend(encode_array_header(ws.bootstrap_witnesses.len()));
        for w in &ws.bootstrap_witnesses {
            buf.extend(encode_bootstrap_witness(w));
        }
    }

    if !ws.plutus_v1_scripts.is_empty() {
        buf.extend(encode_uint(3));
        buf.extend(encode_array_header(ws.plutus_v1_scripts.len()));
        for s in &ws.plutus_v1_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    if !ws.plutus_data.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(ws.plutus_data.len()));
        for d in &ws.plutus_data {
            buf.extend(encode_plutus_data(d));
        }
    }

    if !ws.redeemers.is_empty() {
        buf.extend(encode_uint(5));
        buf.extend(encode_array_header(ws.redeemers.len()));
        for r in &ws.redeemers {
            buf.extend(encode_redeemer(r));
        }
    }

    if !ws.plutus_v2_scripts.is_empty() {
        buf.extend(encode_uint(6));
        buf.extend(encode_array_header(ws.plutus_v2_scripts.len()));
        for s in &ws.plutus_v2_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    if !ws.plutus_v3_scripts.is_empty() {
        buf.extend(encode_uint(7));
        buf.extend(encode_array_header(ws.plutus_v3_scripts.len()));
        for s in &ws.plutus_v3_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    buf
}

/// Encode auxiliary data.
///
/// If only metadata and no scripts: metadata map directly.
/// Otherwise: tag 259 with map {0: metadata, 1: native_scripts, 2: plutus_v1, 3: plutus_v2, 4: plutus_v3}
pub fn encode_auxiliary_data(aux: &AuxiliaryData) -> Vec<u8> {
    let has_scripts = !aux.native_scripts.is_empty()
        || !aux.plutus_v1_scripts.is_empty()
        || !aux.plutus_v2_scripts.is_empty()
        || !aux.plutus_v3_scripts.is_empty();

    if !has_scripts {
        // Simple metadata map
        return encode_metadata_map(&aux.metadata);
    }

    // Alonzo+ format: tag 259 { 0: metadata, 1: native_scripts, ... }
    let mut buf = encode_tag(259);
    let mut count = 0;
    if !aux.metadata.is_empty() {
        count += 1;
    }
    if !aux.native_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v1_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v2_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v3_scripts.is_empty() {
        count += 1;
    }

    buf.extend(encode_map_header(count));

    if !aux.metadata.is_empty() {
        buf.extend(encode_uint(0));
        buf.extend(encode_metadata_map(&aux.metadata));
    }
    if !aux.native_scripts.is_empty() {
        buf.extend(encode_uint(1));
        buf.extend(encode_array_header(aux.native_scripts.len()));
        for s in &aux.native_scripts {
            buf.extend(encode_native_script(s));
        }
    }
    if !aux.plutus_v1_scripts.is_empty() {
        buf.extend(encode_uint(2));
        buf.extend(encode_array_header(aux.plutus_v1_scripts.len()));
        for s in &aux.plutus_v1_scripts {
            buf.extend(encode_bytes(s));
        }
    }
    if !aux.plutus_v2_scripts.is_empty() {
        buf.extend(encode_uint(3));
        buf.extend(encode_array_header(aux.plutus_v2_scripts.len()));
        for s in &aux.plutus_v2_scripts {
            buf.extend(encode_bytes(s));
        }
    }
    if !aux.plutus_v3_scripts.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(aux.plutus_v3_scripts.len()));
        for s in &aux.plutus_v3_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    buf
}

/// Encode a metadata map: {label: metadatum}
fn encode_metadata_map(metadata: &BTreeMap<u64, TransactionMetadatum>) -> Vec<u8> {
    let mut buf = encode_map_header(metadata.len());
    for (label, value) in metadata {
        buf.extend(encode_uint(*label));
        buf.extend(encode_metadatum(value));
    }
    buf
}

/// Encode a transaction body as CBOR map.
///
/// Required keys: 0=inputs, 1=outputs, 2=fee
/// Optional keys: 3=ttl, 4=certs, 5=withdrawals, 7=aux_data_hash, 8=validity_start,
///                9=mint, 11=script_data_hash, 13=collateral, 14=required_signers,
///                15=network_id, 16=collateral_return, 17=total_collateral,
///                18=reference_inputs, 19=voting_procedures, 20=proposal_procedures,
///                21=treasury_value, 22=donation
pub fn encode_transaction_body(body: &TransactionBody) -> Vec<u8> {
    // Count fields
    let mut count = 3; // inputs, outputs, fee always present
    if body.ttl.is_some() {
        count += 1;
    }
    if !body.certificates.is_empty() {
        count += 1;
    }
    if !body.withdrawals.is_empty() {
        count += 1;
    }
    if body.auxiliary_data_hash.is_some() {
        count += 1;
    }
    if body.validity_interval_start.is_some() {
        count += 1;
    }
    if !body.mint.is_empty() {
        count += 1;
    }
    if body.script_data_hash.is_some() {
        count += 1;
    }
    if !body.collateral.is_empty() {
        count += 1;
    }
    if !body.required_signers.is_empty() {
        count += 1;
    }
    if body.network_id.is_some() {
        count += 1;
    }
    if body.collateral_return.is_some() {
        count += 1;
    }
    if body.total_collateral.is_some() {
        count += 1;
    }
    if !body.reference_inputs.is_empty() {
        count += 1;
    }
    if !body.voting_procedures.is_empty() {
        count += 1;
    }
    if !body.proposal_procedures.is_empty() {
        count += 1;
    }
    if body.treasury_value.is_some() {
        count += 1;
    }
    if body.donation.is_some() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    // 0: inputs (set of [tx_hash, index])
    buf.extend(encode_uint(0));
    buf.extend(encode_array_header(body.inputs.len()));
    for input in &body.inputs {
        buf.extend(encode_tx_input(input));
    }

    // 1: outputs
    buf.extend(encode_uint(1));
    buf.extend(encode_array_header(body.outputs.len()));
    for output in &body.outputs {
        buf.extend(encode_transaction_output(output));
    }

    // 2: fee
    buf.extend(encode_uint(2));
    buf.extend(encode_uint(body.fee.0));

    // 3: ttl
    if let Some(ttl) = body.ttl {
        buf.extend(encode_uint(3));
        buf.extend(encode_uint(ttl.0));
    }

    // 4: certificates
    if !body.certificates.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(body.certificates.len()));
        for cert in &body.certificates {
            buf.extend(encode_certificate(cert));
        }
    }

    // 5: withdrawals
    if !body.withdrawals.is_empty() {
        buf.extend(encode_uint(5));
        buf.extend(encode_map_header(body.withdrawals.len()));
        for (addr, amount) in &body.withdrawals {
            buf.extend(encode_bytes(addr));
            buf.extend(encode_uint(amount.0));
        }
    }

    // 7: auxiliary_data_hash
    if let Some(hash) = &body.auxiliary_data_hash {
        buf.extend(encode_uint(7));
        buf.extend(encode_hash32(hash));
    }

    // 8: validity_interval_start
    if let Some(start) = body.validity_interval_start {
        buf.extend(encode_uint(8));
        buf.extend(encode_uint(start.0));
    }

    // 9: mint
    if !body.mint.is_empty() {
        buf.extend(encode_uint(9));
        buf.extend(encode_mint(&body.mint));
    }

    // 11: script_data_hash
    if let Some(hash) = &body.script_data_hash {
        buf.extend(encode_uint(11));
        buf.extend(encode_hash32(hash));
    }

    // 13: collateral
    if !body.collateral.is_empty() {
        buf.extend(encode_uint(13));
        buf.extend(encode_array_header(body.collateral.len()));
        for input in &body.collateral {
            buf.extend(encode_tx_input(input));
        }
    }

    // 14: required_signers
    if !body.required_signers.is_empty() {
        buf.extend(encode_uint(14));
        buf.extend(encode_array_header(body.required_signers.len()));
        for hash in &body.required_signers {
            buf.extend(encode_hash32(hash));
        }
    }

    // 15: network_id
    if let Some(nid) = body.network_id {
        buf.extend(encode_uint(15));
        buf.extend(encode_uint(nid as u64));
    }

    // 16: collateral_return
    if let Some(output) = &body.collateral_return {
        buf.extend(encode_uint(16));
        buf.extend(encode_transaction_output(output));
    }

    // 17: total_collateral
    if let Some(total) = body.total_collateral {
        buf.extend(encode_uint(17));
        buf.extend(encode_uint(total.0));
    }

    // 18: reference_inputs
    if !body.reference_inputs.is_empty() {
        buf.extend(encode_uint(18));
        buf.extend(encode_array_header(body.reference_inputs.len()));
        for input in &body.reference_inputs {
            buf.extend(encode_tx_input(input));
        }
    }

    // 19: voting_procedures
    if !body.voting_procedures.is_empty() {
        buf.extend(encode_uint(19));
        buf.extend(encode_voting_procedures(&body.voting_procedures));
    }

    // 20: proposal_procedures
    if !body.proposal_procedures.is_empty() {
        buf.extend(encode_uint(20));
        buf.extend(encode_array_header(body.proposal_procedures.len()));
        for pp in &body.proposal_procedures {
            buf.extend(encode_proposal_procedure(pp));
        }
    }

    // 21: treasury_value
    if let Some(treasury) = body.treasury_value {
        buf.extend(encode_uint(21));
        buf.extend(encode_uint(treasury.0));
    }

    // 22: donation
    if let Some(donation) = body.donation {
        buf.extend(encode_uint(22));
        buf.extend(encode_uint(donation.0));
    }

    buf
}

/// Encode voting procedures map
fn encode_voting_procedures(
    procedures: &BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>>,
) -> Vec<u8> {
    let mut buf = encode_map_header(procedures.len());
    for (voter, actions) in procedures {
        buf.extend(encode_voter(voter));
        buf.extend(encode_map_header(actions.len()));
        for (action_id, procedure) in actions {
            buf.extend(encode_gov_action_id(action_id));
            buf.extend(encode_voting_procedure(procedure));
        }
    }
    buf
}

fn encode_voter(voter: &Voter) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match voter {
        Voter::ConstitutionalCommittee(cred) => {
            match cred {
                torsten_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(0));
                }
                torsten_primitives::credentials::Credential::Script(_) => {
                    buf.extend(encode_uint(1));
                }
            }
            buf.extend(encode_hash28(cred.to_hash()));
        }
        Voter::DRep(cred) => {
            match cred {
                torsten_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(2));
                }
                torsten_primitives::credentials::Credential::Script(_) => {
                    buf.extend(encode_uint(3));
                }
            }
            buf.extend(encode_hash28(cred.to_hash()));
        }
        Voter::StakePool(hash) => {
            buf.extend(encode_uint(4));
            buf.extend(encode_hash32(hash));
        }
    }
    buf
}

fn encode_gov_action_id(id: &GovActionId) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_hash32(&id.transaction_id));
    buf.extend(encode_uint(id.action_index as u64));
    buf
}

fn encode_voting_procedure(proc: &VotingProcedure) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(match proc.vote {
        Vote::No => 0,
        Vote::Yes => 1,
        Vote::Abstain => 2,
    }));
    buf.extend(encode_optional_anchor(&proc.anchor));
    buf
}

fn encode_proposal_procedure(pp: &ProposalProcedure) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_uint(pp.deposit.0));
    buf.extend(encode_bytes(&pp.return_addr));
    buf.extend(encode_gov_action(&pp.gov_action));
    buf.extend(encode_anchor(&pp.anchor));
    buf
}

fn encode_gov_action(action: &GovAction) -> Vec<u8> {
    match action {
        GovAction::ParameterChange {
            prev_action_id,
            protocol_param_update: _,
            policy_hash,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(0));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            // Protocol param update encoding is complex; use empty map as placeholder
            buf.extend(encode_map_header(0));
            match policy_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::HardForkInitiation {
            prev_action_id,
            protocol_version,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(1));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(protocol_version.0));
            buf.extend(encode_uint(protocol_version.1));
            buf
        }
        GovAction::TreasuryWithdrawals {
            withdrawals,
            policy_hash,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(2));
            buf.extend(encode_map_header(withdrawals.len()));
            for (addr, amount) in withdrawals {
                buf.extend(encode_bytes(addr));
                buf.extend(encode_uint(amount.0));
            }
            match policy_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::NoConfidence { prev_action_id } => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(3));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf
        }
        GovAction::UpdateCommittee {
            prev_action_id,
            members_to_remove,
            members_to_add,
            threshold,
        } => {
            let mut buf = encode_array_header(5);
            buf.extend(encode_uint(4));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(members_to_remove.len()));
            for cred in members_to_remove {
                buf.extend(encode_credential(cred));
            }
            buf.extend(encode_map_header(members_to_add.len()));
            for (cred, epoch) in members_to_add {
                buf.extend(encode_credential(cred));
                buf.extend(encode_uint(*epoch));
            }
            buf.extend(encode_rational(threshold));
            buf
        }
        GovAction::NewConstitution {
            prev_action_id,
            constitution,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(5));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(2));
            buf.extend(encode_anchor(&constitution.anchor));
            match &constitution.script_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::InfoAction => {
            let mut buf = encode_array_header(1);
            buf.extend(encode_uint(6));
            buf
        }
    }
}

fn encode_optional_gov_action_id(id: &Option<GovActionId>) -> Vec<u8> {
    match id {
        Some(id) => encode_gov_action_id(id),
        None => encode_null(),
    }
}

/// Encode a complete transaction: [body, witness_set, is_valid, auxiliary_data]
pub fn encode_transaction(tx: &Transaction) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_transaction_body(&tx.body));
    buf.extend(encode_witness_set(&tx.witness_set));
    buf.extend(encode_bool(tx.is_valid));
    match &tx.auxiliary_data {
        Some(aux) => buf.extend(encode_auxiliary_data(aux)),
        None => buf.extend(encode_null()),
    }
    buf
}

/// Compute the transaction hash from the body encoding (blake2b-256 of CBOR body)
pub fn compute_transaction_hash(body: &TransactionBody) -> Hash32 {
    let body_cbor = encode_transaction_body(body);
    blake2b_256(&body_cbor)
}

/// Encode an operational certificate: [hot_vkey, sequence_number, kes_period, sigma]
pub fn encode_operational_cert(cert: &OperationalCert) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_bytes(&cert.hot_vkey));
    buf.extend(encode_uint(cert.sequence_number));
    buf.extend(encode_uint(cert.kes_period));
    buf.extend(encode_bytes(&cert.sigma));
    buf
}

/// Encode a VRF result: [output, proof]
pub fn encode_vrf_result(vrf: &VrfOutput) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_bytes(&vrf.output));
    buf.extend(encode_bytes(&vrf.proof));
    buf
}

/// Encode a protocol version: [major, minor]
pub fn encode_protocol_version(pv: &ProtocolVersion) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(pv.major));
    buf.extend(encode_uint(pv.minor));
    buf
}

/// Encode a block header body (the part that gets signed by KES).
///
/// [block_number, slot, prev_hash, issuer_vkey, vrf_vkey, vrf_result,
///  body_size, body_hash, operational_cert, protocol_version]
pub fn encode_block_header_body(header: &BlockHeader) -> Vec<u8> {
    let mut buf = encode_array_header(10);
    buf.extend(encode_uint(header.block_number.0));
    buf.extend(encode_uint(header.slot.0));
    buf.extend(encode_hash32(&header.prev_hash));
    buf.extend(encode_bytes(&header.issuer_vkey));
    buf.extend(encode_bytes(&header.vrf_vkey));
    buf.extend(encode_vrf_result(&header.vrf_result));
    buf.extend(encode_uint(header.body_size));
    buf.extend(encode_hash32(&header.body_hash));
    buf.extend(encode_operational_cert(&header.operational_cert));
    buf.extend(encode_protocol_version(&header.protocol_version));
    buf
}

/// Encode a complete block header: [header_body, body_signature]
///
/// The `kes_signature` parameter is the KES signature over the header body.
pub fn encode_block_header(header: &BlockHeader, kes_signature: &[u8]) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_block_header_body(header));
    buf.extend(encode_bytes(kes_signature));
    buf
}

/// Encode a complete Babbage/Conway era block.
///
/// Block = [era_tag, [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]]
///
/// For Babbage (era 6) and Conway (era 7), blocks are wrapped with era tag.
/// The `kes_signature` is the KES signature for the block header.
pub fn encode_block(block: &Block, kes_signature: &[u8]) -> Vec<u8> {
    let era_tag = match block.era {
        torsten_primitives::era::Era::Byron => 0u64,
        torsten_primitives::era::Era::Shelley => 2,
        torsten_primitives::era::Era::Allegra => 3,
        torsten_primitives::era::Era::Mary => 4,
        torsten_primitives::era::Era::Alonzo => 5,
        torsten_primitives::era::Era::Babbage => 6,
        torsten_primitives::era::Era::Conway => 7,
    };

    // Outer array: [era_tag, block_content]
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(era_tag));

    // Block content: [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]
    buf.extend(encode_array_header(5));

    // Header
    buf.extend(encode_block_header(&block.header, kes_signature));

    // Transaction bodies
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        buf.extend(encode_transaction_body(&tx.body));
    }

    // Transaction witness sets
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        buf.extend(encode_witness_set(&tx.witness_set));
    }

    // Auxiliary data map: {tx_index: aux_data}
    let aux_entries: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter_map(|(i, tx)| tx.auxiliary_data.as_ref().map(|aux| (i, aux)))
        .collect();
    buf.extend(encode_map_header(aux_entries.len()));
    for (idx, aux) in &aux_entries {
        buf.extend(encode_uint(*idx as u64));
        buf.extend(encode_auxiliary_data(aux));
    }

    // Invalid transactions (indices of txs with is_valid=false)
    let invalid_indices: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| !tx.is_valid)
        .map(|(i, _)| i)
        .collect();
    buf.extend(encode_array_header(invalid_indices.len()));
    for idx in &invalid_indices {
        buf.extend(encode_uint(*idx as u64));
    }

    buf
}

/// Compute the block body hash from the transaction bodies.
///
/// This is blake2b-256 of the concatenated CBOR-encoded transaction bodies array.
pub fn compute_block_body_hash(transactions: &[Transaction]) -> Hash32 {
    let mut body = encode_array_header(transactions.len());
    for tx in transactions {
        body.extend(encode_transaction_body(&tx.body));
    }
    blake2b_256(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::address::{Address, EnterpriseAddress};
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::era::Era;
    use torsten_primitives::hash::{Hash28, Hash32};
    use torsten_primitives::time::{BlockNo, SlotNo};
    use torsten_primitives::value::{Lovelace, Value};

    #[test]
    fn test_encode_value_pure_ada() {
        let v = Value::lovelace(2_000_000);
        let encoded = encode_value(&v);
        // Should be just the uint encoding of 2000000
        assert_eq!(encoded, encode_uint(2_000_000));
    }

    #[test]
    fn test_encode_value_multi_asset() {
        let policy = Hash28::from_bytes([1u8; 28]);
        let asset_name = AssetName(b"Token".to_vec());
        let mut v = Value::lovelace(5_000_000);
        v.multi_asset
            .entry(policy)
            .or_default()
            .insert(asset_name, 100);

        let encoded = encode_value(&v);
        // Should be [coin, {policy: {name: qty}}]
        assert_eq!(encoded[0], 0x82); // array of 2
    }

    #[test]
    fn test_encode_transaction_output_simple() {
        let output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: torsten_primitives::network::NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            }),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        assert_eq!(encoded[0], 0xa2); // map of 2 (address + value)
    }

    #[test]
    fn test_encode_transaction_output_with_datum_hash() {
        let output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: torsten_primitives::network::NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            }),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::DatumHash(Hash32::ZERO),
            script_ref: None,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        assert_eq!(encoded[0], 0xa3); // map of 3 (address + value + datum)
    }

    #[test]
    fn test_encode_native_script_pubkey() {
        let script = NativeScript::ScriptPubkey(Hash32::ZERO);
        let encoded = encode_native_script(&script);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x00); // type 0
    }

    #[test]
    fn test_encode_native_script_all() {
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(Hash32::ZERO),
            NativeScript::ScriptPubkey(Hash32::ZERO),
        ]);
        let encoded = encode_native_script(&script);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x01); // type 1 (all)
    }

    #[test]
    fn test_encode_certificate_stake_reg() {
        let cert = Certificate::StakeRegistration(Credential::VerificationKey(Hash28::from_bytes(
            [0u8; 28],
        )));
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x00); // type 0
    }

    #[test]
    fn test_encode_certificate_pool_retirement() {
        let cert = Certificate::PoolRetirement {
            pool_hash: Hash28::from_bytes([1u8; 28]),
            epoch: 300,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83); // array of 3
        assert_eq!(encoded[1], 0x04); // type 4
    }

    #[test]
    fn test_encode_witness_set_empty() {
        let ws = TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
        };
        let encoded = encode_witness_set(&ws);
        assert_eq!(encoded, vec![0xa0]); // empty map
    }

    #[test]
    fn test_encode_witness_set_with_vkeys() {
        let ws = TransactionWitnessSet {
            vkey_witnesses: vec![VKeyWitness {
                vkey: vec![0u8; 32],
                signature: vec![0u8; 64],
            }],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
        };
        let encoded = encode_witness_set(&ws);
        assert_eq!(encoded[0], 0xa1); // map of 1
    }

    #[test]
    fn test_encode_transaction_body_minimal() {
        let body = TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            }],
            outputs: vec![TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Mainnet,
                    payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            }],
            fee: Lovelace(200_000),
            ttl: None,
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        };

        let encoded = encode_transaction_body(&body);
        assert_eq!(encoded[0], 0xa3); // map of 3 (inputs, outputs, fee)
    }

    #[test]
    fn test_encode_transaction_roundtrip_hash() {
        let body = TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            }],
            outputs: vec![TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Mainnet,
                    payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            }],
            fee: Lovelace(200_000),
            ttl: Some(SlotNo(1000)),
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        };

        // Hash should be deterministic
        let hash1 = compute_transaction_hash(&body);
        let hash2 = compute_transaction_hash(&body);
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, Hash32::ZERO);
    }

    #[test]
    fn test_encode_transaction_complete() {
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::ZERO,
                    index: 0,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Enterprise(EnterpriseAddress {
                        network: torsten_primitives::network::NetworkId::Mainnet,
                        payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let encoded = encode_transaction(&tx);
        assert_eq!(encoded[0], 0x84); // array of 4
    }

    #[test]
    fn test_encode_block_header_body() {
        let header = BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash: Hash32::from_bytes([1u8; 32]),
            issuer_vkey: vec![0u8; 32],
            vrf_vkey: vec![0u8; 32],
            vrf_result: VrfOutput {
                output: vec![0u8; 64],
                proof: vec![0u8; 80],
            },
            block_number: BlockNo(100),
            slot: SlotNo(500),
            epoch_nonce: Hash32::ZERO,
            body_size: 1024,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![0u8; 32],
                sequence_number: 1,
                kes_period: 200,
                sigma: vec![0u8; 64],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
        };

        let encoded = encode_block_header_body(&header);
        assert_eq!(encoded[0], 0x8a); // array of 10
    }

    #[test]
    fn test_encode_block_complete() {
        let block = Block {
            header: BlockHeader {
                header_hash: Hash32::ZERO,
                prev_hash: Hash32::from_bytes([1u8; 32]),
                issuer_vkey: vec![0u8; 32],
                vrf_vkey: vec![0u8; 32],
                vrf_result: VrfOutput {
                    output: vec![0u8; 64],
                    proof: vec![0u8; 80],
                },
                block_number: BlockNo(100),
                slot: SlotNo(500),
                epoch_nonce: Hash32::ZERO,
                body_size: 0,
                body_hash: Hash32::ZERO,
                operational_cert: OperationalCert {
                    hot_vkey: vec![0u8; 32],
                    sequence_number: 1,
                    kes_period: 200,
                    sigma: vec![0u8; 64],
                },
                protocol_version: ProtocolVersion { major: 9, minor: 0 },
            },
            transactions: vec![],
            era: Era::Conway,
            raw_cbor: None,
        };

        let kes_sig = vec![0u8; 448]; // KES signature placeholder
        let encoded = encode_block(&block, &kes_sig);
        assert_eq!(encoded[0], 0x82); // outer array of 2 [era_tag, block]
        assert_eq!(encoded[1], 0x07); // era 7 (Conway)
    }

    #[test]
    fn test_encode_auxiliary_data_simple() {
        let mut metadata = BTreeMap::new();
        metadata.insert(1u64, TransactionMetadatum::Text("hello".to_string()));

        let aux = AuxiliaryData {
            metadata,
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        };

        let encoded = encode_auxiliary_data(&aux);
        assert_eq!(encoded[0], 0xa1); // map of 1
    }

    #[test]
    fn test_compute_block_body_hash() {
        let hash = compute_block_body_hash(&[]);
        // Hash of empty array (CBOR: 0x80)
        assert_ne!(hash, Hash32::ZERO);
    }

    #[test]
    fn test_encode_redeemer() {
        let r = Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: 100000,
                steps: 200000,
            },
        };
        let encoded = encode_redeemer(&r);
        assert_eq!(encoded[0], 0x84); // array of 4
    }

    #[test]
    fn test_encode_drep_variants() {
        let abstain = encode_drep(&DRep::Abstain);
        assert_eq!(abstain, vec![0x81, 0x02]); // [2]

        let no_conf = encode_drep(&DRep::NoConfidence);
        assert_eq!(no_conf, vec![0x81, 0x03]); // [3]

        let key = encode_drep(&DRep::KeyHash(Hash32::ZERO));
        assert_eq!(key[0], 0x82); // [0, hash]
    }

    #[test]
    fn test_encode_certificate_conway_drep() {
        let cert = Certificate::RegDRep {
            credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
            deposit: Lovelace(500_000_000),
            anchor: Some(Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            }),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84); // array of 4
    }
}
