use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::multiplexer::Segment;
use crate::query_handler::{ProtocolParamsSnapshot, QueryHandler, QueryResult};

use super::{N2CServerError, MINI_PROTOCOL_STATE_QUERY};

/// Handle LocalStateQuery messages
///
/// Protocol flow:
///   Client: MsgAcquire(point) → Server: MsgAcquired
///   Client: MsgQuery(query)   → Server: MsgResult(result)
///   Client: MsgRelease        → (back to idle)
///   Client: MsgDone           → (end)
pub(crate) async fn handle_state_query(
    payload: &[u8],
    query_handler: &Arc<RwLock<QueryHandler>>,
    _negotiated_version: u16,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    // Parse the CBOR message tag
    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad msg tag: {e}")))?,
        Ok(None) => {
            // Indefinite length array
            decoder
                .u32()
                .map_err(|e| N2CServerError::Protocol(format!("bad msg tag: {e}")))?
        }
        _ => {
            return Err(N2CServerError::Protocol(
                "invalid state query message".into(),
            ))
        }
    };

    match msg_tag {
        0 | 8 => {
            // MsgAcquire(point) [0] or MsgAcquireNoPoint [8]
            // Tag 8 acquires at current tip without specifying a point.
            // Used by newer cardano-cli versions.
            debug!("LocalStateQuery: MsgAcquire (tag {msg_tag})");
            // Respond with MsgAcquired [1]
            let mut resp = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut resp);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcquired
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: resp,
            }))
        }
        3 => {
            // MsgQuery(query)
            debug!(
                query_hex = %format!("{:02x?}", &payload[..payload.len().min(32)]),
                "LocalStateQuery: MsgQuery"
            );
            let handler = query_handler.read().await;
            let result = handler.handle_query_cbor(payload);
            let response_cbor = encode_query_result(&result);
            debug!(
                response_hex = %format!("{:02x?}", &response_cbor[..response_cbor.len().min(32)]),
                response_len = response_cbor.len(),
                "LocalStateQuery: MsgResult"
            );

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: response_cbor,
            }))
        }
        5 | 10 => {
            // MsgReAcquire(point) [5] or MsgReAcquireNoPoint [10]
            debug!("LocalStateQuery: MsgReAcquire (tag {msg_tag})");
            // Respond with MsgAcquired [1]
            let mut resp = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut resp);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: resp,
            }))
        }
        7 => {
            // MsgRelease
            debug!("LocalStateQuery: MsgRelease");
            Ok(None)
        }
        9 => {
            // MsgDone
            debug!("LocalStateQuery: MsgDone");
            Ok(None)
        }
        other => {
            warn!("Unknown LocalStateQuery message tag: {other}");
            Ok(None)
        }
    }
}

/// Encode a UTxO output in PostAlonzo format (CBOR map with integer keys).
///
/// Format: {0: address_bytes, 1: value, 2?: datum_option, 3?: script_ref}
/// Value: coin (integer) or [coin, {policy_id -> {asset_name -> quantity}}]
fn encode_utxo_output(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    utxo: &crate::query_handler::UtxoSnapshot,
) {
    let has_datum = utxo.datum_hash.is_some();
    let field_count = 2 + has_datum as u64; // address + value + optional datum
    enc.map(field_count).ok();

    // 0: address (raw bytes)
    enc.u32(0).ok();
    enc.bytes(&utxo.address_bytes).ok();

    // 1: value
    enc.u32(1).ok();
    if utxo.multi_asset.is_empty() {
        // Coin-only: encode as plain integer
        enc.u64(utxo.lovelace).ok();
    } else {
        // Multi-asset: [coin, {policy_id -> {asset_name -> quantity}}]
        enc.array(2).ok();
        enc.u64(utxo.lovelace).ok();
        enc.map(utxo.multi_asset.len() as u64).ok();
        for (policy_id, assets) in &utxo.multi_asset {
            enc.bytes(policy_id).ok();
            enc.map(assets.len() as u64).ok();
            for (asset_name, quantity) in assets {
                enc.bytes(asset_name).ok();
                enc.u64(*quantity).ok();
            }
        }
    }

    // 2: datum_option (if present)
    if let Some(ref datum_hash) = utxo.datum_hash {
        enc.u32(2).ok();
        // DatumOption::Hash variant: [0, datum_hash]
        enc.array(2).ok();
        enc.u32(0).ok();
        enc.bytes(datum_hash).ok();
    }
}

/// Encode protocol parameters as a positional CBOR array(31) per Haskell ConwayPParams.
///
/// The Haskell reference uses `encCBOR` which encodes PParams as a flat positional array,
/// NOT a map. Field order matches `eraPParams @ConwayEra`:
///   [0] txFeePerByte, [1] txFeeFixed, [2] maxBBSize, [3] maxTxSize,
///   [4] maxBHSize, [5] keyDeposit, [6] poolDeposit, [7] eMax, [8] nOpt,
///   [9] a0, [10] rho, [11] tau, [12] protocolVersion,
///   [13] minPoolCost, [14] coinsPerUTxOByte, [15] costModels,
///   [16] prices, [17] maxTxExUnits, [18] maxBlockExUnits,
///   [19] maxValSize, [20] collateralPercentage, [21] maxCollateralInputs,
///   [22] poolVotingThresholds(5), [23] drepVotingThresholds(10),
///   [24] committeeMinSize, [25] committeeMaxTermLength, [26] govActionLifetime,
///   [27] govActionDeposit, [28] drepDeposit, [29] drepActivity,
///   [30] minFeeRefScriptCostPerByte
fn encode_protocol_params_cbor(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ProtocolParamsSnapshot,
) {
    enc.array(31).ok();

    // [0] txFeePerByte (min_fee_a)
    enc.u64(pp.min_fee_a).ok();
    // [1] txFeeFixed (min_fee_b)
    enc.u64(pp.min_fee_b).ok();
    // [2] maxBlockBodySize
    enc.u64(pp.max_block_body_size).ok();
    // [3] maxTxSize
    enc.u64(pp.max_tx_size).ok();
    // [4] maxBlockHeaderSize
    enc.u64(pp.max_block_header_size).ok();
    // [5] keyDeposit
    enc.u64(pp.key_deposit).ok();
    // [6] poolDeposit
    enc.u64(pp.pool_deposit).ok();
    // [7] eMax
    enc.u64(pp.e_max).ok();
    // [8] nOpt
    enc.u64(pp.n_opt).ok();

    // [9] a0 (rational as tag 30)
    encode_tagged_rational(enc, pp.a0_num, pp.a0_den);
    // [10] rho
    encode_tagged_rational(enc, pp.rho_num, pp.rho_den);
    // [11] tau
    encode_tagged_rational(enc, pp.tau_num, pp.tau_den);

    // [12] protocolVersion [major, minor]
    enc.array(2).ok();
    enc.u64(pp.protocol_version_major).ok();
    enc.u64(pp.protocol_version_minor).ok();

    // [13] minPoolCost
    enc.u64(pp.min_pool_cost).ok();
    // [14] coinsPerUTxOByte
    enc.u64(pp.ada_per_utxo_byte).ok();

    // [15] costModels (map: {0: [v1], 1: [v2], 2: [v3]})
    {
        let cm_count = pp.cost_models_v1.is_some() as u64
            + pp.cost_models_v2.is_some() as u64
            + pp.cost_models_v3.is_some() as u64;
        enc.map(cm_count).ok();
        if let Some(ref v1) = pp.cost_models_v1 {
            enc.u32(0).ok();
            enc.array(v1.len() as u64).ok();
            for cost in v1 {
                enc.i64(*cost).ok();
            }
        }
        if let Some(ref v2) = pp.cost_models_v2 {
            enc.u32(1).ok();
            enc.array(v2.len() as u64).ok();
            for cost in v2 {
                enc.i64(*cost).ok();
            }
        }
        if let Some(ref v3) = pp.cost_models_v3 {
            enc.u32(2).ok();
            enc.array(v3.len() as u64).ok();
            for cost in v3 {
                enc.i64(*cost).ok();
            }
        }
    }

    // [16] prices [mem_price, step_price] as tagged rationals
    enc.array(2).ok();
    encode_tagged_rational(enc, pp.execution_costs_mem_num, pp.execution_costs_mem_den);
    encode_tagged_rational(
        enc,
        pp.execution_costs_step_num,
        pp.execution_costs_step_den,
    );

    // [17] maxTxExUnits [mem, steps]
    enc.array(2).ok();
    enc.u64(pp.max_tx_ex_mem).ok();
    enc.u64(pp.max_tx_ex_steps).ok();

    // [18] maxBlockExUnits [mem, steps]
    enc.array(2).ok();
    enc.u64(pp.max_block_ex_mem).ok();
    enc.u64(pp.max_block_ex_steps).ok();

    // [19] maxValSize
    enc.u64(pp.max_val_size).ok();
    // [20] collateralPercentage
    enc.u64(pp.collateral_percentage).ok();
    // [21] maxCollateralInputs
    enc.u64(pp.max_collateral_inputs).ok();

    // [22] poolVotingThresholds (5 tagged rationals)
    enc.array(5).ok();
    encode_tagged_rational(
        enc,
        pp.pvt_motion_no_confidence_num,
        pp.pvt_motion_no_confidence_den,
    );
    encode_tagged_rational(
        enc,
        pp.pvt_committee_normal_num,
        pp.pvt_committee_normal_den,
    );
    encode_tagged_rational(
        enc,
        pp.pvt_committee_no_confidence_num,
        pp.pvt_committee_no_confidence_den,
    );
    encode_tagged_rational(enc, pp.pvt_hard_fork_num, pp.pvt_hard_fork_den);
    encode_tagged_rational(
        enc,
        pp.pvt_pp_security_group_num,
        pp.pvt_pp_security_group_den,
    );

    // [23] drepVotingThresholds (10 tagged rationals)
    enc.array(10).ok();
    encode_tagged_rational(enc, pp.dvt_no_confidence_num, pp.dvt_no_confidence_den);
    encode_tagged_rational(
        enc,
        pp.dvt_committee_normal_num,
        pp.dvt_committee_normal_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_committee_no_confidence_num,
        pp.dvt_committee_no_confidence_den,
    );
    encode_tagged_rational(enc, pp.dvt_constitution_num, pp.dvt_constitution_den);
    encode_tagged_rational(enc, pp.dvt_hard_fork_num, pp.dvt_hard_fork_den);
    encode_tagged_rational(
        enc,
        pp.dvt_pp_network_group_num,
        pp.dvt_pp_network_group_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_pp_economic_group_num,
        pp.dvt_pp_economic_group_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_pp_technical_group_num,
        pp.dvt_pp_technical_group_den,
    );
    encode_tagged_rational(enc, pp.dvt_pp_gov_group_num, pp.dvt_pp_gov_group_den);
    encode_tagged_rational(
        enc,
        pp.dvt_treasury_withdrawal_num,
        pp.dvt_treasury_withdrawal_den,
    );

    // [24] committeeMinSize
    enc.u64(pp.committee_min_size).ok();
    // [25] committeeMaxTermLength
    enc.u64(pp.committee_max_term_length).ok();
    // [26] govActionLifetime
    enc.u64(pp.gov_action_lifetime).ok();
    // [27] govActionDeposit
    enc.u64(pp.gov_action_deposit).ok();
    // [28] drepDeposit
    enc.u64(pp.drep_deposit).ok();
    // [29] drepActivity
    enc.u64(pp.drep_activity).ok();

    // [30] minFeeRefScriptCostPerByte
    encode_tagged_rational(enc, pp.min_fee_ref_script_cost_per_byte, 1);
}

/// Helper to encode a tagged rational number: tag(30)[numerator, denominator]
fn encode_tagged_rational(enc: &mut minicbor::Encoder<&mut Vec<u8>>, num: u64, den: u64) {
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(num).ok();
    enc.u64(den).ok();
}

/// Encode a LedgerRelayAccessPoint for LedgerPeerSnapshot.
///
/// Haskell wire format:
///   DNS domain:   array(3) [0, port_integer, domain_bytestring]
///   IPv4 address: array(3) [1, port_integer, array(4)[o1, o2, o3, o4]]
///   IPv6 address: array(3) [2, port_integer, array(4)[w1, w2, w3, w4]]
fn encode_ledger_relay(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    relay: &crate::query_handler::RelaySnapshot,
) {
    match relay {
        crate::query_handler::RelaySnapshot::SingleHostAddr { port, ipv4, ipv6 } => {
            let p = port.unwrap_or(3001) as i64;
            if let Some(ip4) = ipv4 {
                // IPv4: [1, port, [o1, o2, o3, o4]]
                enc.array(3).ok();
                enc.u32(1).ok();
                enc.i64(p).ok();
                enc.array(4).ok();
                for octet in ip4 {
                    enc.i64(*octet as i64).ok();
                }
            } else if let Some(ip6) = ipv6 {
                // IPv6: [2, port, [w1, w2, w3, w4]] as 4 x 32-bit words
                enc.array(3).ok();
                enc.u32(2).ok();
                enc.i64(p).ok();
                enc.array(4).ok();
                for chunk in ip6.chunks(4) {
                    let w = u32::from_be_bytes([
                        chunk.first().copied().unwrap_or(0),
                        chunk.get(1).copied().unwrap_or(0),
                        chunk.get(2).copied().unwrap_or(0),
                        chunk.get(3).copied().unwrap_or(0),
                    ]);
                    enc.i64(w as i64).ok();
                }
            } else {
                // No IP — encode as IPv4 0.0.0.0
                enc.array(3).ok();
                enc.u32(1).ok();
                enc.i64(p).ok();
                enc.array(4).ok();
                for _ in 0..4 {
                    enc.i64(0).ok();
                }
            }
        }
        crate::query_handler::RelaySnapshot::SingleHostName { port, dns_name } => {
            // DNS: [0, port, domain_bytes]
            enc.array(3).ok();
            enc.u32(0).ok();
            enc.i64(port.unwrap_or(3001) as i64).ok();
            enc.bytes(dns_name.as_bytes()).ok();
        }
        crate::query_handler::RelaySnapshot::MultiHostName { dns_name } => {
            // DNS: [0, port=3001, domain_bytes]
            enc.array(3).ok();
            enc.u32(0).ok();
            enc.i64(3001).ok();
            enc.bytes(dns_name.as_bytes()).ok();
        }
    }
}

