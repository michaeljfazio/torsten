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
        0 => {
            // MsgAcquire(point)
            debug!("LocalStateQuery: MsgAcquire");
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
            debug!("LocalStateQuery: MsgQuery");
            let handler = query_handler.read().await;
            let result = handler.handle_query_cbor(payload);
            let response_cbor = encode_query_result(&result);

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: response_cbor,
            }))
        }
        5 => {
            // MsgReAcquire(point)
            debug!("LocalStateQuery: MsgReAcquire");
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

    // Determine if this is a BlockQuery result that needs HFC wrapping.
    // QueryAnytime (CurrentEra, SystemStart) and QueryHardFork (ChainBlockNo, ChainTip)
    // do NOT get the HFC wrapper. Only BlockQuery (Shelley/Conway) results DO.
    let needs_hfc_wrapper = !matches!(
        result,
        QueryResult::CurrentEra(_)
            | QueryResult::SystemStart(_)
            | QueryResult::ChainBlockNo(_)
            | QueryResult::ChainTip { .. }
            | QueryResult::EraHistory(_)
    );

    if needs_hfc_wrapper {
        enc.array(1).ok(); // HFC success wrapper: array(1) = Right
    }

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
            enc.str(time_str).ok();
        }
        QueryResult::ChainBlockNo(block_no) => {
            enc.u64(*block_no).ok();
        }
        QueryResult::ProtocolParams(pp) => {
            encode_protocol_params_cbor(&mut enc, pp);
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
                encode_tagged_rational(&mut enc, pool.stake, total);
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
                // GovActionState = array(7)
                //   [0] gasId, [1] committeeVotes, [2] drepVotes,
                //   [3] spoVotes, [4] procedure, [5] proposedIn, [6] expiresAfter
                enc.array(7).ok();
                // [0] GovActionId = array(2) [tx_hash, action_index]
                enc.array(2).ok();
                enc.bytes(&p.tx_id).ok();
                enc.u32(p.action_index).ok();
                // [1] committeeVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [2] drepVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [3] spoVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [4] ProposalProcedure = array(4) [deposit, return_addr, gov_action, anchor]
                enc.array(4).ok();
                enc.u64(p.deposit).ok();
                enc.bytes(&p.return_addr).ok();
                // gov_action = sum type tagged by action type
                encode_gov_action_tag(&mut enc, &p.action_type);
                // anchor = array(2) [url, hash]
                enc.array(2).ok();
                enc.str(&p.anchor_url).ok();
                enc.bytes(&p.anchor_hash).ok();
                // [5] proposedIn (EpochNo)
                enc.u64(p.proposed_epoch).ok();
                // [6] expiresAfter (EpochNo)
                enc.u64(p.expires_epoch).ok();
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
                    encode_tagged_rational(&mut enc, num, den);
                } else {
                    encode_tagged_rational(&mut enc, 2, 3); // default 2/3
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
            encode_protocol_params_cbor(&mut enc, &gov.cur_pparams);

            // [4] prevPParams = array(31)
            encode_protocol_params_cbor(&mut enc, &gov.prev_pparams);

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
                // [3] drepDelegs: tag(258) Set of Credential
                enc.tag(minicbor::data::Tag::new(258)).ok();
                enc.array(drep.delegator_hashes.len() as u64).ok();
                for dh in &drep.delegator_hashes {
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
                encode_tagged_rational(&mut enc, num, den);
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

                // Value: PostAlonzo TransactionOutput as CBOR map {0: addr, 1: value, ...}
                encode_utxo_output(&mut enc, utxo);
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
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(pool_ids.len() as u64).ok();
            for pid in pool_ids {
                enc.bytes(pid).ok();
            }
        }
        QueryResult::PoolParams(params) => {
            // Wire format: Map<pool_hash(28), PoolParams>
            // PoolParams is a CDDL record (positional fields, no array wrapper):
            //   operator(hash28), vrf_keyhash(hash32), pledge(coin), cost(coin),
            //   margin(unit_interval), reward_account(bytes), owners(set<hash28>),
            //   relays([*relay]), metadata(nullable [url, hash])
            enc.map(params.len() as u64).ok();
            for pool in params {
                // Key: pool hash
                enc.bytes(&pool.pool_id).ok();
                // Value: positional PoolParams fields (9 items, NOT wrapped in array)
                // Per CDDL: pool_params = (operator, vrf_keyhash, pledge, cost, margin,
                //            reward_account, pool_owners, relays, pool_metadata)
                // When used as a map value in GetStakePoolParams result, each value
                // is encoded as a 9-element array.
                enc.array(9).ok();
                enc.bytes(&pool.pool_id).ok(); // operator
                enc.bytes(&pool.vrf_keyhash).ok();
                enc.u64(pool.pledge).ok();
                enc.u64(pool.cost).ok();
                // margin as tagged rational
                encode_tagged_rational(&mut enc, pool.margin_num, pool.margin_den);
                enc.bytes(&pool.reward_account).ok();
                // owners as set (tag 258)
                enc.tag(minicbor::data::Tag::new(258)).ok();
                enc.array(pool.owners.len() as u64).ok();
                for owner in &pool.owners {
                    enc.bytes(owner).ok();
                }
                // relays
                enc.array(pool.relays.len() as u64).ok();
                for relay in &pool.relays {
                    encode_relay_cbor(&mut enc, relay);
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
            encode_shelley_pparams(&mut enc, &gc.protocol_params);

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
            // Wire format: array of EraSummary entries (no HFC wrapper)
            // Each EraSummary = [start_bound, era_end, era_params]
            // Bound = [relative_time_pico, slot_no, epoch_no]
            // EraEnd = Bound | null
            // EraParams = [epoch_size, slot_length_ms, safe_zone, genesis_window]
            // SafeZone: StandardSafeZone(n) = [3, 0, n, [1, 0]]
            //           UnsafeIndefiniteSafeZone = [1, 1]
            enc.array(summaries.len() as u64).ok();
            for (i, summary) in summaries.iter().enumerate() {
                enc.array(3).ok();
                // Start bound
                enc.array(3).ok();
                enc.u64(summary.start_time_pico).ok();
                enc.u64(summary.start_slot).ok();
                enc.u64(summary.start_epoch).ok();
                // Era end
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
                if is_last {
                    // Current era: UnsafeIndefiniteSafeZone = [1, 1]
                    enc.array(2).ok();
                    enc.u64(1).ok();
                    enc.u64(1).ok();
                } else {
                    // Past era: StandardSafeZone(n) = [3, 0, n, [1, 0]]
                    enc.array(4).ok();
                    enc.u64(3).ok();
                    enc.u64(0).ok();
                    enc.u64(summary.safe_zone).ok();
                    enc.array(2).ok();
                    enc.u64(1).ok();
                    enc.u64(0).ok();
                }
                // genesis_window = null (optional, only in Peras)
                enc.null().ok();
            }
        }
        QueryResult::Error(msg) => {
            enc.str(msg).ok();
        }
    }

    buf
}