/// Encode a single GovActionState as CBOR array(7).
fn encode_gov_action_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    p: &crate::query_handler::ProposalSnapshot,
) {
    // GovActionState = array(7)
    //   [0] gasId, [1] committeeVotes, [2] drepVotes,
    //   [3] spoVotes, [4] procedure, [5] proposedIn, [6] expiresAfter
    enc.array(7).ok();
    // [0] GovActionId = array(2) [tx_hash, action_index]
    enc.array(2).ok();
    enc.bytes(&p.tx_id).ok();
    enc.u32(p.action_index).ok();
    // [1] committeeVotes = Map<Credential, Vote>
    // Credential = [cred_type, hash(28)], Vote = uint (0=No, 1=Yes, 2=Abstain)
    enc.map(p.committee_votes.len() as u64).ok();
    for (hash, cred_type, vote) in &p.committee_votes {
        enc.array(2).ok();
        enc.u8(*cred_type).ok();
        enc.bytes(hash).ok();
        enc.u8(*vote).ok();
    }
    // [2] drepVotes = Map<Credential, Vote>
    enc.map(p.drep_votes.len() as u64).ok();
    for (hash, cred_type, vote) in &p.drep_votes {
        enc.array(2).ok();
        enc.u8(*cred_type).ok();
        enc.bytes(hash).ok();
        enc.u8(*vote).ok();
    }
    // [3] spoVotes = Map<KeyHash, Vote>
    // SPO uses bare KeyHash (28 bytes), not wrapped Credential
    enc.map(p.spo_votes.len() as u64).ok();
    for (pool_hash, vote) in &p.spo_votes {
        enc.bytes(pool_hash).ok();
        enc.u8(*vote).ok();
    }
    // [4] ProposalProcedure = array(4) [deposit, return_addr, gov_action, anchor]
    enc.array(4).ok();
    enc.u64(p.deposit).ok();
    enc.bytes(&p.return_addr).ok();
    // gov_action = sum type tagged by action type
    encode_gov_action_tag(enc, &p.action_type);
    // anchor = array(2) [url, hash]
    enc.array(2).ok();
    enc.str(&p.anchor_url).ok();
    enc.bytes(&p.anchor_hash).ok();
    // [5] proposedIn (EpochNo)
    enc.u64(p.proposed_epoch).ok();
    // [6] expiresAfter (EpochNo)
    enc.u64(p.expires_epoch).ok();
}

/// Encode a GovAction as a CBOR sum type tag.
/// We encode a simplified version since we only have the action type string.
fn encode_gov_action_tag(enc: &mut minicbor::Encoder<&mut Vec<u8>>, action_type: &str) {
    match action_type {
        "ParameterChange" => {
            // [0, prev_action_id, params, policy_hash]
            enc.array(4).ok();
            enc.u32(0).ok();
            enc.null().ok(); // prev action id
            enc.map(0).ok(); // empty params update
            enc.null().ok(); // policy hash
        }
        "HardForkInitiation" => {
            // [1, prev_action_id, protocol_version]
            enc.array(3).ok();
            enc.u32(1).ok();
            enc.null().ok();
            enc.array(2).ok();
            enc.u64(0).ok();
            enc.u64(0).ok();
        }
        "TreasuryWithdrawals" => {
            // [2, withdrawals_map, policy_hash]
            enc.array(3).ok();
            enc.u32(2).ok();
            enc.map(0).ok();
            enc.null().ok();
        }
        "NoConfidence" => {
            // [3, prev_action_id]
            enc.array(2).ok();
            enc.u32(3).ok();
            enc.null().ok();
        }
        "UpdateCommittee" => {
            // [4, prev_action_id, remove_set, add_map, quorum]
            enc.array(5).ok();
            enc.u32(4).ok();
            enc.null().ok();
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            enc.map(0).ok();
            encode_tagged_rational(enc, 2, 3);
        }
        "NewConstitution" => {
            // [5, prev_action_id, constitution]
            enc.array(3).ok();
            enc.u32(5).ok();
            enc.null().ok();
            enc.array(2).ok();
            enc.array(2).ok();
            enc.str("").ok();
            enc.bytes(&[0u8; 32]).ok();
            enc.null().ok();
        }
        _ => {
            // [6]
            enc.array(1).ok();
            enc.u32(6).ok();
        }
    }
}

fn encode_relay_cbor(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    relay: &crate::query_handler::RelaySnapshot,
) {
    use crate::query_handler::RelaySnapshot;
    match relay {
        RelaySnapshot::SingleHostAddr { port, ipv4, ipv6 } => {
            enc.array(4).ok();
            enc.u32(0).ok();
            match port {
                Some(p) => {
                    enc.u16(*p).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            match ipv4 {
                Some(ip) => {
                    enc.bytes(ip).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            match ipv6 {
                Some(ip) => {
                    enc.bytes(ip).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
        }
        RelaySnapshot::SingleHostName { port, dns_name } => {
            enc.array(3).ok();
            enc.u32(1).ok();
            match port {
                Some(p) => {
                    enc.u16(*p).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            enc.str(dns_name).ok();
        }
        RelaySnapshot::MultiHostName { dns_name } => {
            enc.array(2).ok();
            enc.u32(2).ok();
            enc.str(dns_name).ok();
        }
    }
}

/// Encode a Map<pool_hash(28), PoolParams> for pool state queries.
fn encode_pool_params_map(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    params: &[crate::query_handler::PoolParamsSnapshot],
) {
    enc.map(params.len() as u64).ok();
    for pool in params {
        enc.bytes(&pool.pool_id).ok();
        enc.array(9).ok();
        enc.bytes(&pool.pool_id).ok(); // operator
        enc.bytes(&pool.vrf_keyhash).ok();
        enc.u64(pool.pledge).ok();
        enc.u64(pool.cost).ok();
        encode_tagged_rational(enc, pool.margin_num, pool.margin_den);
        enc.bytes(&pool.reward_account).ok();
        // owners as tag(258) set — sorted for canonical CBOR
        let mut sorted_owners = pool.owners.clone();
        sorted_owners.sort();
        enc.tag(minicbor::data::Tag::new(258)).ok();
        enc.array(sorted_owners.len() as u64).ok();
        for owner in &sorted_owners {
            enc.bytes(owner).ok();
        }
        // relays
        enc.array(pool.relays.len() as u64).ok();
        for relay in &pool.relays {
            encode_relay_cbor(enc, relay);
        }
        // metadata
        if let Some(url) = &pool.metadata_url {
            enc.array(2).ok();
            enc.str(url).ok();
            if let Some(hash) = &pool.metadata_hash {
                enc.bytes(hash).ok();
            } else {
                enc.bytes(&[0u8; 32]).ok();
            }
        } else {
            enc.null().ok();
        }
    }
}

/// Parse an ISO-8601 UTC timestamp to (year, dayOfYear, picosecondsOfDay).
/// Input format: "2022-04-01T00:00:00Z" or similar.
fn parse_utctime(s: &str) -> (u64, u64, u64) {
    // Try to parse "YYYY-MM-DDThh:mm:ssZ"
    let s = s.trim_end_matches('Z');
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() != 2 {
        return (2017, 266, 0); // fallback: mainnet system start
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time_parts: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();

    if date_parts.len() < 3 || time_parts.len() < 3 {
        return (2017, 266, 0);
    }

    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);

    // Calculate day of year
    let days_in_months: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut day_of_year = day;
    for (i, &days) in days_in_months.iter().enumerate().take((month - 1) as usize) {
        day_of_year += days;
        if i == 1 && is_leap {
            day_of_year += 1;
        }
    }

    // Picoseconds of day
    let picos = (time_parts[0] * 3600 + time_parts[1] * 60 + time_parts[2]) * 1_000_000_000_000;

    (year, day_of_year, picos)
}

/// Encode SystemStart as UTCTime: [year, day_of_year, pico_of_day]
fn encode_system_start(enc: &mut minicbor::Encoder<&mut Vec<u8>>, time_str: &str) {
    let (year, day_of_year, picos) = parse_utctime(time_str);
    enc.array(3).ok();
    enc.u64(year).ok();
    enc.u64(day_of_year).ok();
    enc.u64(picos).ok();
}

/// Encode legacy Shelley PParams as array(18) (N2C V16-V20 legacy format).
fn encode_shelley_pparams(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &crate::query_handler::ShelleyPParamsSnapshot,
) {
    enc.array(18).ok();
    enc.u64(pp.min_fee_a).ok(); // [0] txFeePerByte
    enc.u64(pp.min_fee_b).ok(); // [1] txFeeFixed
    enc.u32(pp.max_block_body_size).ok(); // [2] maxBBSize
    enc.u32(pp.max_tx_size).ok(); // [3] maxTxSize
    enc.u16(pp.max_block_header_size).ok(); // [4] maxBHSize
    enc.u64(pp.key_deposit).ok(); // [5] keyDeposit
    enc.u64(pp.pool_deposit).ok(); // [6] poolDeposit
    enc.u32(pp.e_max).ok(); // [7] eMax
    enc.u16(pp.n_opt).ok(); // [8] nOpt
    encode_tagged_rational(enc, pp.a0_num, pp.a0_den); // [9] a0
    encode_tagged_rational(enc, pp.rho_num, pp.rho_den); // [10] rho
    encode_tagged_rational(enc, pp.tau_num, pp.tau_den); // [11] tau
    encode_tagged_rational(enc, pp.d_num, pp.d_den); // [12] d (decentralization)
                                                     // [13] extraEntropy: NeutralNonce = [0]
    enc.array(1).ok();
    enc.u32(0).ok();
    // [14] protocolVersion major
    enc.u64(pp.protocol_version_major).ok();
    // [15] protocolVersion minor
    enc.u64(pp.protocol_version_minor).ok();
    // [16] minUTxOValue
    enc.u64(pp.min_utxo_value).ok();
    // [17] minPoolCost
    enc.u64(pp.min_pool_cost).ok();
}

/// Test-only access to `parse_utctime`.
#[cfg(test)]
pub(crate) fn parse_utctime_for_test(s: &str) -> (u64, u64, u64) {
    parse_utctime(s)
}

pub(crate) fn encode_query_result(result: &QueryResult) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // MsgResult [4, result]
    // For BlockQuery (era-specific) results: [4, [result]]  (HFC success wrapper)
    // For QueryAnytime/QueryHardFork results: [4, result]   (no wrapper)
    enc.array(2).ok();
    enc.u32(4).ok(); // MsgResult tag

    // QueryVersion2 (N2C v16+) response encoding for BlockQuery:
    //
    // Top-level queries (outer tags 1/2/3: GetSystemStart/GetChainBlockNo/GetChainPoint):
    //   [4, toCBOR result]  — result directly, no HFC wrapping
    //
    // BlockQuery > QueryIfCurrent results (Shelley era queries):
    //   [4, array(1), result]  — EitherMismatch Right (success) wrapper
    //   The array(1) = Right in the Either encoding. No NS era index on results.
    //
    // BlockQuery > QueryAnytime results (GetCurrentEra, GetEraStart):
    //   [4, result]  — no EitherMismatch wrapping
    //
    // BlockQuery > QueryHardFork results (GetCurrentEra, GetInterpreter):
    //   [4, result]  — no EitherMismatch wrapping (raw word8 for era, encoded summary for history)
    let needs_either_mismatch = !matches!(
        result,
        // Top-level queries (no wrapping)
        QueryResult::SystemStart(_)
            | QueryResult::ChainBlockNo(_)
            | QueryResult::ChainTip { .. }
            | QueryResult::ChainPoint { .. }
            // BlockQuery > QueryAnytime results (no wrapping)
            | QueryResult::CurrentEra(_)
            // BlockQuery > QueryHardFork results (no wrapping)
            | QueryResult::HardForkCurrentEra(_)
            | QueryResult::EraHistory(_)
    );

    if needs_either_mismatch {
        // QueryIfCurrent results get EitherMismatch Right wrapper: array(1) = success
        enc.array(1).ok();
    }

    encode_query_result_value(&mut enc, result);

    buf
}

/// Encode just the query result value (no MsgResult wrapper, no HFC wrapper).
/// Used by `encode_query_result` for normal encoding and by `WrappedCbor` for inner encoding.
fn encode_query_result_value(enc: &mut minicbor::Encoder<&mut Vec<u8>>, result: &QueryResult) {
    match result {
        QueryResult::EpochNo(epoch) => {
            enc.u64(*epoch).ok();
        }
        QueryResult::ChainTip {
            slot,
            hash,
            block_no,
        } => {
            enc.array(2).ok();
            // Point: [slot, hash]
            enc.array(2).ok();
            enc.u64(*slot).ok();
            enc.bytes(hash).ok();
            // Block number
            enc.u64(*block_no).ok();
        }
        QueryResult::CurrentEra(era) => {
            enc.u32(*era).ok();
        }
        QueryResult::SystemStart(time_str) => {
            // SystemStart is a UTCTime, encoded as [year, day_of_year, pico_of_day]
            // Parse the ISO 8601 date string and convert to ordinal date representation
            encode_system_start(enc, time_str);
        }
        QueryResult::ChainBlockNo(block_no) => {
            // WithOrigin encoding (generic Serialise):
            //   Origin = [0] (constructor 0)
            //   At blockNo = [1, blockNo] (constructor 1)
            enc.array(2).ok();
            enc.u8(1).ok(); // At constructor
            enc.u64(*block_no).ok();
        }
        QueryResult::ChainPoint { slot, hash } => {
            // Point encoding: [] for Origin, [slot, hash] for Specific
            if hash.is_empty() {
                enc.array(0).ok();
            } else {
                enc.array(2).ok();
                enc.u64(*slot).ok();
                enc.bytes(hash).ok();
            }
        }
        QueryResult::ProtocolParams(pp) => {
            encode_protocol_params_cbor(enc, pp);
        }
        QueryResult::StakeDistribution(pools) => {
            // Wire format: Map<pool_hash(28), IndividualPoolStake>
            // IndividualPoolStake: array(2) [tag(30)[num,den], vrf_hash(32)]
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(2).ok();
                // Stake fraction as tagged rational
                let total = pool.total_active_stake.max(1); // avoid div by zero
                encode_tagged_rational(enc, pool.stake, total);
                enc.bytes(&pool.vrf_keyhash).ok();
            }
        }
        QueryResult::GovState(gov) => {
            // ConwayGovState = array(7):
            //   [0] Proposals, [1] Committee, [2] Constitution,
            //   [3] curPParams, [4] prevPParams, [5] FuturePParams,
            //   [6] DRepPulsingState
            enc.array(7).ok();

            // [0] Proposals = array(2) [roots, values]
            enc.array(2).ok();
            // roots = array(4) of StrictMaybe GovPurposeId
            // Order: [PParamUpdate, HardFork, Committee, Constitution]
            enc.array(4).ok();
            let roots = [
                &gov.enacted_pparam_update,
                &gov.enacted_hard_fork,
                &gov.enacted_committee,
                &gov.enacted_constitution,
            ];
            for root in &roots {
                match root {
                    Some((tx_hash, action_index)) => {
                        // StrictMaybe Just = array(1) [GovActionId]
                        enc.array(1).ok();
                        enc.array(2).ok();
                        enc.bytes(tx_hash).ok();
                        enc.u32(*action_index).ok();
                    }
                    None => {
                        enc.array(0).ok(); // StrictMaybe Nothing = array(0)
                    }
                }
            }
            // values = array(n) of GovActionState
            enc.array(gov.proposals.len() as u64).ok();
            for p in &gov.proposals {
                encode_gov_action_state(enc, p);
            }

            // [1] Committee = StrictMaybe(array(2) [Map<ColdCred,EpochNo>, UnitInterval])
            if gov.committee.members.is_empty() && gov.committee.threshold.is_none() {
                enc.array(0).ok(); // StrictMaybe Nothing
            } else {
                enc.array(1).ok(); // StrictMaybe Just
                enc.array(2).ok();
                // Map<ColdCredential, EpochNo>
                enc.map(gov.committee.members.len() as u64).ok();
                for m in &gov.committee.members {
                    // Key: Credential [type, hash]
                    enc.array(2).ok();
                    enc.u8(m.cold_credential_type).ok();
                    enc.bytes(&m.cold_credential).ok();
                    // Value: expiry epoch
                    enc.u64(m.expiry_epoch.unwrap_or(0)).ok();
                }
                // UnitInterval (quorum threshold)
                if let Some((num, den)) = gov.committee.threshold {
                    encode_tagged_rational(enc, num, den);
                } else {
                    encode_tagged_rational(enc, 2, 3); // default 2/3
                }
            }

            // [2] Constitution = array(2) [Anchor, StrictMaybe ScriptHash]
            enc.array(2).ok();
            // Anchor = array(2) [url, hash]
            enc.array(2).ok();
            enc.str(&gov.constitution_url).ok();
            enc.bytes(&gov.constitution_hash).ok();
            // StrictMaybe ScriptHash (null-encoded: null=Nothing, bytes=Just)
            if let Some(ref script) = gov.constitution_script {
                enc.bytes(script).ok();
            } else {
                enc.null().ok();
            }

            // [3] curPParams = array(31)
            encode_protocol_params_cbor(enc, &gov.cur_pparams);

            // [4] prevPParams = array(31)
            encode_protocol_params_cbor(enc, &gov.prev_pparams);

            // [5] FuturePParams = Sum: [0] = NoPParamsUpdate
            enc.array(1).ok();
            enc.u32(0).ok();

            // [6] DRepPulsingState = DRComplete: array(2) [PulsingSnapshot(4), RatifyState(4)]
            enc.array(2).ok();
            // PulsingSnapshot = array(4) [Map<DRep,Coin>, Map<Credential,Vote>, Map<GASId,Gas>, Map<Pool,IndivPoolStake>]
            enc.array(4).ok();
            enc.map(0).ok(); // drep stake distribution
            enc.map(0).ok(); // drep votes (credential->vote)
            enc.map(0).ok(); // proposals map
            enc.map(0).ok(); // pool stake distribution
                             // RatifyState = array(4) [enacted, expired, delayed_flag, future_pparams]
            enc.array(4).ok();
            // enacted proposals (tag(258) set)
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            // expired proposals (tag(258) set)
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            // delayed flag (bool)
            enc.bool(false).ok();
            // future pparams: [0] = NoPParamsUpdate
            enc.array(1).ok();
            enc.u32(0).ok();
        }
        QueryResult::DRepState(dreps) => {
            // Wire format: Map<Credential, DRepState>
            //   Credential: [0|1, hash(28)]
            //   DRepState: array(4) [expiry, maybe_anchor, deposit, tag(258)[delegators]]
            enc.map(dreps.len() as u64).ok();
            for drep in dreps {
                // Key: Credential
                enc.array(2).ok();
                enc.u8(drep.credential_type).ok();
                enc.bytes(&drep.credential_hash).ok();
                // Value: DRepState array(4)
                enc.array(4).ok();
                // [0] drepExpiry (EpochNo)
                enc.u64(drep.expiry_epoch).ok();
                // [1] drepAnchor (StrictMaybe Anchor)
                if let (Some(url), Some(hash)) = (&drep.anchor_url, &drep.anchor_hash) {
                    enc.array(1).ok(); // SJust
                    enc.array(2).ok(); // Anchor
                    enc.str(url).ok();
                    enc.bytes(hash).ok();
                } else {
                    enc.array(0).ok(); // SNothing
                }
                // [2] drepDeposit (Coin)
                enc.u64(drep.deposit).ok();
                // [3] drepDelegs: tag(258) Set of Credential — sorted for canonical CBOR
                let mut sorted_delegators = drep.delegator_hashes.clone();
                sorted_delegators.sort();
                enc.tag(minicbor::data::Tag::new(258)).ok();
                enc.array(sorted_delegators.len() as u64).ok();
                for dh in &sorted_delegators {
                    enc.array(2).ok();
                    enc.u8(0).ok(); // KeyHashObj
                    enc.bytes(dh).ok();
                }
            }
        }
        QueryResult::CommitteeState(committee) => {
            // Wire format: array(3) [map_members, maybe_threshold, epoch]
            enc.array(3).ok();
            // [0] Map<ColdCredential, CommitteeMemberState>
            enc.map(committee.members.len() as u64).ok();
            for member in &committee.members {
                // Key: Credential [type, hash(28)]
                enc.array(2).ok();
                enc.u8(member.cold_credential_type).ok();
                enc.bytes(&member.cold_credential).ok();
                // Value: CommitteeMemberState array(4)
                enc.array(4).ok();
                // [0] HotCredAuthStatus (Sum type)
                match member.hot_status {
                    0 => {
                        // MemberAuthorized: [0, credential]
                        enc.array(2).ok();
                        enc.u32(0).ok();
                        if let Some(hot) = &member.hot_credential {
                            enc.array(2).ok();
                            enc.u8(0).ok(); // KeyHashObj
                            enc.bytes(hot).ok();
                        }
                    }
                    1 => {
                        // MemberNotAuthorized: [1]
                        enc.array(1).ok();
                        enc.u32(1).ok();
                    }
                    _ => {
                        // MemberResigned: [2, maybe_anchor]
                        enc.array(2).ok();
                        enc.u32(2).ok();
                        enc.array(0).ok(); // SNothing anchor
                    }
                }
                // [1] MemberStatus enum (0=Active, 1=Expired, 2=Unrecognized)
                enc.u8(member.member_status).ok();
                // [2] Maybe EpochNo (expiration)
                if let Some(exp) = member.expiry_epoch {
                    enc.array(1).ok();
                    enc.u64(exp).ok();
                } else {
                    enc.array(0).ok();
                }
                // [3] NextEpochChange: NoChangeExpected [2]
                enc.array(1).ok();
                enc.u32(2).ok();
            }
            // [1] Maybe UnitInterval (threshold)
            if let Some((num, den)) = committee.threshold {
                enc.array(1).ok();
                encode_tagged_rational(enc, num, den);
            } else {
                enc.array(0).ok();
            }
            // [2] Current epoch
            enc.u64(committee.current_epoch).ok();
        }
        QueryResult::UtxoByAddress(utxos) => {
            // Cardano wire format: Map<[tx_hash, index], TransactionOutput>
            enc.map(utxos.len() as u64).ok();
            for utxo in utxos {
                // Key: [tx_hash, index]
                enc.array(2).ok();
                enc.bytes(&utxo.tx_hash).ok();
                enc.u32(utxo.output_index).ok();

                // Value: use pre-encoded raw CBOR if available (preserves original
                // wire format from the ledger), otherwise re-encode from snapshot fields.
                if let Some(raw) = &utxo.raw_cbor {
                    enc.writer_mut().extend_from_slice(raw);
                } else {
                    encode_utxo_output(enc, utxo);
                }
            }
        }
        QueryResult::StakeAddressInfo(addrs) => {
            // Wire format: array(2) [delegations_map, rewards_map]
            // delegations_map: Map<Credential, pool_hash(28)>
            // rewards_map: Map<Credential, Coin>
            // Credential: [0, hash(28)] for KeyHash
            let delegated: Vec<_> = addrs
                .iter()
                .filter(|a| a.delegated_pool.is_some())
                .collect();
            enc.array(2).ok();
            // Delegations map
            enc.map(delegated.len() as u64).ok();
            for addr in &delegated {
                // Credential key
                enc.array(2).ok();
                enc.u32(0).ok(); // KeyHashObj
                enc.bytes(&addr.credential_hash).ok();
                // Pool hash value
                if let Some(pool) = addr.delegated_pool.as_ref() {
                    enc.bytes(pool).ok();
                } else {
                    enc.bytes(&[]).ok();
                }
            }
            // Rewards map
            enc.map(addrs.len() as u64).ok();
            for addr in addrs {
                // Credential key
                enc.array(2).ok();
                enc.u32(0).ok(); // KeyHashObj
                enc.bytes(&addr.credential_hash).ok();
                // Reward balance value
                enc.u64(addr.reward_balance).ok();
            }
        }
        QueryResult::StakeSnapshots(snapshots) => {
            // Wire format: array(4) [pool_map, mark_total, set_total, go_total]
            // pool_map: Map<pool_hash(28), array(3) [mark_stake, set_stake, go_stake]>
            enc.array(4).ok();
            enc.map(snapshots.pools.len() as u64).ok();
            for pool in &snapshots.pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(3).ok();
                enc.u64(pool.mark_stake).ok();
                enc.u64(pool.set_stake).ok();
                enc.u64(pool.go_stake).ok();
            }
            // Totals (NonZero Coin — must be >= 1)
            enc.u64(snapshots.total_mark_stake.max(1)).ok();
            enc.u64(snapshots.total_set_stake.max(1)).ok();
            enc.u64(snapshots.total_go_stake.max(1)).ok();
        }
        QueryResult::StakePools(pool_ids) => {
            // Wire format: tag(258) Set<KeyHash StakePool>
            // CBOR canonical Set requires elements in sorted order
            let mut sorted_ids = pool_ids.clone();
            sorted_ids.sort();
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(sorted_ids.len() as u64).ok();
            for pid in &sorted_ids {
                enc.bytes(pid).ok();
            }
        }
        QueryResult::PoolParams(params) => {
            encode_pool_params_map(enc, params);
        }
        QueryResult::PoolState {
            pool_params,
            future_pool_params,
            retiring,
            deposits,
        } => {
            // QueryPoolStateResult: array(4) [poolParams, futurePoolParams, retiring, deposits]
            enc.array(4).ok();
            // Map 0: current pool params
            encode_pool_params_map(enc, pool_params);
            // Map 1: future pool params
            encode_pool_params_map(enc, future_pool_params);
            // Map 2: retiring pools -> epoch
            enc.map(retiring.len() as u64).ok();
            for (pool_id, epoch) in retiring {
                enc.bytes(pool_id).ok();
                enc.u64(*epoch).ok();
            }
            // Map 3: deposits
            enc.map(deposits.len() as u64).ok();
            for (pool_id, coin) in deposits {
                enc.bytes(pool_id).ok();
                enc.u64(*coin).ok();
            }
        }
        QueryResult::AccountState { treasury, reserves } => {
            // Account state: [treasury, reserves]
            enc.array(2).ok();
            enc.u64(*treasury).ok();
            enc.u64(*reserves).ok();
        }
        QueryResult::GenesisConfig(gc) => {
            // CompactGenesis: array(15) matching ShelleyGenesis CBOR wire format
            enc.array(15).ok();

            // [0] systemStart: UTCTime = array(3) [year, dayOfYear, picosecondsOfDay]
            let (year, day_of_year, picos) = parse_utctime(&gc.system_start);
            enc.array(3).ok();
            enc.u64(year).ok();
            enc.u64(day_of_year).ok();
            enc.u64(picos).ok();

            // [1] networkMagic: u32
            enc.u32(gc.network_magic).ok();

            // [2] networkId: 0=Testnet, 1=Mainnet
            enc.u8(gc.network_id).ok();

            // [3] activeSlotsCoeff: [num, den] (NO tag(30))
            enc.array(2).ok();
            enc.u64(gc.active_slots_coeff_num).ok();
            enc.u64(gc.active_slots_coeff_den).ok();

            // [4] securityParam: u64
            enc.u64(gc.security_param).ok();

            // [5] epochLength: u64
            enc.u64(gc.epoch_length).ok();

            // [6] slotsPerKESPeriod: u64
            enc.u64(gc.slots_per_kes_period).ok();

            // [7] maxKESEvolutions: u64
            enc.u64(gc.max_kes_evolutions).ok();

            // [8] slotLength: Fixed E6 integer (microseconds)
            enc.u64(gc.slot_length_micros).ok();

            // [9] updateQuorum: u64
            enc.u64(gc.update_quorum).ok();

            // [10] maxLovelaceSupply: u64
            enc.u64(gc.max_lovelace_supply).ok();

            // [11] protocolParams: legacy Shelley PParams array(18)
            encode_shelley_pparams(enc, &gc.protocol_params);

            // [12] genDelegs: Map<hash28 -> array(2)[hash28, hash32]>
            enc.map(gc.gen_delegs.len() as u64).ok();
            for (genesis_hash, delegate_hash, vrf_hash) in &gc.gen_delegs {
                enc.bytes(genesis_hash).ok();
                enc.array(2).ok();
                enc.bytes(delegate_hash).ok();
                enc.bytes(vrf_hash).ok();
            }

            // [13] initialFunds: empty map (CompactGenesis)
            enc.map(0).ok();

            // [14] staking: array(2) [empty_map, empty_map] (CompactGenesis)
            enc.array(2).ok();
            enc.map(0).ok();
            enc.map(0).ok();
        }
        QueryResult::NonMyopicMemberRewards(rewards) => {
            // Map from stake_amount -> map from pool_id -> reward
            enc.map(rewards.len() as u64).ok();
            for entry in rewards {
                enc.u64(entry.stake_amount).ok();
                enc.map(entry.pool_rewards.len() as u64).ok();
                for (pool_id, reward) in &entry.pool_rewards {
                    enc.bytes(pool_id).ok();
                    enc.u64(*reward).ok();
                }
            }
        }
        QueryResult::ProposedPParamsUpdates => {
            // Empty map — Conway era uses governance proposals instead of PP updates
            enc.map(0).ok();
        }
        QueryResult::Constitution {
            url,
            data_hash,
            script_hash,
        } => {
            // Constitution = array(2) [Anchor, StrictMaybe ScriptHash]
            enc.array(2).ok();
            // Anchor = array(2) [url, hash]
            enc.array(2).ok();
            enc.str(url).ok();
            enc.bytes(data_hash).ok();
            // StrictMaybe ScriptHash (null-encoded)
            if let Some(script) = script_hash {
                enc.bytes(script).ok();
            } else {
                enc.null().ok();
            }
        }
        QueryResult::PoolDistr(pools) => {
            // Wire format: Map<pool_hash(28), IndividualPoolStake>
            // IndividualPoolStake: array(2) [tag(30)[num,den], vrf_hash(32)]
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(2).ok();
                // Rational as tag(30) [numerator, denominator]
                enc.tag(minicbor::data::Tag::new(30)).ok();
                enc.array(2).ok();
                enc.u64(pool.stake).ok();
                enc.u64(pool.total_active_stake.max(1)).ok();
                enc.bytes(&pool.vrf_keyhash).ok();
            }
        }
        QueryResult::StakeDelegDeposits(deposits) => {
            // Wire format: Map<Credential, Coin>
            // Credential: [0|1, hash(28)]
            enc.map(deposits.len() as u64).ok();
            for entry in deposits {
                enc.array(2).ok();
                enc.u8(entry.credential_type).ok();
                enc.bytes(&entry.credential_hash).ok();
                enc.u64(entry.deposit).ok();
            }
        }
        QueryResult::DRepStakeDistr(entries) => {
            // Wire format: Map<DRep, Coin>
            // DRep: [0, keyhash(28)] | [1, scripthash(28)] | [2] | [3]
            enc.map(entries.len() as u64).ok();
            for entry in entries {
                match entry.drep_type {
                    0 | 1 => {
                        enc.array(2).ok();
                        enc.u8(entry.drep_type).ok();
                        if let Some(ref h) = entry.drep_hash {
                            enc.bytes(h).ok();
                        }
                    }
                    _ => {
                        enc.array(1).ok();
                        enc.u8(entry.drep_type).ok();
                    }
                }
                enc.u64(entry.stake).ok();
            }
        }
        QueryResult::FilteredVoteDelegatees(delegatees) => {
            // Wire format: Map<Credential, DRep>
            // Credential: [0|1, hash(28)]
            // DRep: [0, keyhash(28)] | [1, scripthash(28)] | [2] | [3]
            enc.map(delegatees.len() as u64).ok();
            for entry in delegatees {
                // Key: Credential
                enc.array(2).ok();
                enc.u8(entry.credential_type).ok();
                enc.bytes(&entry.credential_hash).ok();
                // Value: DRep
                match entry.drep_type {
                    0 | 1 => {
                        enc.array(2).ok();
                        enc.u8(entry.drep_type).ok();
                        if let Some(ref h) = entry.drep_hash {
                            enc.bytes(h).ok();
                        }
                    }
                    _ => {
                        enc.array(1).ok();
                        enc.u8(entry.drep_type).ok();
                    }
                }
            }
        }
        QueryResult::EraHistory(summaries) => {
            // Wire format matching Haskell ouroboros-consensus Serialise instances:
            // Indefinite-length array of EraSummary entries.
            // Each EraSummary = array(3) [start_bound, era_end, era_params]
            // Bound = array(3) [relative_time_pico, slot_no, epoch_no]
            // EraEnd: EraEnd(bound) = encode bound directly; EraUnbounded = null (0xf6)
            // EraParams = array(4) [epoch_size, slot_length_ms, safe_zone, genesis_window]
            // SafeZone: StandardSafeZone(n) = array(3) [0, n, array(1)[0]]
            //           UnsafeIndefiniteSafeZone = array(1) [1]
            enc.begin_array().ok(); // indefinite-length array (0x9f)
            for (i, summary) in summaries.iter().enumerate() {
                enc.array(3).ok();
                // Start bound: [time_pico, slot, epoch]
                enc.array(3).ok();
                enc.u64(summary.start_time_pico).ok();
                enc.u64(summary.start_slot).ok();
                enc.u64(summary.start_epoch).ok();
                // Era end: EraEnd(bound) = Bound directly, EraUnbounded = null
                if let Some(end) = &summary.end {
                    enc.array(3).ok();
                    enc.u64(end.time_pico).ok();
                    enc.u64(end.slot).ok();
                    enc.u64(end.epoch).ok();
                } else {
                    enc.null().ok();
                }
                // Era params: [epoch_size, slot_length_ms, safe_zone, genesis_window]
                enc.array(4).ok();
                enc.u64(summary.epoch_size).ok();
                enc.u64(summary.slot_length_ms).ok();
                // Safe zone encoding
                let is_last = i == summaries.len() - 1;
                if is_last && summary.end.is_none() {
                    // Current/unbounded era: UnsafeIndefiniteSafeZone = array(1) [1]
                    enc.array(1).ok();
                    enc.u8(1).ok();
                } else {
                    // Past era or bounded: StandardSafeZone(n) = array(3) [0, n, [0]]
                    enc.array(3).ok();
                    enc.u8(0).ok();
                    enc.u64(summary.safe_zone).ok();
                    enc.array(1).ok();
                    enc.u8(0).ok();
                }
                enc.u64(summary.genesis_window).ok();
            }
            enc.end().ok(); // end indefinite-length array (0xff)
        }
        QueryResult::WrappedCbor(inner) => {
            // GetCBOR (tag 9): encode the inner result value as CBOR, then wrap in tag(24).
            // The inner encoding must NOT include the MsgResult [4,...] or HFC wrappers —
            // those are already provided by the outer encode_query_result call.
            let mut inner_buf = Vec::new();
            let mut inner_enc = minicbor::Encoder::new(&mut inner_buf);
            encode_query_result_value(&mut inner_enc, inner);
            enc.tag(minicbor::data::Tag::new(24)).ok();
            enc.bytes(&inner_buf).ok();
        }
        QueryResult::DebugEpochState {
            epoch,
            treasury,
            reserves,
            stake_pool_count,
            utxo_count,
            active_stake,
            delegations,
            rewards,
        } => {
            // Epoch state summary: array(8)
            // [epoch, treasury, reserves, pool_count, utxo_count, active_stake, delegations, rewards]
            enc.array(8).ok();
            enc.u64(*epoch).ok();
            enc.u64(*treasury).ok();
            enc.u64(*reserves).ok();
            enc.u64(*stake_pool_count).ok();
            enc.u64(*utxo_count).ok();
            enc.u64(*active_stake).ok();
            enc.u64(*delegations).ok();
            enc.u64(*rewards).ok();
        }
        QueryResult::DebugNewEpochState {
            epoch,
            block_number,
            slot,
            protocol_major,
            protocol_minor,
        } => {
            // New epoch state summary: array(5)
            // [epoch, block_number, slot, protocol_major, protocol_minor]
            enc.array(5).ok();
            enc.u64(*epoch).ok();
            enc.u64(*block_number).ok();
            enc.u64(*slot).ok();
            enc.u64(*protocol_major).ok();
            enc.u64(*protocol_minor).ok();
        }
        QueryResult::DebugChainDepState {
            last_slot,
            epoch_nonce,
            evolving_nonce,
            candidate_nonce,
            lab_nonce,
        } => {
            // Chain dep state: array(5)
            // [last_slot, epoch_nonce(bytes32), evolving_nonce(bytes32),
            //  candidate_nonce(bytes32), lab_nonce(bytes32)]
            enc.array(5).ok();
            enc.u64(*last_slot).ok();
            enc.bytes(epoch_nonce).ok();
            enc.bytes(evolving_nonce).ok();
            enc.bytes(candidate_nonce).ok();
            enc.bytes(lab_nonce).ok();
        }
        QueryResult::RewardProvenance {
            epoch,
            total_rewards_pot,
            treasury_tax,
            active_stake,
        } => {
            // Reward provenance: array(4) [epoch, rewards_pot, treasury_tax, active_stake]
            enc.array(4).ok();
            enc.u64(*epoch).ok();
            enc.u64(*total_rewards_pot).ok();
            enc.u64(*treasury_tax).ok();
            enc.u64(*active_stake).ok();
        }
        QueryResult::RewardInfoPools(pools) => {
            // Map<pool_hash(28), PoolRewardInfo>
            // PoolRewardInfo: array(7) [stake, owner_stake, pool_reward, leader_reward,
            //                           member_reward, margin_rational, cost]
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(7).ok();
                enc.u64(pool.stake).ok();
                enc.u64(pool.owner_stake).ok();
                enc.u64(pool.pool_reward).ok();
                enc.u64(pool.leader_reward).ok();
                enc.u64(pool.member_reward).ok();
                enc.tag(minicbor::data::Tag::new(30)).ok();
                enc.array(2).ok();
                enc.u64(pool.margin.0).ok();
                enc.u64(pool.margin.1).ok();
                enc.u64(pool.cost).ok();
            }
        }
        QueryResult::HardForkCurrentEra(era) => {
            // QueryHardFork GetCurrentEra result: EraIndex as raw word8
            enc.u8(*era as u8).ok();
        }
        QueryResult::Proposals(proposals) => {
            // GetProposals result: Seq (GovActionState) = OSet of GovActionState
            enc.array(proposals.len() as u64).ok();
            for p in proposals {
                encode_gov_action_state(enc, p);
            }
        }
        QueryResult::RatifyState {
            enacted,
            expired,
            delayed,
        } => {
            // RatifyState = array(4) [enacted_seq, expired_seq, delayed_bool, future_pparam_update]
            enc.array(4).ok();
            // enacted: Seq of (GovActionState, GovActionId)
            enc.array(enacted.len() as u64).ok();
            for (proposal, action_id) in enacted {
                enc.array(2).ok();
                encode_gov_action_state(enc, proposal);
                enc.array(2).ok();
                enc.bytes(&action_id.tx_id).ok();
                enc.u32(action_id.action_index).ok();
            }
            // expired: Seq of GovActionId
            enc.array(expired.len() as u64).ok();
            for action_id in expired {
                enc.array(2).ok();
                enc.bytes(&action_id.tx_id).ok();
                enc.u32(action_id.action_index).ok();
            }
            // delayed
            enc.bool(*delayed).ok();
            // future pparams: NoPParamsUpdate [0]
            enc.array(1).ok();
            enc.u32(0).ok();
        }
        QueryResult::NoFuturePParams => {
            // GetFuturePParams result: Maybe PParams = Nothing
            // Generic Serialise: Nothing = [0] (constructor 0)
            enc.array(1).ok();
            enc.u8(0).ok();
        }
        QueryResult::PoolDistr2 {
            pools,
            total_active_stake,
        } => {
            // SL.PoolDistr: array(2)[pool_map, total_active_stake]
            // Each pool entry: array(3)[stake_rational, compact_lovelace, vrf_hash]
            enc.array(2).ok();
            let total = *total_active_stake;
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(3).ok();
                // stake as rational fraction
                if total > 0 {
                    encode_tagged_rational(enc, pool.stake, total);
                } else {
                    encode_tagged_rational(enc, 0, 1);
                }
                // compact lovelace (absolute pool stake)
                enc.u64(pool.stake).ok();
                // VRF key hash
                enc.bytes(&pool.vrf_keyhash).ok();
            }
            // total active stake
            enc.u64(total).ok();
        }
        QueryResult::MaxMajorProtocolVersion(v) => {
            // Plain integer
            enc.u32(*v).ok();
        }
        QueryResult::LedgerPeerSnapshot(peers) => {
            // LedgerPeerSnapshotV2 (version 1): array(2) [1, array(2)[WithOrigin, pools_indef]]
            // Haskell: (WithOrigin SlotNo, [(AccPoolStake, (PoolStake, NonEmpty relay))])
            // Stakes are Rational: array(2)[numerator, denominator]
            // Only include "big ledger peers" — top pools controlling 90% of stake.

            // Sort by stake descending and filter to big peers (top 90%)
            let mut sorted: Vec<_> = peers.iter().filter(|p| p.stake > 0).collect();
            sorted.sort_by(|a, b| b.stake.cmp(&a.stake));
            let total_stake: u64 = sorted.iter().map(|p| p.stake).sum();
            let cutoff = total_stake * 9 / 10; // 90% threshold
            let mut acc_raw: u64 = 0;
            let mut big_peers = Vec::new();
            for peer in &sorted {
                big_peers.push(*peer);
                acc_raw += peer.stake;
                if acc_raw >= cutoff {
                    break;
                }
            }

            enc.array(2).ok();
            enc.u32(1).ok(); // version 1
            enc.array(2).ok();
            // WithOrigin: Origin = [0]  (we don't track the snapshot slot)
            enc.array(1).ok();
            enc.u32(0).ok();
            // pools: indefinite-length array
            enc.begin_array().ok();
            let mut acc_num: u64 = 0;
            for peer in &big_peers {
                acc_num += peer.stake;
                enc.array(2).ok();
                // AccPoolStake as Rational (accumulated stake / total)
                enc.array(2).ok();
                enc.u64(acc_num).ok();
                enc.u64(total_stake.max(1)).ok();
                // (PoolStake, relays)
                enc.array(2).ok();
                // PoolStake as Rational (relative stake)
                enc.array(2).ok();
                enc.u64(peer.stake).ok();
                enc.u64(total_stake.max(1)).ok();
                // relays: indefinite-length array (NonEmpty)
                enc.begin_array().ok();
                for relay in &peer.relays {
                    encode_ledger_relay(enc, relay);
                }
                enc.end().ok();
            }
            enc.end().ok(); // end pool list
        }
        QueryResult::StakePoolDefaultVote(entries) => {
            // Map<PoolId, DefaultVote>
            // DefaultVote: [0]=NoConfidence, [1]=Abstain, [2]=DRepVote
            enc.map(entries.len() as u64).ok();
            for entry in entries {
                enc.bytes(&entry.pool_id).ok();
                enc.array(1).ok();
                enc.u32(entry.default_vote as u32).ok();
            }
        }
        QueryResult::Error(msg) => {
            enc.str(msg).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_handler::*;

    /// Helper: encode a QueryResult and return the raw CBOR bytes.
    fn encode(result: &QueryResult) -> Vec<u8> {
        encode_query_result(result)
    }

    /// Helper: decode and verify the MsgResult [4, ...] envelope.
    /// Returns the decoder positioned after the envelope.
    fn decode_msg_result(buf: &[u8]) -> minicbor::Decoder<'_> {
        let mut dec = minicbor::Decoder::new(buf);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2, "MsgResult outer array must be 2");
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 4, "MsgResult tag must be 4");
        dec
    }

    /// Helper: strip HFC EitherMismatch Right wrapper array(1).
    fn strip_hfc(dec: &mut minicbor::Decoder<'_>) {
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1, "HFC success wrapper must be array(1)");
    }

    #[test]
    fn test_encode_epoch_no() {
        let buf = encode(&QueryResult::EpochNo(500));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        assert_eq!(dec.u64().unwrap(), 500);
    }

    #[test]
    fn test_encode_chain_block_no() {
        let buf = encode(&QueryResult::ChainBlockNo(12345));
        let mut dec = decode_msg_result(&buf);
        // No HFC wrapper for top-level queries
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u8().unwrap(), 1); // At constructor
        assert_eq!(dec.u64().unwrap(), 12345);
    }

    #[test]
    fn test_encode_chain_point() {
        let hash = vec![0xAB; 32];
        let buf = encode(&QueryResult::ChainPoint {
            slot: 42,
            hash: hash.clone(),
        });
        let mut dec = decode_msg_result(&buf);
        // No HFC wrapper
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u64().unwrap(), 42);
        assert_eq!(dec.bytes().unwrap(), &hash);
    }

    #[test]
    fn test_encode_chain_point_origin() {
        let buf = encode(&QueryResult::ChainPoint {
            slot: 0,
            hash: vec![],
        });
        let mut dec = decode_msg_result(&buf);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 0); // Origin = empty array
    }

    #[test]
    fn test_encode_system_start() {
        let buf = encode(&QueryResult::SystemStart(
            "2022-10-25T00:00:00Z".to_string(),
        ));
        let mut dec = decode_msg_result(&buf);
        // No HFC wrapper for SystemStart
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 3);
        let year = dec.u64().unwrap();
        let day_of_year = dec.u64().unwrap();
        let picos = dec.u64().unwrap();
        assert_eq!(year, 2022);
        assert_eq!(day_of_year, 298); // Oct 25 = day 298
        assert_eq!(picos, 0);
    }

    #[test]
    fn test_encode_current_era() {
        let buf = encode(&QueryResult::CurrentEra(6));
        let mut dec = decode_msg_result(&buf);
        // No HFC wrapper for QueryAnytime
        assert_eq!(dec.u32().unwrap(), 6);
    }

    #[test]
    fn test_encode_constitution() {
        let buf = encode(&QueryResult::Constitution {
            url: "https://example.com/constitution".to_string(),
            data_hash: vec![0xCC; 32],
            script_hash: Some(vec![0xDD; 28]),
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // Constitution = array(2) [Anchor, StrictMaybe ScriptHash]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        // Anchor = array(2) [url, hash]
        let arr2 = dec.array().unwrap().unwrap();
        assert_eq!(arr2, 2);
        assert_eq!(dec.str().unwrap(), "https://example.com/constitution");
        assert_eq!(dec.bytes().unwrap(), &[0xCC; 32]);
        // StrictMaybe ScriptHash (bytes for Just)
        assert_eq!(dec.bytes().unwrap(), &[0xDD; 28]);
    }

    #[test]
    fn test_encode_constitution_no_script() {
        let buf = encode(&QueryResult::Constitution {
            url: "https://example.com".to_string(),
            data_hash: vec![0xAA; 32],
            script_hash: None,
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        let _ = dec.array(); // anchor
        let _ = dec.str(); // url
        let _ = dec.bytes(); // hash
                             // StrictMaybe Nothing = null
        assert!(dec.null().is_ok());
    }

    #[test]
    fn test_encode_account_state() {
        let buf = encode(&QueryResult::AccountState {
            treasury: 42_000_000_000,
            reserves: 13_000_000_000_000,
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u64().unwrap(), 42_000_000_000);
        assert_eq!(dec.u64().unwrap(), 13_000_000_000_000);
    }

    #[test]
    fn test_encode_utxo_coin_only() {
        let buf = encode(&QueryResult::UtxoByAddress(vec![UtxoSnapshot {
            tx_hash: vec![0x11; 32],
            output_index: 0,
            address_bytes: vec![0x01; 57],
            lovelace: 5_000_000,
            multi_asset: vec![],
            datum_hash: None,
            raw_cbor: None,
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // Map with 1 entry
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        // Key: [tx_hash, index]
        let _ = dec.array();
        assert_eq!(dec.bytes().unwrap(), &[0x11; 32]);
        assert_eq!(dec.u32().unwrap(), 0);
        // Value: PostAlonzo output map
        let fields = dec.map().unwrap().unwrap();
        assert_eq!(fields, 2); // address + value (no datum)
                               // field 0: address
        assert_eq!(dec.u32().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0x01; 57]);
        // field 1: value (coin-only = plain integer)
        assert_eq!(dec.u32().unwrap(), 1);
        assert_eq!(dec.u64().unwrap(), 5_000_000);
    }

    #[test]
    fn test_encode_utxo_multi_asset() {
        let buf = encode(&QueryResult::UtxoByAddress(vec![UtxoSnapshot {
            tx_hash: vec![0x22; 32],
            output_index: 1,
            address_bytes: vec![0x02; 57],
            lovelace: 2_000_000,
            multi_asset: vec![(vec![0xAA; 28], vec![("Token1".as_bytes().to_vec(), 100)])],
            datum_hash: None,
            raw_cbor: None,
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let _ = dec.map(); // 1 entry
        let _ = dec.array(); // key
        let _ = dec.bytes(); // tx_hash
        let _ = dec.u32(); // index
        let _ = dec.map(); // output fields
        let _ = dec.u32(); // field 0
        let _ = dec.bytes(); // address
        assert_eq!(dec.u32().unwrap(), 1); // field 1 (value)
                                           // Multi-asset: [coin, {policy -> {asset -> qty}}]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u64().unwrap(), 2_000_000);
        let _ = dec.map(); // policy map
        assert_eq!(dec.bytes().unwrap(), &[0xAA; 28]);
        let _ = dec.map(); // asset map
        assert_eq!(dec.bytes().unwrap(), "Token1".as_bytes());
        assert_eq!(dec.u64().unwrap(), 100);
    }

    #[test]
    fn test_encode_utxo_raw_cbor_passthrough() {
        // When raw_cbor is present, it should be used directly instead of re-encoding
        let raw_output = vec![
            0xa2, // map(2)
            0x00, 0x41, 0xFF, // 0: bytes(1) 0xFF
            0x01, 0x1a, 0x00, 0x4c, 0x4b, 0x40, // 1: 5_000_000
        ];
        let buf = encode(&QueryResult::UtxoByAddress(vec![UtxoSnapshot {
            tx_hash: vec![0x33; 32],
            output_index: 2,
            address_bytes: vec![0x01; 57], // ignored when raw_cbor is present
            lovelace: 999,                 // ignored when raw_cbor is present
            multi_asset: vec![],
            datum_hash: None,
            raw_cbor: Some(raw_output.clone()),
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let _ = dec.map(); // 1 entry
        let _ = dec.array(); // key
        assert_eq!(dec.bytes().unwrap(), &[0x33; 32]);
        assert_eq!(dec.u32().unwrap(), 2);
        // Value should be the raw CBOR bytes directly
        let fields = dec.map().unwrap().unwrap();
        assert_eq!(fields, 2);
        assert_eq!(dec.u32().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0xFF]);
        assert_eq!(dec.u32().unwrap(), 1);
        assert_eq!(dec.u64().unwrap(), 5_000_000);
    }

    #[test]
    fn test_encode_stake_distribution() {
        let buf = encode(&QueryResult::StakeDistribution(vec![StakePoolSnapshot {
            pool_id: vec![0x33; 28],
            stake: 1_000_000,
            total_active_stake: 10_000_000,
            vrf_keyhash: vec![0x44; 32],
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        assert_eq!(dec.bytes().unwrap(), &[0x33; 28]);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        // Tagged rational
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 30);
        let _ = dec.array();
        assert_eq!(dec.u64().unwrap(), 1_000_000);
        assert_eq!(dec.u64().unwrap(), 10_000_000);
        assert_eq!(dec.bytes().unwrap(), &[0x44; 32]);
    }

    #[test]
    fn test_encode_stake_pools_sorted() {
        let mut pool_a = vec![0x01; 28];
        let mut pool_b = vec![0x02; 28];
        // Put them in reverse order — encoding should sort
        let buf = encode(&QueryResult::StakePools(vec![
            pool_b.clone(),
            pool_a.clone(),
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 258); // Set tag
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        // First should be pool_a (0x01...) since sorted
        pool_a.sort();
        pool_b.sort();
        let first = dec.bytes().unwrap().to_vec();
        let second = dec.bytes().unwrap().to_vec();
        assert!(first < second, "CBOR Set elements must be sorted");
    }

    #[test]
    fn test_encode_drep_state() {
        let buf = encode(&QueryResult::DRepState(vec![DRepSnapshot {
            credential_type: 0,
            credential_hash: vec![0x55; 28],
            expiry_epoch: 200,
            deposit: 500_000_000,
            anchor_url: Some("https://drep.example.com".to_string()),
            anchor_hash: Some(vec![0x66; 32]),
            delegator_hashes: vec![vec![0x77; 28]],
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        // Key: Credential [0, hash]
        let _ = dec.array();
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0x55; 28]);
        // Value: DRepState array(4)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 4);
        assert_eq!(dec.u64().unwrap(), 200); // expiry
                                             // anchor: SJust
        let sjust = dec.array().unwrap().unwrap();
        assert_eq!(sjust, 1);
        let _ = dec.array(); // Anchor
        assert_eq!(dec.str().unwrap(), "https://drep.example.com");
        assert_eq!(dec.bytes().unwrap(), &[0x66; 32]);
        // deposit
        assert_eq!(dec.u64().unwrap(), 500_000_000);
        // delegators: tag(258) Set
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 258);
    }

    #[test]
    fn test_encode_committee_state() {
        let buf = encode(&QueryResult::CommitteeState(CommitteeSnapshot {
            members: vec![CommitteeMemberSnapshot {
                cold_credential_type: 0,
                cold_credential: vec![0x88; 28],
                hot_status: 0, // Authorized
                hot_credential: Some(vec![0x99; 28]),
                member_status: 0, // Active
                expiry_epoch: Some(300),
            }],
            threshold: Some((2, 3)),
            current_epoch: 100,
        }));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // array(3) [members_map, maybe_threshold, epoch]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 3);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        // Key: credential
        let _ = dec.array();
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0x88; 28]);
        // Value: CommitteeMemberState array(4)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 4);
        // [0] HotCredAuthStatus: MemberAuthorized [0, credential]
        let _ = dec.array();
        assert_eq!(dec.u32().unwrap(), 0);
        let _ = dec.array(); // credential
        let _ = dec.u8(); // type
        assert_eq!(dec.bytes().unwrap(), &[0x99; 28]);
        // [1] status
        assert_eq!(dec.u8().unwrap(), 0);
        // [2] Maybe EpochNo: SJust
        let sjust = dec.array().unwrap().unwrap();
        assert_eq!(sjust, 1);
        assert_eq!(dec.u64().unwrap(), 300);
        // [3] NextEpochChange: [2] NoChangeExpected
        let _ = dec.array();
        assert_eq!(dec.u32().unwrap(), 2);
        // Threshold: SJust
        let sjust = dec.array().unwrap().unwrap();
        assert_eq!(sjust, 1);
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 30);
        // Current epoch
        let _ = dec.array();
        let _ = dec.u64(); // num
        let _ = dec.u64(); // den
        assert_eq!(dec.u64().unwrap(), 100);
    }

    #[test]
    fn test_encode_ratify_state() {
        let buf = encode(&QueryResult::RatifyState {
            enacted: Vec::new(),
            expired: Vec::new(),
            delayed: false,
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 4);
        // enacted: Seq (array)
        let enacted_len = dec.array().unwrap().unwrap();
        assert_eq!(enacted_len, 0);
        // expired: Seq (array)
        let expired_len = dec.array().unwrap().unwrap();
        assert_eq!(expired_len, 0);
        // delayed
        assert!(!dec.bool().unwrap());
    }

    #[test]
    fn test_encode_ratify_state_with_data() {
        let enacted_proposal = ProposalSnapshot {
            tx_id: vec![0x11; 32],
            action_index: 0,
            action_type: "InfoAction".to_string(),
            proposed_epoch: 100,
            expires_epoch: 106,
            yes_votes: 5,
            no_votes: 1,
            abstain_votes: 0,
            deposit: 100_000_000_000,
            return_addr: vec![0x00; 29],
            anchor_url: "https://example.com".to_string(),
            anchor_hash: vec![0xAA; 32],
            committee_votes: vec![],
            drep_votes: vec![],
            spo_votes: vec![],
        };
        let enacted_id = GovActionId {
            tx_id: vec![0x11; 32],
            action_index: 0,
        };
        let expired_id = GovActionId {
            tx_id: vec![0x22; 32],
            action_index: 3,
        };
        let buf = encode(&QueryResult::RatifyState {
            enacted: vec![(enacted_proposal, enacted_id)],
            expired: vec![expired_id],
            delayed: true,
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 4, "RatifyState must be array(4)");
        // enacted: array(1) of (GovActionState, GovActionId)
        let enacted_len = dec.array().unwrap().unwrap();
        assert_eq!(enacted_len, 1);
        // Each entry is array(2) [GovActionState, GovActionId]
        let pair = dec.array().unwrap().unwrap();
        assert_eq!(pair, 2);
        // Skip GovActionState (complex)
        dec.skip().unwrap();
        // GovActionId = array(2) [tx_hash, index]
        let gaid = dec.array().unwrap().unwrap();
        assert_eq!(gaid, 2);
        assert_eq!(dec.bytes().unwrap(), &[0x11; 32]);
        assert_eq!(dec.u32().unwrap(), 0);
        // expired: array(1) of GovActionId
        let expired_len = dec.array().unwrap().unwrap();
        assert_eq!(expired_len, 1);
        let gaid2 = dec.array().unwrap().unwrap();
        assert_eq!(gaid2, 2);
        assert_eq!(dec.bytes().unwrap(), &[0x22; 32]);
        assert_eq!(dec.u32().unwrap(), 3);
        // delayed = true
        assert!(dec.bool().unwrap());
        // future_pparams: NoPParamsUpdate [0]
        let fp = dec.array().unwrap().unwrap();
        assert_eq!(fp, 1);
        assert_eq!(dec.u32().unwrap(), 0);
    }

    #[test]
    fn test_encode_stake_deleg_deposits() {
        let buf = encode(&QueryResult::StakeDelegDeposits(vec![
            StakeDelegDepositEntry {
                credential_type: 0,
                credential_hash: vec![0xAA; 28],
                deposit: 2_000_000,
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        let _ = dec.array();
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0xAA; 28]);
        assert_eq!(dec.u64().unwrap(), 2_000_000);
    }

    #[test]
    fn test_encode_drep_stake_distr() {
        let buf = encode(&QueryResult::DRepStakeDistr(vec![
            DRepStakeEntry {
                drep_type: 0,
                drep_hash: Some(vec![0xBB; 28]),
                stake: 5_000_000,
            },
            DRepStakeEntry {
                drep_type: 2, // AlwaysAbstain
                drep_hash: None,
                stake: 1_000_000,
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 2);
        // Entry 1: [0, hash] -> 5M
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0xBB; 28]);
        assert_eq!(dec.u64().unwrap(), 5_000_000);
        // Entry 2: [2] -> 1M (AlwaysAbstain has no hash)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1);
        assert_eq!(dec.u8().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 1_000_000);
    }

    #[test]
    fn test_encode_filtered_vote_delegatees() {
        let buf = encode(&QueryResult::FilteredVoteDelegatees(vec![
            VoteDelegateeEntry {
                credential_type: 0,
                credential_hash: vec![0xCC; 28],
                drep_type: 0,
                drep_hash: Some(vec![0xDD; 28]),
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        // Key: Credential [0, hash]
        let _ = dec.array();
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0xCC; 28]);
        // Value: DRep [0, hash]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u8().unwrap(), 0);
        assert_eq!(dec.bytes().unwrap(), &[0xDD; 28]);
    }

    #[test]
    fn test_encode_hfc_wrapper_present_for_block_query() {
        // BlockQuery results (like EpochNo) MUST have HFC wrapper
        let buf = encode(&QueryResult::EpochNo(100));
        let mut dec = minicbor::Decoder::new(&buf);
        let _ = dec.array(); // outer [4, ...]
        let _ = dec.u32(); // tag 4
                           // Must be array(1) HFC wrapper
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1, "BlockQuery result must have HFC array(1) wrapper");
    }

    #[test]
    fn test_encode_no_hfc_wrapper_for_system_start() {
        // Top-level queries do NOT have HFC wrapper
        let buf = encode(&QueryResult::SystemStart(
            "2022-01-01T00:00:00Z".to_string(),
        ));
        let mut dec = minicbor::Decoder::new(&buf);
        let _ = dec.array(); // outer [4, ...]
        let _ = dec.u32(); // tag 4
                           // Next element should be array(3) (UTCTime), NOT array(1) (HFC wrapper)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 3, "SystemStart should NOT have HFC wrapper");
    }

    #[test]
    fn test_encode_no_hfc_wrapper_for_chain_block_no() {
        let buf = encode(&QueryResult::ChainBlockNo(999));
        let mut dec = minicbor::Decoder::new(&buf);
        let _ = dec.array(); // outer [4, ...]
        let _ = dec.u32(); // tag 4
                           // Should be array(2) [1, blockNo], NOT array(1) HFC wrapper
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(
            arr, 2,
            "ChainBlockNo should NOT have HFC wrapper, should be [1, blockNo]"
        );
    }

    #[test]
    fn test_encode_proposals() {
        let buf = encode(&QueryResult::Proposals(vec![ProposalSnapshot {
            tx_id: vec![0x11; 32],
            action_index: 0,
            action_type: "InfoAction".to_string(),
            proposed_epoch: 100,
            expires_epoch: 106,
            yes_votes: 5,
            no_votes: 1,
            abstain_votes: 0,
            deposit: 100_000_000_000,
            return_addr: vec![0x00; 29],
            anchor_url: "https://example.com".to_string(),
            anchor_hash: vec![0xAA; 32],
            committee_votes: vec![],
            drep_votes: vec![],
            spo_votes: vec![],
        }]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // Proposals: array(n)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1);
        // GovActionState: array(7)
        let gas_arr = dec.array().unwrap().unwrap();
        assert_eq!(gas_arr, 7, "GovActionState must be array(7)");
    }

    #[test]
    fn test_encode_pool_distr2() {
        let buf = encode(&QueryResult::PoolDistr2 {
            pools: vec![StakePoolSnapshot {
                pool_id: vec![0xAA; 28],
                stake: 500_000_000,
                vrf_keyhash: vec![0xBB; 32],
                total_active_stake: 1_000_000_000,
            }],
            total_active_stake: 1_000_000_000,
        });
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // SL.PoolDistr: array(2) [pool_map, total_active_stake]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        // pool_id
        assert_eq!(dec.bytes().unwrap(), &[0xAA; 28]);
        // IndividualPoolStake: array(3) [rational, compact_lovelace, vrf_hash]
        let pool_arr = dec.array().unwrap().unwrap();
        assert_eq!(pool_arr, 3);
        // rational (tagged)
        dec.skip().unwrap();
        // compact lovelace
        assert_eq!(dec.u64().unwrap(), 500_000_000);
        // VRF hash
        assert_eq!(dec.bytes().unwrap(), &[0xBB; 32]);
        // total_active_stake
        assert_eq!(dec.u64().unwrap(), 1_000_000_000);
    }

    #[test]
    fn test_encode_max_major_protocol_version() {
        let buf = encode(&QueryResult::MaxMajorProtocolVersion(10));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        assert_eq!(dec.u32().unwrap(), 10);
    }

    #[test]
    fn test_encode_ledger_peer_snapshot_empty() {
        let buf = encode(&QueryResult::LedgerPeerSnapshot(vec![]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // LedgerPeerSnapshot: array(2) [version, array(2) [WithOrigin, pools]]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u32().unwrap(), 1); // version 1
        let inner = dec.array().unwrap().unwrap();
        assert_eq!(inner, 2);
        // WithOrigin: Origin = [0]
        let wo = dec.array().unwrap().unwrap();
        assert_eq!(wo, 1);
        assert_eq!(dec.u32().unwrap(), 0);
    }

    #[test]
    fn test_encode_no_future_pparams() {
        let buf = encode(&QueryResult::NoFuturePParams);
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        // Maybe PParams = Nothing = [0]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1);
        assert_eq!(dec.u8().unwrap(), 0);
    }

    #[test]
    fn test_encode_pool_default_vote() {
        let buf = encode(&QueryResult::StakePoolDefaultVote(vec![
            PoolDefaultVoteEntry {
                pool_id: vec![0xEE; 28],
                default_vote: 1, // Abstain
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        assert_eq!(dec.bytes().unwrap(), &[0xEE; 28]);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 1);
        assert_eq!(dec.u32().unwrap(), 1); // Abstain
    }

    // -----------------------------------------------------------------------
    // Golden CBOR Tests — verify exact byte sequences for protocol compat
    // -----------------------------------------------------------------------

    /// Encode protocol params and verify the CBOR structure is array(31) with correct field order.
    #[test]
    fn golden_protocol_params_structure() {
        let pp = ProtocolParamsSnapshot::default();
        let buf = encode(&QueryResult::ProtocolParams(Box::new(pp.clone())));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);

        // PParams must be array(31) — the Conway positional encoding
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 31, "ConwayPParams must be array(31)");

        // [0] min_fee_a
        assert_eq!(dec.u64().unwrap(), pp.min_fee_a);
        // [1] min_fee_b
        assert_eq!(dec.u64().unwrap(), pp.min_fee_b);
        // [2] max_block_body_size
        assert_eq!(dec.u64().unwrap(), pp.max_block_body_size);
        // [3] max_tx_size
        assert_eq!(dec.u64().unwrap(), pp.max_tx_size);
        // [4] max_block_header_size
        assert_eq!(dec.u64().unwrap(), pp.max_block_header_size);
        // [5] key_deposit
        assert_eq!(dec.u64().unwrap(), pp.key_deposit);
        // [6] pool_deposit
        assert_eq!(dec.u64().unwrap(), pp.pool_deposit);
        // [7] e_max
        assert_eq!(dec.u64().unwrap(), pp.e_max);
        // [8] n_opt
        assert_eq!(dec.u64().unwrap(), pp.n_opt);

        // [9] a0 (tagged rational)
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 30, "a0 must use tag 30");
        let rat_arr = dec.array().unwrap().unwrap();
        assert_eq!(rat_arr, 2);
        assert_eq!(dec.u64().unwrap(), pp.a0_num);
        assert_eq!(dec.u64().unwrap(), pp.a0_den);

        // [10] rho (tagged rational)
        let _ = dec.tag().unwrap();
        let _ = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), pp.rho_num);
        assert_eq!(dec.u64().unwrap(), pp.rho_den);

        // [11] tau (tagged rational)
        let _ = dec.tag().unwrap();
        let _ = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), pp.tau_num);
        assert_eq!(dec.u64().unwrap(), pp.tau_den);

        // [12] protocolVersion [major, minor]
        let ver_arr = dec.array().unwrap().unwrap();
        assert_eq!(ver_arr, 2);
        assert_eq!(dec.u64().unwrap(), pp.protocol_version_major);
        assert_eq!(dec.u64().unwrap(), pp.protocol_version_minor);

        // [13] minPoolCost
        assert_eq!(dec.u64().unwrap(), pp.min_pool_cost);
        // [14] coinsPerUTxOByte
        assert_eq!(dec.u64().unwrap(), pp.ada_per_utxo_byte);
    }

    /// Verify tagged rational encoding: tag(30)[num, den]
    #[test]
    fn golden_tagged_rational() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_tagged_rational(&mut enc, 3, 10);

        // Expected: d8 1e (tag 30) 82 (array 2) 03 (int 3) 0a (int 10)
        assert_eq!(buf, vec![0xd8, 0x1e, 0x82, 0x03, 0x0a]);
    }

    /// Verify SystemStart encoding is a string
    #[test]
    fn golden_system_start() {
        let buf = encode(&QueryResult::SystemStart(
            "2022-11-18T00:00:00Z".to_string(),
        ));
        let dec = decode_msg_result(&buf);
        // SystemStart has its own encoding — just verify it round-trips
        // The decoder should be able to read *something* after the envelope
        assert!(dec.position() < buf.len());
    }

    /// Verify EraHistory encoding: indefinite array of EraSummary
    #[test]
    fn golden_era_history_structure() {
        let buf = encode(&QueryResult::EraHistory(vec![
            crate::query_handler::EraSummary {
                start_slot: 0,
                start_epoch: 0,
                start_time_pico: 0,
                end: None,
                slot_length_ms: 20_000,
                epoch_size: 4320,
                safe_zone: 4320,
                genesis_window: 36000,
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        // No HFC wrapper for EraHistory

        // Should start with indefinite array
        let arr_type = dec.array().unwrap();
        assert!(arr_type.is_none(), "EraHistory must use indefinite array");

        // First era summary should be array(3): [start, end, params]
        let summary_arr = dec.array().unwrap().unwrap();
        assert_eq!(summary_arr, 3);
    }

    /// Verify WithOrigin encoding for GetChainBlockNo
    #[test]
    fn golden_chain_block_no_at() {
        let buf = encode(&QueryResult::ChainBlockNo(42));
        let mut dec = decode_msg_result(&buf);
        // WithOrigin: [1, blockNo] for At
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u8().unwrap(), 1); // At constructor
        assert_eq!(dec.u64().unwrap(), 42);
    }

    /// Verify Point encoding for GetChainPoint
    #[test]
    fn golden_chain_point_specific() {
        let hash = vec![0xAB; 32];
        let buf = encode(&QueryResult::ChainPoint {
            slot: 100,
            hash: hash.clone(),
        });
        let mut dec = decode_msg_result(&buf);
        // Point: [slot, hash]
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2);
        assert_eq!(dec.u64().unwrap(), 100);
        assert_eq!(dec.bytes().unwrap(), &[0xAB; 32]);
    }

    /// Verify MaxMajorProtocolVersion encoding
    #[test]
    fn golden_max_major_protocol_version() {
        let buf = encode(&QueryResult::MaxMajorProtocolVersion(10));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        assert_eq!(dec.u64().unwrap(), 10);
    }

    // ---- CBOR golden hex tests ----

    /// Helper: encode ProtocolParamsSnapshot directly (bypassing QueryResult envelope)
    /// to isolate the pparams CBOR for golden comparison.
    fn encode_pparams_raw(pp: &ProtocolParamsSnapshot) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_protocol_params_cbor(&mut enc, pp);
        buf
    }

    /// Golden test: tagged rational tag(30)[n, d] produces exact bytes.
    #[test]
    fn golden_hex_tagged_rational() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_tagged_rational(&mut enc, 3, 10);
        let actual = hex::encode(&buf);
        // tag(30) = d8 1e, array(2) = 82, 3 = 03, 10 = 0a
        assert_eq!(
            actual, "d81e82030a",
            "Tagged rational tag(30)[3,10] CBOR encoding changed"
        );
    }

    /// Golden test: tagged rational with larger values.
    #[test]
    fn golden_hex_tagged_rational_large() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        encode_tagged_rational(&mut enc, 577, 10000);
        let actual = hex::encode(&buf);
        // tag(30) = d8 1e, array(2) = 82, 577 = 19 0241, 10000 = 19 2710
        assert_eq!(
            actual, "d81e82190241192710",
            "Tagged rational tag(30)[577,10000] CBOR encoding changed"
        );
    }

    /// Golden test: default ProtocolParamsSnapshot encodes to a stable hex string.
    /// If the encoding logic or default values change, this test will fail —
    /// update the expected hex only after verifying correctness.
    #[test]
    fn golden_hex_default_protocol_params() {
        let pp = ProtocolParamsSnapshot::default();
        let buf = encode_pparams_raw(&pp);
        let actual = hex::encode(&buf);

        // Capture the golden value. This was generated from the current encoding
        // of ProtocolParamsSnapshot::default() and must remain stable.
        let expected = GOLDEN_DEFAULT_PPARAMS_HEX;
        assert_eq!(
            actual, expected,
            "Default ProtocolParamsSnapshot CBOR encoding changed!\n\
             If this is intentional, update GOLDEN_DEFAULT_PPARAMS_HEX.\n\
             Actual:   {actual}\n\
             Expected: {expected}"
        );
    }

    /// Golden test: protocol params with cost models populated.
    #[test]
    fn golden_hex_protocol_params_with_cost_models() {
        let pp = ProtocolParamsSnapshot {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: 2_000_000,
            pool_deposit: 500_000_000,
            e_max: 18,
            n_opt: 500,
            a0_num: 3,
            a0_den: 10,
            rho_num: 3,
            rho_den: 1000,
            tau_num: 2,
            tau_den: 10,
            min_pool_cost: 170_000_000,
            ada_per_utxo_byte: 4310,
            // 3 cost model values each for simplicity
            cost_models_v1: Some(vec![100, 200, 300]),
            cost_models_v2: Some(vec![400, 500, 600]),
            cost_models_v3: None,
            execution_costs_mem_num: 577,
            execution_costs_mem_den: 10000,
            execution_costs_step_num: 721,
            execution_costs_step_den: 10000000,
            max_tx_ex_mem: 14_000_000,
            max_tx_ex_steps: 10_000_000_000,
            max_block_ex_mem: 62_000_000,
            max_block_ex_steps: 40_000_000_000,
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            protocol_version_major: 9,
            protocol_version_minor: 0,
            min_fee_ref_script_cost_per_byte: 15,
            drep_deposit: 500_000_000,
            drep_activity: 20,
            gov_action_deposit: 100_000_000_000,
            gov_action_lifetime: 6,
            committee_min_size: 7,
            committee_max_term_length: 146,
            dvt_pp_network_group_num: 67,
            dvt_pp_network_group_den: 100,
            dvt_pp_economic_group_num: 67,
            dvt_pp_economic_group_den: 100,
            dvt_pp_technical_group_num: 67,
            dvt_pp_technical_group_den: 100,
            dvt_pp_gov_group_num: 67,
            dvt_pp_gov_group_den: 100,
            dvt_hard_fork_num: 60,
            dvt_hard_fork_den: 100,
            dvt_no_confidence_num: 67,
            dvt_no_confidence_den: 100,
            dvt_committee_normal_num: 67,
            dvt_committee_normal_den: 100,
            dvt_committee_no_confidence_num: 60,
            dvt_committee_no_confidence_den: 100,
            dvt_constitution_num: 75,
            dvt_constitution_den: 100,
            dvt_treasury_withdrawal_num: 67,
            dvt_treasury_withdrawal_den: 100,
            pvt_motion_no_confidence_num: 51,
            pvt_motion_no_confidence_den: 100,
            pvt_committee_normal_num: 51,
            pvt_committee_normal_den: 100,
            pvt_committee_no_confidence_num: 51,
            pvt_committee_no_confidence_den: 100,
            pvt_hard_fork_num: 51,
            pvt_hard_fork_den: 100,
            pvt_pp_security_group_num: 51,
            pvt_pp_security_group_den: 100,
        };
        let buf = encode_pparams_raw(&pp);
        let actual = hex::encode(&buf);

        let expected = GOLDEN_COST_MODELS_PPARAMS_HEX;
        assert_eq!(
            actual, expected,
            "ProtocolParamsSnapshot with cost models CBOR encoding changed!\n\
             If this is intentional, update GOLDEN_COST_MODELS_PPARAMS_HEX.\n\
             Actual:   {actual}\n\
             Expected: {expected}"
        );
    }

    /// Golden test: empty cost models produce map(0) at position [15].
    #[test]
    fn golden_hex_empty_cost_models() {
        let pp = ProtocolParamsSnapshot {
            cost_models_v1: None,
            cost_models_v2: None,
            cost_models_v3: None,
            ..ProtocolParamsSnapshot::default()
        };
        let buf = encode_pparams_raw(&pp);
        let actual = hex::encode(&buf);

        // The default already has no cost models, so this should match
        let default_buf = encode_pparams_raw(&ProtocolParamsSnapshot::default());
        assert_eq!(
            buf, default_buf,
            "Empty cost models should produce same encoding as default (no cost models)"
        );

        // Verify the cost models section contains map(0) = a0
        // Find it by checking the encoding contains the map(0) byte
        assert!(
            actual.contains("a0"),
            "Empty cost models should encode as CBOR map(0)"
        );
    }

    /// Golden test: full QueryResult::ProtocolParams envelope (MsgResult + HFC wrapper).
    #[test]
    fn golden_hex_protocol_params_envelope() {
        let pp = ProtocolParamsSnapshot {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 65536,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: 2_000_000,
            pool_deposit: 500_000_000,
            e_max: 18,
            n_opt: 150,
            a0_num: 1,
            a0_den: 2,
            rho_num: 3,
            rho_den: 1000,
            tau_num: 2,
            tau_den: 10,
            min_pool_cost: 340_000_000,
            ada_per_utxo_byte: 4310,
            cost_models_v1: None,
            cost_models_v2: None,
            cost_models_v3: None,
            execution_costs_mem_num: 577,
            execution_costs_mem_den: 10000,
            execution_costs_step_num: 721,
            execution_costs_step_den: 10000000,
            max_tx_ex_mem: 10_000_000,
            max_tx_ex_steps: 10_000_000_000,
            max_block_ex_mem: 50_000_000,
            max_block_ex_steps: 40_000_000_000,
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            protocol_version_major: 10,
            protocol_version_minor: 0,
            min_fee_ref_script_cost_per_byte: 15,
            drep_deposit: 500_000_000,
            drep_activity: 20,
            gov_action_deposit: 100_000_000_000,
            gov_action_lifetime: 6,
            committee_min_size: 7,
            committee_max_term_length: 146,
            dvt_pp_network_group_num: 2,
            dvt_pp_network_group_den: 3,
            dvt_pp_economic_group_num: 2,
            dvt_pp_economic_group_den: 3,
            dvt_pp_technical_group_num: 2,
            dvt_pp_technical_group_den: 3,
            dvt_pp_gov_group_num: 2,
            dvt_pp_gov_group_den: 3,
            dvt_hard_fork_num: 3,
            dvt_hard_fork_den: 5,
            dvt_no_confidence_num: 2,
            dvt_no_confidence_den: 3,
            dvt_committee_normal_num: 2,
            dvt_committee_normal_den: 3,
            dvt_committee_no_confidence_num: 3,
            dvt_committee_no_confidence_den: 5,
            dvt_constitution_num: 3,
            dvt_constitution_den: 4,
            dvt_treasury_withdrawal_num: 2,
            dvt_treasury_withdrawal_den: 3,
            pvt_motion_no_confidence_num: 51,
            pvt_motion_no_confidence_den: 100,
            pvt_committee_normal_num: 51,
            pvt_committee_normal_den: 100,
            pvt_committee_no_confidence_num: 51,
            pvt_committee_no_confidence_den: 100,
            pvt_hard_fork_num: 51,
            pvt_hard_fork_den: 100,
            pvt_pp_security_group_num: 51,
            pvt_pp_security_group_den: 100,
        };
        let buf = encode(&QueryResult::ProtocolParams(Box::new(pp)));
        let actual = hex::encode(&buf);

        let expected = GOLDEN_PPARAMS_ENVELOPE_HEX;
        assert_eq!(
            actual, expected,
            "Full ProtocolParams QueryResult envelope CBOR encoding changed!\n\
             If this is intentional, update GOLDEN_PPARAMS_ENVELOPE_HEX.\n\
             Actual:   {actual}\n\
             Expected: {expected}"
        );
    }

    // ===== Additional CBOR conformance tests =====

    /// Verify protocol params produces valid CBOR that can be fully decoded field by field.
    #[test]
    fn test_protocol_params_full_field_decode() {
        let pp = ProtocolParamsSnapshot {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: 2_000_000,
            pool_deposit: 500_000_000,
            e_max: 18,
            n_opt: 500,
            a0_num: 3,
            a0_den: 10,
            rho_num: 3,
            rho_den: 1000,
            tau_num: 2,
            tau_den: 10,
            protocol_version_major: 10,
            protocol_version_minor: 0,
            min_pool_cost: 170_000_000,
            ada_per_utxo_byte: 4310,
            cost_models_v1: None,
            cost_models_v2: Some(vec![100, 200, 300]),
            cost_models_v3: None,
            execution_costs_mem_num: 577,
            execution_costs_mem_den: 10000,
            execution_costs_step_num: 721,
            execution_costs_step_den: 10000000,
            max_tx_ex_mem: 14_000_000,
            max_tx_ex_steps: 10_000_000_000,
            max_block_ex_mem: 62_000_000,
            max_block_ex_steps: 40_000_000_000,
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            min_fee_ref_script_cost_per_byte: 15,
            ..ProtocolParamsSnapshot::default()
        };
        let raw = encode_pparams_raw(&pp);
        let mut dec = minicbor::Decoder::new(&raw);

        // Must be array(31)
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 31);

        // Fields [0]-[8] are plain integers
        assert_eq!(dec.u64().unwrap(), 44); // [0] min_fee_a
        assert_eq!(dec.u64().unwrap(), 155381); // [1] min_fee_b
        assert_eq!(dec.u64().unwrap(), 90112); // [2] max_block_body_size
        assert_eq!(dec.u64().unwrap(), 16384); // [3] max_tx_size
        assert_eq!(dec.u64().unwrap(), 1100); // [4] max_block_header_size
        assert_eq!(dec.u64().unwrap(), 2_000_000); // [5] key_deposit
        assert_eq!(dec.u64().unwrap(), 500_000_000); // [6] pool_deposit
        assert_eq!(dec.u64().unwrap(), 18); // [7] e_max
        assert_eq!(dec.u64().unwrap(), 500); // [8] n_opt

        // [9] a0 (tagged rational)
        assert_eq!(dec.tag().unwrap().as_u64(), 30);
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 3);
        assert_eq!(dec.u64().unwrap(), 10);

        // [10] rho
        assert_eq!(dec.tag().unwrap().as_u64(), 30);
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 3);
        assert_eq!(dec.u64().unwrap(), 1000);

        // [11] tau
        assert_eq!(dec.tag().unwrap().as_u64(), 30);
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 10);

        // [12] protocolVersion
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 10);
        assert_eq!(dec.u64().unwrap(), 0);

        // [13] minPoolCost
        assert_eq!(dec.u64().unwrap(), 170_000_000);
        // [14] coinsPerUTxOByte
        assert_eq!(dec.u64().unwrap(), 4310);

        // [15] costModels: should have 1 entry (v2 only)
        let cm_map = dec.map().unwrap().unwrap();
        assert_eq!(cm_map, 1);
        assert_eq!(dec.u32().unwrap(), 1); // v2 key
        let cm_arr = dec.array().unwrap().unwrap();
        assert_eq!(cm_arr, 3);
        assert_eq!(dec.i64().unwrap(), 100);
        assert_eq!(dec.i64().unwrap(), 200);
        assert_eq!(dec.i64().unwrap(), 300);
    }

    /// Verify stake distribution encoding produces map with tagged rationals.
    #[test]
    fn test_stake_distribution_multiple_pools() {
        let buf = encode(&QueryResult::StakeDistribution(vec![
            StakePoolSnapshot {
                pool_id: vec![0x11; 28],
                stake: 5_000_000,
                total_active_stake: 100_000_000,
                vrf_keyhash: vec![0x22; 32],
            },
            StakePoolSnapshot {
                pool_id: vec![0x33; 28],
                stake: 3_000_000,
                total_active_stake: 100_000_000,
                vrf_keyhash: vec![0x44; 32],
            },
        ]));
        let mut dec = decode_msg_result(&buf);
        strip_hfc(&mut dec);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 2);

        // First pool
        let pool1_id = dec.bytes().unwrap().to_vec();
        let arr1 = dec.array().unwrap().unwrap();
        assert_eq!(arr1, 2);
        let tag1 = dec.tag().unwrap();
        assert_eq!(tag1.as_u64(), 30);
        let _ = dec.array().unwrap(); // [num, den]
        let _ = dec.u64().unwrap(); // num
        let _ = dec.u64().unwrap(); // den
        let vrf1 = dec.bytes().unwrap().to_vec();
        assert_eq!(vrf1.len(), 32);

        // Second pool
        let pool2_id = dec.bytes().unwrap().to_vec();
        assert_ne!(pool1_id, pool2_id);
        let arr2 = dec.array().unwrap().unwrap();
        assert_eq!(arr2, 2);
    }

    /// Verify SystemStart UTC time encoding for non-midnight time.
    #[test]
    fn test_system_start_with_time() {
        let buf = encode(&QueryResult::SystemStart(
            "2022-10-25T12:30:45Z".to_string(),
        ));
        let mut dec = decode_msg_result(&buf);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 3);
        let year = dec.u64().unwrap();
        let day_of_year = dec.u64().unwrap();
        let picos = dec.u64().unwrap();
        assert_eq!(year, 2022);
        assert_eq!(day_of_year, 298); // Oct 25 = day 298
                                      // 12h30m45s in picoseconds
        let expected_picos: u64 = (12 * 3600 + 30 * 60 + 45) * 1_000_000_000_000;
        assert_eq!(picos, expected_picos);
    }

    /// Verify EraHistory with multiple eras including bounded and unbounded.
    #[test]
    fn test_era_history_multiple_eras() {
        let buf = encode(&QueryResult::EraHistory(vec![
            crate::query_handler::EraSummary {
                start_slot: 0,
                start_epoch: 0,
                start_time_pico: 0,
                end: Some(crate::query_handler::EraBound {
                    time_pico: 1_000_000_000_000,
                    slot: 100,
                    epoch: 10,
                }),
                slot_length_ms: 20_000,
                epoch_size: 100,
                safe_zone: 200,
                genesis_window: 36000,
            },
            crate::query_handler::EraSummary {
                start_slot: 100,
                start_epoch: 10,
                start_time_pico: 1_000_000_000_000,
                end: None, // unbounded
                slot_length_ms: 1_000,
                epoch_size: 432000,
                safe_zone: 129600,
                genesis_window: 36000,
            },
        ]));
        let mut dec = decode_msg_result(&buf);

        // Indefinite array
        let arr_type = dec.array().unwrap();
        assert!(arr_type.is_none(), "Must be indefinite array");

        // First era: array(3) [start, end, params]
        let s1 = dec.array().unwrap().unwrap();
        assert_eq!(s1, 3);

        // Start bound: array(3) [time, slot, epoch]
        let start1 = dec.array().unwrap().unwrap();
        assert_eq!(start1, 3);
        assert_eq!(dec.u64().unwrap(), 0); // time
        assert_eq!(dec.u64().unwrap(), 0); // slot
        assert_eq!(dec.u64().unwrap(), 0); // epoch

        // End bound (not null since era has end)
        let end1 = dec.array().unwrap().unwrap();
        assert_eq!(end1, 3);
        assert_eq!(dec.u64().unwrap(), 1_000_000_000_000); // time
        assert_eq!(dec.u64().unwrap(), 100); // slot
        assert_eq!(dec.u64().unwrap(), 10); // epoch

        // Era params: array(4) [epoch_size, slot_length, safe_zone, genesis_window]
        let params1 = dec.array().unwrap().unwrap();
        assert_eq!(params1, 4);

        let _ = dec.u64().unwrap(); // epoch_size
        let _ = dec.u64().unwrap(); // slot_length
                                    // safe_zone: StandardSafeZone = array(3) [0, n, [0]]
        let sz_arr = dec.array().unwrap().unwrap();
        assert_eq!(sz_arr, 3);
        assert_eq!(dec.u8().unwrap(), 0); // StandardSafeZone tag
        let _ = dec.u64().unwrap(); // safe_zone value
        let _ = dec.array().unwrap(); // inner [0]
        let _ = dec.u8().unwrap(); // 0
        let _ = dec.u64().unwrap(); // genesis_window

        // Second era
        let s2 = dec.array().unwrap().unwrap();
        assert_eq!(s2, 3);
    }

    /// Verify CurrentEra encoding for different era numbers.
    #[test]
    fn test_current_era_various() {
        for era in [0u32, 1, 5, 6] {
            let buf = encode(&QueryResult::CurrentEra(era));
            let mut dec = decode_msg_result(&buf);
            assert_eq!(dec.u32().unwrap(), era);
        }
    }

    // Golden reference hex values for CBOR encoding stability.
    // These were captured from the current (known-correct) encoding implementation.
    // Update ONLY after verifying the new encoding is correct and intentional.

    /// Default ProtocolParamsSnapshot encoded as CBOR array(31).
    const GOLDEN_DEFAULT_PPARAMS_HEX: &str = "981f182c1a00025ef51a0001600019400019044c1a001e84801a1dcd6500121901f4d81e82030ad81e82031903e8d81e82020a8209001a0a21fe801910d6a082d81e82190241192710d81e821902d11a00989680821a00d59f801b00000002540be400821a03b20b801b00000009502f900019138818960385d81e8218331864d81e8218331864d81e8218331864d81e8218331864d81e82183318648ad81e8218431864d81e8218431864d81e82183c1864d81e82184b1864d81e82183c1864d81e8218431864d81e8218431864d81e8218431864d81e8218431864d81e8218431864071892061b000000174876e8001a1dcd650014d81e820f01";

    /// ProtocolParamsSnapshot with V1+V2 cost models (3 values each).
    const GOLDEN_COST_MODELS_PPARAMS_HEX: &str = "981f182c1a00025ef51a0001600019400019044c1a001e84801a1dcd6500121901f4d81e82030ad81e82031903e8d81e82020a8209001a0a21fe801910d6a20083186418c819012c01831901901901f419025882d81e82190241192710d81e821902d11a00989680821a00d59f801b00000002540be400821a03b20b801b00000009502f900019138818960385d81e8218331864d81e8218331864d81e8218331864d81e8218331864d81e82183318648ad81e8218431864d81e8218431864d81e82183c1864d81e82184b1864d81e82183c1864d81e8218431864d81e8218431864d81e8218431864d81e8218431864d81e8218431864071892061b000000174876e8001a1dcd650014d81e820f01";

    /// Full QueryResult::ProtocolParams with MsgResult envelope + HFC wrapper.
    const GOLDEN_PPARAMS_ENVELOPE_HEX: &str = "820481981f182c1a00025ef51a0001000019400019044c1a001e84801a1dcd6500121896d81e820102d81e82031903e8d81e82020a820a001a1443fd001910d6a082d81e82190241192710d81e821902d11a00989680821a009896801b00000002540be400821a02faf0801b00000009502f900019138818960385d81e8218331864d81e8218331864d81e8218331864d81e8218331864d81e82183318648ad81e820203d81e820203d81e820305d81e820304d81e820305d81e820203d81e820203d81e820203d81e820203d81e820203071892061b000000174876e8001a1dcd650014d81e820f01";
}
