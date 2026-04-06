//! CBOR encoding helpers for LocalStateQuery responses.
//!
//! This module contains `encode_query_result` (the top-level serializer) and
//! all supporting encode helpers.  The HFC wrapper logic (EitherMismatch Right
//! / QueryAnytime / QueryHardFork) lives here so that the dispatch layer in
//! `mod.rs` stays thin.

use crate::node::n2c_query::types::{
    DRepDelegationEntry, GovActionId, ProposalSnapshot, ProtocolParamsSnapshot, QueryResult,
    RelaySnapshot, ShelleyPParamsSnapshot, SnapshotStakeData, UtxoSnapshot,
};

// ─── Top-level result encoder ────────────────────────────────────────────────

/// Encode a `QueryResult` as a full N2C `MsgResult` response.
///
/// Wire format:
/// ```text
/// [4, result]                          -- QueryAnytime / QueryHardFork / top-level
/// [4, [result]]                        -- BlockQuery (EitherMismatch Right wrapper)
/// ```
#[allow(dead_code)] // used in tests
pub fn encode_query_result(result: &QueryResult) -> Vec<u8> {
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

/// Encode a `QueryResult` as the MsgResult payload (no `[4, ...]` envelope).
///
/// Returns the result with proper HFC wrapping:
/// - BlockQuery QueryIfCurrent results: `[1, result]` (EitherMismatch Right)
/// - Top-level / QueryAnytime / QueryHardFork results: `result` (no wrapping)
pub fn encode_query_result_payload(result: &QueryResult) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    let needs_either_mismatch = !matches!(
        result,
        QueryResult::SystemStart(_)
            | QueryResult::ChainBlockNo(_)
            | QueryResult::ChainTip { .. }
            | QueryResult::ChainPoint { .. }
            | QueryResult::CurrentEra(_)
            | QueryResult::HardForkCurrentEra(_)
            | QueryResult::EraHistory(_)
    );

    if needs_either_mismatch {
        enc.array(1).ok();
    }

    encode_query_result_value(&mut enc, result);
    buf
}

/// Encode just the query result value (no MsgResult wrapper, no HFC wrapper).
///
/// Used by `encode_query_result` for normal encoding and by `WrappedCbor` for
/// inner encoding (GetCBOR tag 9 wraps the inner result in `tag(24)`).
pub(crate) fn encode_query_result_value(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    result: &QueryResult,
) {
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
            encode_gov_state(enc, gov);
        }
        QueryResult::DRepState(dreps) => {
            encode_drep_state(enc, dreps);
        }
        QueryResult::CommitteeState(committee) => {
            encode_committee_state(enc, committee);
        }
        QueryResult::UtxoByAddress(utxos) => {
            encode_utxo_by_address(enc, utxos);
        }
        QueryResult::StakeAddressInfo(addrs) => {
            encode_stake_address_info(enc, addrs);
        }
        QueryResult::StakeSnapshots(snapshots) => {
            encode_stake_snapshots(enc, snapshots);
        }
        QueryResult::StakePools(pool_ids) => {
            encode_stake_pools(enc, pool_ids);
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
            encode_pool_state(enc, pool_params, future_pool_params, retiring, deposits);
        }
        QueryResult::AccountState { treasury, reserves } => {
            // Account state: [treasury, reserves]
            enc.array(2).ok();
            enc.u64(*treasury).ok();
            enc.u64(*reserves).ok();
        }
        QueryResult::GenesisConfig(gc, version) => {
            encode_genesis_config(enc, gc, *version);
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
            encode_constitution(enc, url, data_hash, script_hash.as_deref());
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
            encode_drep_stake_distr(enc, entries);
        }
        QueryResult::FilteredVoteDelegatees(delegatees) => {
            encode_filtered_vote_delegatees(enc, delegatees);
        }
        QueryResult::DRepDelegations(delegations) => {
            encode_drep_delegations(enc, delegations);
        }
        QueryResult::EraHistory(summaries) => {
            encode_era_history(enc, summaries);
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
            treasury,
            reserves,
            snap_mark,
            snap_set,
            snap_go,
            snap_fee,
        } => {
            encode_debug_epoch_state(
                enc, *treasury, *reserves, snap_mark, snap_set, snap_go, *snap_fee,
            );
        }
        QueryResult::DebugNewEpochState {
            epoch,
            blocks_made_prev,
            blocks_made_cur,
            treasury,
            reserves,
            snap_mark,
            snap_set,
            snap_go,
            snap_fee,
            total_active_stake,
            pool_distr,
        } => {
            encode_debug_new_epoch_state(
                enc,
                *epoch,
                blocks_made_prev,
                blocks_made_cur,
                *treasury,
                *reserves,
                snap_mark,
                snap_set,
                snap_go,
                *snap_fee,
                *total_active_stake,
                pool_distr,
            );
        }
        QueryResult::DebugChainDepState {
            last_slot,
            last_slot_is_origin,
            ocert_counters,
            evolving_nonce,
            candidate_nonce,
            epoch_nonce,
            lab_nonce,
            last_epoch_block_nonce,
        } => {
            encode_debug_chain_dep_state(
                enc,
                *last_slot,
                *last_slot_is_origin,
                ocert_counters,
                evolving_nonce,
                candidate_nonce,
                epoch_nonce,
                lab_nonce,
                last_epoch_block_nonce,
            );
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
            encode_reward_info_pools(enc, pools);
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
            gov,
            enacted,
            expired,
            delayed,
        } => {
            encode_ratify_state(enc, gov, enacted, expired, *delayed);
        }
        QueryResult::NoFuturePParams => {
            // GetFuturePParams result: Maybe PParams = Nothing
            // Haskell encodeMaybe: Nothing = encodeListLen 0 = empty array (0x80)
            enc.array(0).ok();
        }
        QueryResult::PoolDistr2 {
            pools,
            total_active_stake,
        } => {
            encode_pool_distr2(enc, pools, *total_active_stake);
        }
        QueryResult::MaxMajorProtocolVersion(v) => {
            // Plain integer
            enc.u32(*v).ok();
        }
        QueryResult::LedgerPeerSnapshot(peers) => {
            encode_ledger_peer_snapshot(enc, peers);
        }
        QueryResult::StakePoolDefaultVote(vote) => {
            // Bare word8: 0=DefaultNo, 1=DefaultAbstain, 2=DefaultNoConfidence
            enc.u8(*vote).ok();
        }
        QueryResult::SPOStakeDistr(entries) => {
            // Map<pool_hash(28), Coin> — plain map from pool key hash to lovelace
            enc.map(entries.len() as u64).ok();
            for (pool_id, stake) in entries {
                enc.bytes(pool_id).ok();
                enc.u64(*stake).ok();
            }
        }
        QueryResult::Error(msg) => {
            enc.str(msg).ok();
        }
    }
}

// ─── UTxO encoding ───────────────────────────────────────────────────────────

/// Encode a UTxO output in PostAlonzo format (CBOR map with integer keys).
///
/// Format: `{0: address_bytes, 1: value, 2?: datum_option, 3?: script_ref}`
/// Value: `coin` (integer) or `[coin, {policy_id -> {asset_name -> quantity}}]`
pub(crate) fn encode_utxo_output(enc: &mut minicbor::Encoder<&mut Vec<u8>>, utxo: &UtxoSnapshot) {
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

fn encode_utxo_by_address(enc: &mut minicbor::Encoder<&mut Vec<u8>>, utxos: &[UtxoSnapshot]) {
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

// ─── Protocol parameters encoding ────────────────────────────────────────────

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
pub(crate) fn encode_protocol_params_cbor(
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

/// Helper to encode a tagged rational number: `tag(30)[numerator, denominator]`
pub(crate) fn encode_tagged_rational(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    num: u64,
    den: u64,
) {
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(num).ok();
    enc.u64(den).ok();
}

// ─── Pool encoding ────────────────────────────────────────────────────────────

/// Encode a `Map<pool_hash(28), PoolParams>` for pool state queries.
pub(crate) fn encode_pool_params_map(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    params: &[crate::node::n2c_query::types::PoolParamsSnapshot],
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

fn encode_pool_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pool_params: &[crate::node::n2c_query::types::PoolParamsSnapshot],
    future_pool_params: &[crate::node::n2c_query::types::PoolParamsSnapshot],
    retiring: &[(Vec<u8>, u64)],
    deposits: &[(Vec<u8>, u64)],
) {
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

fn encode_pool_distr2(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pools: &[crate::node::n2c_query::types::StakePoolSnapshot],
    total_active_stake: u64,
) {
    // SL.PoolDistr: array(2)[pool_map, total_active_stake]
    // Each pool entry: array(3)[stake_rational, compact_lovelace, vrf_hash]
    enc.array(2).ok();
    let total = total_active_stake;
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

// ─── Stake encoding ───────────────────────────────────────────────────────────

fn encode_stake_address_info(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    addrs: &[crate::node::n2c_query::types::StakeAddressSnapshot],
) {
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

fn encode_stake_snapshots(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    snapshots: &crate::node::n2c_query::types::StakeSnapshotsResult,
) {
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

fn encode_stake_pools(enc: &mut minicbor::Encoder<&mut Vec<u8>>, pool_ids: &[Vec<u8>]) {
    // Wire format: tag(258) Set<KeyHash StakePool>
    // CBOR canonical Set requires elements in sorted order
    let mut sorted_ids = pool_ids.to_owned();
    sorted_ids.sort();
    enc.tag(minicbor::data::Tag::new(258)).ok();
    enc.array(sorted_ids.len() as u64).ok();
    for pid in &sorted_ids {
        enc.bytes(pid).ok();
    }
}

fn encode_drep_stake_distr(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    entries: &[crate::node::n2c_query::types::DRepStakeEntry],
) {
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

fn encode_filtered_vote_delegatees(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    delegatees: &[crate::node::n2c_query::types::VoteDelegateeEntry],
) {
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

/// Encode `GetDRepDelegations` (tag 39, V23+) response.
///
/// Wire format: `Map<Credential, DRep>`
///   Key: `array(2) [credential_type(0|1), hash(28)]`
///   Value: `array(2) [0|1, hash(28)]`  for KeyHash/ScriptHash DRep
///          `array(1) [2|3]`            for AlwaysAbstain / AlwaysNoConfidence
///
/// The encoding is identical to `GetFilteredVoteDelegatees` (tag 28), defined
/// as a separate function to keep the two query paths independent.
fn encode_drep_delegations(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    delegations: &[DRepDelegationEntry],
) {
    // Map<Credential, DRep>
    enc.map(delegations.len() as u64).ok();
    for entry in delegations {
        // Key: Credential = array(2) [type, hash(28)]
        enc.array(2).ok();
        enc.u8(entry.credential_type).ok();
        enc.bytes(&entry.credential_hash).ok();
        // Value: DRep
        match entry.drep_type {
            0 | 1 => {
                // KeyHash or ScriptHash DRep: array(2) [type, hash(28)]
                enc.array(2).ok();
                enc.u8(entry.drep_type).ok();
                if let Some(ref h) = entry.drep_hash {
                    enc.bytes(h).ok();
                }
            }
            _ => {
                // AlwaysAbstain (2) or AlwaysNoConfidence (3): array(1) [type]
                enc.array(1).ok();
                enc.u8(entry.drep_type).ok();
            }
        }
    }
}

// ─── Governance encoding ──────────────────────────────────────────────────────

fn encode_gov_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    gov: &crate::node::n2c_query::types::GovStateSnapshot,
) {
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

    // [6] DRepPulsingState = DRComplete (Rec, no constructor tag): array(2)
    //     [PulsingSnapshot, RatifyState]
    enc.array(2).ok();

    // PulsingSnapshot = array(4):
    //   [0] psProposals:  StrictSeq GovActionState (CBOR array, not map)
    //   [1] psDRepDistr:  Map DRep (CompactForm Coin)
    //   [2] psDRepState:  Map (Credential DRepRole) DRepState
    //   [3] psPoolDistr:  Map (KeyHash StakePool) (CompactForm Coin)
    enc.array(4).ok();
    enc.array(0).ok(); // psProposals: empty StrictSeq (array, NOT map)
    enc.map(0).ok(); // psDRepDistr
    enc.map(0).ok(); // psDRepState
    enc.map(0).ok(); // psPoolDistr

    // RatifyState = array(4):
    //   [0] rsEnactState:  EnactState (array(7))
    //   [1] rsEnacted:     Seq GovActionState (plain array, no tag 258)
    //   [2] rsExpired:     Set GovActionId (tag(258) + array)
    //   [3] rsDelayed:     Bool
    enc.array(4).ok();

    // [0] EnactState = array(7):
    //   [0] ensCommittee       StrictMaybe Committee
    //   [1] ensConstitution    Constitution
    //   [2] ensCurPParams      PParams
    //   [3] ensPrevPParams     PParams
    //   [4] ensTreasury        Coin
    //   [5] ensWithdrawals     Map (Credential Staking) Coin
    //   [6] ensPrevGovActionIds GovRelation StrictMaybe (array(4))
    enc.array(7).ok();

    // ensCommittee: reuse committee from ConwayGovState
    if gov.committee.members.is_empty() && gov.committee.threshold.is_none() {
        enc.array(0).ok(); // SNothing
    } else {
        enc.array(1).ok(); // SJust
        enc.array(2).ok();
        // Map<ColdCredential, EpochNo>
        enc.map(gov.committee.members.len() as u64).ok();
        for m in &gov.committee.members {
            enc.array(2).ok();
            enc.u8(m.cold_credential_type).ok();
            enc.bytes(&m.cold_credential).ok();
            enc.u64(m.expiry_epoch.unwrap_or(0)).ok();
        }
        // Quorum threshold
        if let Some((num, den)) = gov.committee.threshold {
            encode_tagged_rational(enc, num, den);
        } else {
            encode_tagged_rational(enc, 2, 3);
        }
    }

    // ensConstitution: array(2) [Anchor, StrictMaybe ScriptHash]
    enc.array(2).ok();
    enc.array(2).ok();
    enc.str(&gov.constitution_url).ok();
    enc.bytes(&gov.constitution_hash).ok();
    if let Some(ref script) = gov.constitution_script {
        enc.bytes(script).ok();
    } else {
        enc.null().ok();
    }

    // ensCurPParams
    encode_protocol_params_cbor(enc, &gov.cur_pparams);
    // ensPrevPParams
    encode_protocol_params_cbor(enc, &gov.prev_pparams);
    // ensTreasury
    enc.u64(gov.treasury).ok();
    // ensWithdrawals: empty map
    enc.map(0).ok();
    // ensPrevGovActionIds: GovRelation StrictMaybe = array(4) of StrictMaybe GovPurposeId
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
                enc.array(1).ok(); // SJust
                enc.array(2).ok(); // GovActionId
                enc.bytes(tx_hash).ok();
                enc.u32(*action_index).ok();
            }
            None => {
                enc.array(0).ok(); // SNothing
            }
        }
    }

    // [1] rsEnacted: Seq GovActionState (plain array, NOT tagged set)
    enc.array(0).ok();
    // [2] rsExpired: Set GovActionId (tag(258) + array)
    enc.tag(minicbor::data::Tag::new(258)).ok();
    enc.array(0).ok();
    // [3] rsDelayed: Bool
    enc.bool(false).ok();
}

fn encode_drep_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    dreps: &[crate::node::n2c_query::types::DRepSnapshot],
) {
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

fn encode_committee_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    committee: &crate::node::n2c_query::types::CommitteeSnapshot,
) {
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
                    enc.u8(member.hot_credential_type).ok(); // 0=KeyHashObj, 1=ScriptHashObj
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

fn encode_constitution(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    url: &str,
    data_hash: &[u8],
    script_hash: Option<&[u8]>,
) {
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

/// Encode a single `GovActionState` as CBOR `array(7)`.
pub(crate) fn encode_gov_action_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    p: &ProposalSnapshot,
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

/// Encode a `GovAction` as a CBOR sum type tag.
///
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

fn encode_ratify_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    gov: &crate::node::n2c_query::types::GovStateSnapshot,
    enacted: &[(ProposalSnapshot, GovActionId)],
    expired: &[GovActionId],
    delayed: bool,
) {
    // Haskell RatifyState = array(4):
    //   [0] EnactState(array(7))
    //   [1] rsEnacted: Seq GovActionState (plain array)
    //   [2] rsExpired: Set GovActionId (tag(258) + array)
    //   [3] rsDelayed: Bool
    enc.array(4).ok();

    // [0] EnactState = array(7) — reuses the same encoding as the embedded
    // version in encode_gov_state. See lines 991-1059 for the canonical
    // EnactState field order.
    enc.array(7).ok();
    // ensCommittee: StrictMaybe Committee
    if gov.committee.members.is_empty() && gov.committee.threshold.is_none() {
        enc.array(0).ok(); // SNothing
    } else {
        enc.array(1).ok(); // SJust
        enc.array(2).ok();
        enc.map(gov.committee.members.len() as u64).ok();
        for m in &gov.committee.members {
            enc.array(2).ok();
            enc.u8(m.cold_credential_type).ok();
            enc.bytes(&m.cold_credential).ok();
            enc.u64(m.expiry_epoch.unwrap_or(0)).ok();
        }
        if let Some((num, den)) = gov.committee.threshold {
            encode_tagged_rational(enc, num, den);
        } else {
            encode_tagged_rational(enc, 2, 3);
        }
    }
    // ensConstitution: array(2) [Anchor, StrictMaybe ScriptHash]
    enc.array(2).ok();
    enc.array(2).ok();
    enc.str(&gov.constitution_url).ok();
    enc.bytes(&gov.constitution_hash).ok();
    if let Some(ref script) = gov.constitution_script {
        enc.bytes(script).ok();
    } else {
        enc.null().ok();
    }
    // ensCurPParams
    encode_protocol_params_cbor(enc, &gov.cur_pparams);
    // ensPrevPParams
    encode_protocol_params_cbor(enc, &gov.prev_pparams);
    // ensTreasury
    enc.u64(gov.treasury).ok();
    // ensWithdrawals: empty map
    enc.map(0).ok();
    // ensPrevGovActionIds: GovRelation StrictMaybe = array(4)
    enc.array(4).ok();
    let roots = [
        &gov.enacted_pparam_update,
        &gov.enacted_hard_fork,
        &gov.enacted_committee,
        &gov.enacted_constitution,
    ];
    for root in &roots {
        if let Some((tx_id, action_index)) = root {
            enc.array(1).ok(); // SJust
            enc.array(2).ok();
            enc.bytes(tx_id).ok();
            enc.u32(*action_index).ok();
        } else {
            enc.array(0).ok(); // SNothing
        }
    }

    // [1] rsEnacted: Seq of GovActionState (plain array, no tag 258)
    enc.array(enacted.len() as u64).ok();
    for (proposal, action_id) in enacted {
        enc.array(2).ok();
        encode_gov_action_state(enc, proposal);
        enc.array(2).ok();
        enc.bytes(&action_id.tx_id).ok();
        enc.u32(action_id.action_index).ok();
    }
    // [2] rsExpired: Set of GovActionId (tag(258) + array per Haskell Set encoding)
    enc.tag(minicbor::data::Tag::new(258)).ok();
    enc.array(expired.len() as u64).ok();
    for action_id in expired {
        enc.array(2).ok();
        enc.bytes(&action_id.tx_id).ok();
        enc.u32(action_id.action_index).ok();
    }
    // [3] rsDelayed
    enc.bool(delayed).ok();
}

// ─── Relay encoding ───────────────────────────────────────────────────────────

/// Encode a `LedgerRelayAccessPoint` for `LedgerPeerSnapshot`.
///
/// Haskell wire format:
///   DNS domain:   `array(3) [0, port_integer, domain_bytestring]`
///   IPv4 address: `array(3) [1, port_integer, array(4)[o1, o2, o3, o4]]`
///   IPv6 address: `array(3) [2, port_integer, array(4)[w1, w2, w3, w4]]`
fn encode_ledger_relay(enc: &mut minicbor::Encoder<&mut Vec<u8>>, relay: &RelaySnapshot) {
    match relay {
        RelaySnapshot::SingleHostAddr { port, ipv4, ipv6 } => {
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
        RelaySnapshot::SingleHostName { port, dns_name } => {
            // DNS: [0, port, domain_bytes]
            enc.array(3).ok();
            enc.u32(0).ok();
            enc.i64(port.unwrap_or(3001) as i64).ok();
            enc.bytes(dns_name.as_bytes()).ok();
        }
        RelaySnapshot::MultiHostName { dns_name } => {
            // DNS: [0, port=3001, domain_bytes]
            enc.array(3).ok();
            enc.u32(0).ok();
            enc.i64(3001).ok();
            enc.bytes(dns_name.as_bytes()).ok();
        }
    }
}

/// Encode a relay in the standard PoolParams relay encoding.
///
/// This is distinct from `encode_ledger_relay` which is used for
/// `LedgerPeerSnapshot` and uses a different byte layout for IP addresses.
fn encode_relay_cbor(enc: &mut minicbor::Encoder<&mut Vec<u8>>, relay: &RelaySnapshot) {
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

// ─── SnapShot encoding ────────────────────────────────────────────────────────

/// Encode a single Cardano `SnapShot` as `array(3)` per Haskell wire format.
///
/// SnapShot = array(3):
///   [0] stake_map       — `Map<Credential(29B), Lovelace>`
///   [1] delegation_map  — `Map<Credential(29B), pool_id(28B)>`
///   [2] pool_params_map — `Map<pool_id(28B), PoolParams(array(9))>`
///
/// Credential (29 bytes) = 1-byte type prefix (0x00=KeyHash, 0x01=ScriptHash)
/// followed by 28 bytes of the hash.
///
/// cncli reads these maps to compute the leader schedule for a pool operator.
pub(crate) fn encode_snap_shot(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    snap: &SnapshotStakeData,
) {
    enc.array(3).ok();

    // [0] stake_map: Map<Credential(29B), Lovelace>
    enc.map(snap.stake_entries.len() as u64).ok();
    for (cred_type, cred_hash, lovelace) in &snap.stake_entries {
        // Credential key: 1-byte type prefix || 28-byte hash
        let mut key = Vec::with_capacity(29);
        key.push(*cred_type);
        key.extend_from_slice(cred_hash);
        enc.bytes(&key).ok();
        enc.u64(*lovelace).ok();
    }

    // [1] delegation_map: Map<Credential(29B), pool_id(28B)>
    enc.map(snap.delegation_entries.len() as u64).ok();
    for (cred_type, cred_hash, pool_id) in &snap.delegation_entries {
        let mut key = Vec::with_capacity(29);
        key.push(*cred_type);
        key.extend_from_slice(cred_hash);
        enc.bytes(&key).ok();
        enc.bytes(pool_id).ok();
    }

    // [2] pool_params_map: Map<pool_id(28B), PoolParams>
    encode_pool_params_map(enc, &snap.pool_params);
}

// ─── Debug query encoding ─────────────────────────────────────────────────────

/// Encode `DebugEpochState` (tag 8) as the Haskell `EpochState` CBOR structure.
///
/// Haskell `EpochState` is a 4-element positional record:
/// ```text
/// array(4) [
///   ChainAccountState,   -- array(2) [treasury, reserves]
///   LedgerState,         -- simplified placeholder (CBOR-skippable)
///   SnapShots,           -- array(4) [mark, set, go, fee]
///   NonMyopic,           -- array(2) [likelihoods_map, reward_pot_coin]
/// ]
/// ```
///
/// References:
///   `cardano-ledger / eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs`
///   `encCBOR (EpochState acnt ls ss nm) = ... encodeListLen 4 <> ...`
fn encode_debug_epoch_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    treasury: u64,
    reserves: u64,
    snap_mark: &SnapshotStakeData,
    snap_set: &SnapshotStakeData,
    snap_go: &SnapshotStakeData,
    snap_fee: u64,
) {
    // EpochState = array(4) [AccountState, LedgerState, SnapShots, NonMyopic]
    enc.array(4).ok();

    // [0] ChainAccountState = array(2) [treasury, reserves]
    enc.array(2).ok();
    enc.u64(treasury).ok();
    enc.u64(reserves).ok();

    // [1] LedgerState — simplified CBOR-skippable placeholder.
    //
    // In Conway, `LedgerState = array(2) [UTxOState, CertState]`.
    // We emit a minimal but structurally valid representation so that a
    // strict CBOR parser can decode past it to reach SnapShots at [2].
    //
    // UTxOState = array(5) [utxo_map, deposited, fees, gov_state, donation]
    // CertState = array(3) [VState, PState, DState]
    //
    // Haskell references:
    //   `Cardano.Ledger.Shelley.LedgerState.UTxOState` (encodeListLen 5)
    //   `Cardano.Ledger.Shelley.LedgerState.CertState` (encodeListLen 3)
    enc.array(2).ok();
    // UTxOState: array(5) with all-zero / empty contents
    enc.array(5).ok();
    enc.map(0).ok(); // empty UTxO map
    enc.u64(0).ok(); // deposited lovelace = 0
    enc.u64(0).ok(); // fees = 0
                     // GovState placeholder: ConwayGovState = array(7) — emit array(0) as a
                     // skippable marker; parsers that only read LedgerState[1] (CertState) skip
                     // this via decodeSkip before reaching CertState.
    enc.array(0).ok();
    enc.u64(0).ok(); // donation = 0
                     // CertState: array(3) [VState, PState, DState] — all empty
    enc.array(3).ok();
    enc.array(0).ok(); // VState placeholder
    enc.array(0).ok(); // PState placeholder
    enc.array(0).ok(); // DState placeholder

    // [2] SnapShots = array(4) [mark, set, go, fee]
    enc.array(4).ok();
    encode_snap_shot(enc, snap_mark);
    encode_snap_shot(enc, snap_set);
    encode_snap_shot(enc, snap_go);
    enc.u64(snap_fee).ok();

    // [3] NonMyopic = array(2) [likelihoods_map, reward_pot_coin]
    //
    // `NonMyopic` stores per-pool likelihood histories used for non-myopic
    // pool ranking.  We emit an empty likelihoods map and a zero reward pot.
    // Reference: `Cardano.Ledger.Shelley.PoolRank` (encodeListLen 2)
    enc.array(2).ok();
    enc.map(0).ok(); // empty likelihoods map
    enc.u64(0).ok(); // reward pot coin = 0
}

#[allow(clippy::too_many_arguments)]
fn encode_debug_new_epoch_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    epoch: u64,
    blocks_made_prev: &[(Vec<u8>, u64)],
    blocks_made_cur: &[(Vec<u8>, u64)],
    treasury: u64,
    reserves: u64,
    snap_mark: &SnapshotStakeData,
    snap_set: &SnapshotStakeData,
    snap_go: &SnapshotStakeData,
    snap_fee: u64,
    total_active_stake: u64,
    pool_distr: &[crate::node::n2c_query::types::StakePoolSnapshot],
) {
    // Full Haskell-compatible NewEpochState (array(7)):
    //
    //   [0] EpochNo
    //   [1] BlocksMade (prev epoch) — Map<pool_id_28B, u64>
    //   [2] BlocksMade (cur  epoch) — Map<pool_id_28B, u64>
    //   [3] EpochState — array(4) [AccountState, LedgerState, SnapShots, NonMyopic]
    //   [4] StrictMaybe RewardUpdate — array(0) for Nothing
    //   [5] PoolDistr — Map<pool_id_28B, IndividualPoolStake>
    //   [6] Extra — array(0) (Conway-era field, empty)
    //
    // cncli reads [3][2] (SnapShots) to extract the per-credential
    // stake distribution for leader-schedule computation.
    enc.array(7).ok();

    // [0] EpochNo
    enc.u64(epoch).ok();

    // [1] BlocksMade previous epoch: Map<pool_id(28B), u64>
    enc.map(blocks_made_prev.len() as u64).ok();
    for (pool_id, count) in blocks_made_prev {
        enc.bytes(pool_id).ok();
        enc.u64(*count).ok();
    }

    // [2] BlocksMade current epoch: Map<pool_id(28B), u64>
    enc.map(blocks_made_cur.len() as u64).ok();
    for (pool_id, count) in blocks_made_cur {
        enc.bytes(pool_id).ok();
        enc.u64(*count).ok();
    }

    // [3] EpochState = array(4)
    enc.array(4).ok();

    // [3][0] AccountState = array(2) [treasury, reserves]
    enc.array(2).ok();
    enc.u64(treasury).ok();
    enc.u64(reserves).ok();

    // [3][1] LedgerState — simplified empty placeholder.
    // cncli does not parse this field; it only inspects [3][2].
    // We encode a minimal valid array(2) [UTxOState, CertState] with
    // empty contents so that a CBOR parser can skip past it.
    enc.array(2).ok();
    // UTxOState = array(5): utxo_map, deposited, fees, gov_state, donation
    enc.array(5).ok();
    enc.map(0).ok(); // empty UTxO map
    enc.u64(0).ok(); // deposited = 0
    enc.u64(0).ok(); // fees = 0
                     // GovState placeholder (array(0))
    enc.array(0).ok();
    enc.u64(0).ok(); // donation = 0
                     // CertState = array(3): VState, PState, DState (all empty)
    enc.array(3).ok();
    enc.array(0).ok(); // VState placeholder
    enc.array(0).ok(); // PState placeholder
    enc.array(0).ok(); // DState placeholder

    // [3][2] SnapShots = array(4) [mark, set, go, fee]
    enc.array(4).ok();
    encode_snap_shot(enc, snap_mark);
    encode_snap_shot(enc, snap_set);
    encode_snap_shot(enc, snap_go);
    enc.u64(snap_fee).ok();

    // [3][3] NonMyopic = array(2) [likelihoods_map, reward_pot_coin]
    // cncli does not inspect this field, but we encode it correctly for
    // strict parsers.  Reference: Cardano.Ledger.Shelley.PoolRank (encodeListLen 2)
    enc.array(2).ok();
    enc.map(0).ok(); // empty likelihoods map
    enc.u64(0).ok(); // reward pot coin = 0

    // [4] StrictMaybe RewardUpdate = Nothing = array(0)
    enc.array(0).ok();

    // [5] PoolDistr: Map<pool_id(28B), IndividualPoolStake>
    // IndividualPoolStake = array(2) [tag(30)[num,den], vrf_hash(32B)]
    let total = total_active_stake.max(1);
    enc.map(pool_distr.len() as u64).ok();
    for pool in pool_distr {
        enc.bytes(&pool.pool_id).ok();
        enc.array(2).ok();
        encode_tagged_rational(enc, pool.stake, total);
        enc.bytes(&pool.vrf_keyhash).ok();
    }

    // [6] Extra = array(0)
    enc.array(0).ok();
}

/// Encode `DebugChainDepState` (tag 13) as the Haskell `PraosState` CBOR structure.
///
/// Haskell uses `encodeVersion 0` (from `Ouroboros.Consensus.Util.Versioned`),
/// which wraps any payload as `array(2) [version, payload]`.  The PraosState
/// payload is `array(7)` of the seven fields listed below.
///
/// Field layout (released `ouroboros-consensus-protocol-0.13.0.0`, shipped with
/// cardano-node 10.6.x / 10.7.x):
///   [0] praosStateLastSlot              — WithOrigin SlotNo
///   [1] praosStateOCertCounters         — Map<KeyHash BlockIssuer, Word64>
///   [2] praosStateEvolvingNonce         — Nonce
///   [3] praosStateCandidateNonce        — Nonce
///   [4] praosStateEpochNonce            — Nonce
///   [5] praosStateLabNonce              — Nonce
///   [6] praosStateLastEpochBlockNonce   — Nonce
///
/// NOTE: The unreleased main branch (commit 5598d9fb, 2025-10-29) inserts a
/// `praosStatePreviousEpochNonce` field at position [5] (shifting lab/lastEpoch
/// to [6]/[7]) and uses `array(8)`.  That change is NOT in any released
/// cardano-node as of 2026-04-06; encoding array(8) causes cardano-cli 10.15 to
/// reject the response with a CBOR enforceSize mismatch.
///
/// All nonce values use the Haskell `Nonce` encoding:
///   - `NeutralNonce` → `array(1) [0]`
///   - `Nonce hash`   → `array(2) [1, bytes32]`
///
/// Empty or all-zero slices are treated as `NeutralNonce`.
///
/// The `WithOrigin SlotNo` encoding:
///   - `Origin` → `array(1) [0]`
///   - `At slot` → `array(2) [1, slot_u64]`
///
/// References:
///   `ouroboros-consensus-protocol 0.13.0.0 / Ouroboros/Consensus/Protocol/Praos.hs`
///   `ouroboros-consensus / Ouroboros/Consensus/Util/Versioned.hs`
///   `cardano-ledger / libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs`
#[allow(clippy::too_many_arguments)]
fn encode_debug_chain_dep_state(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    last_slot: u64,
    last_slot_is_origin: bool,
    ocert_counters: &[(Vec<u8>, u64)],
    evolving_nonce: &[u8],
    candidate_nonce: &[u8],
    epoch_nonce: &[u8],
    lab_nonce: &[u8],
    last_epoch_block_nonce: &[u8],
) {
    // encodeVersion 0: array(2) [0, payload]
    enc.array(2).ok();
    enc.u8(0).ok(); // version number

    // PraosState: array(7) [lastSlot, ocertCounters, evolvingNonce,
    //   candidateNonce, epochNonce, labNonce, lastEpochBlockNonce]
    enc.array(7).ok();

    // [0] praosStateLastSlot: WithOrigin SlotNo
    // WithOrigin<T> via generic Serialise: Origin=[0], At slot=[1, slot]
    if last_slot_is_origin {
        enc.array(1).ok();
        enc.u8(0).ok();
    } else {
        enc.array(2).ok();
        enc.u8(1).ok();
        enc.u64(last_slot).ok();
    }

    // [1] praosStateOCertCounters: Map<KeyHash BlockIssuer, Word64>
    // KeyHash is a 28-byte hash; we use the raw bytes as map key.
    enc.map(ocert_counters.len() as u64).ok();
    for (pool_hash, counter) in ocert_counters {
        enc.bytes(pool_hash).ok();
        enc.u64(*counter).ok();
    }

    // Helper: encode a Cardano `Nonce` value.
    // NeutralNonce: empty or all-zero bytes → array(1)[0]
    // Nonce(hash):  any non-zero 32-byte slice → array(2)[1, bytes32]
    let encode_nonce = |enc: &mut minicbor::Encoder<&mut Vec<u8>>, nonce: &[u8]| {
        let is_neutral = nonce.is_empty() || nonce.iter().all(|&b| b == 0);
        if is_neutral {
            enc.array(1).ok();
            enc.u8(0).ok();
        } else {
            enc.array(2).ok();
            enc.u8(1).ok();
            enc.bytes(nonce).ok();
        }
    };

    // [2] praosStateEvolvingNonce
    encode_nonce(enc, evolving_nonce);
    // [3] praosStateCandidateNonce
    encode_nonce(enc, candidate_nonce);
    // [4] praosStateEpochNonce
    encode_nonce(enc, epoch_nonce);
    // [5] praosStateLabNonce
    encode_nonce(enc, lab_nonce);
    // [6] praosStateLastEpochBlockNonce
    encode_nonce(enc, last_epoch_block_nonce);
}

fn encode_reward_info_pools(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pools: &[crate::node::n2c_query::types::PoolRewardInfo],
) {
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

// ─── Protocol / era history encoding ─────────────────────────────────────────

/// Parse an ISO-8601 UTC timestamp to `(year, dayOfYear, picosecondsOfDay)`.
///
/// Input format: `"2022-04-01T00:00:00Z"` or similar.
pub(crate) fn parse_utctime(s: &str) -> (u64, u64, u64) {
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

/// Encode `SystemStart` as `UTCTime`: `[year, day_of_year, pico_of_day]`
fn encode_system_start(enc: &mut minicbor::Encoder<&mut Vec<u8>>, time_str: &str) {
    let (year, day_of_year, picos) = parse_utctime(time_str);
    enc.array(3).ok();
    enc.u64(year).ok();
    enc.u64(day_of_year).ok();
    enc.u64(picos).ok();
}

/// Encode legacy Shelley PParams as `array(18)` (N2C V16-V20 legacy format).
/// Encode Shelley PParams in legacy format (V16-V20).
///
/// `array(18)` with ProtocolVersion as two flat integers at [14] and [15].
pub(crate) fn encode_shelley_pparams(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ShelleyPParamsSnapshot,
) {
    encode_shelley_pparams_common(enc, pp, false);
}

/// Encode Shelley PParams in new format (V21+).
///
/// `array(17)` with ProtocolVersion as `array(2) [major, minor]` at [14].
pub(crate) fn encode_shelley_pparams_v21(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ShelleyPParamsSnapshot,
) {
    encode_shelley_pparams_common(enc, pp, true);
}

/// Shared Shelley PParams encoding. When `v21_protver` is true, uses
/// `array(17)` with bundled ProtocolVersion; otherwise `array(18)` with
/// flat major/minor fields per the legacy encoding.
fn encode_shelley_pparams_common(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ShelleyPParamsSnapshot,
    v21_protver: bool,
) {
    enc.array(if v21_protver { 17 } else { 18 }).ok();
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
    if v21_protver {
        // V21+: ProtocolVersion = array(2) [major, minor] at [14]
        enc.array(2).ok();
        enc.u64(pp.protocol_version_major).ok();
        enc.u64(pp.protocol_version_minor).ok();
    } else {
        // V16-V20: ProtocolVersion as two flat integers at [14] and [15]
        enc.u64(pp.protocol_version_major).ok();
        enc.u64(pp.protocol_version_minor).ok();
    }
    // [15/16] minUTxOValue
    enc.u64(pp.min_utxo_value).ok();
    // [16/17] minPoolCost
    enc.u64(pp.min_pool_cost).ok();
}

/// Encode a picosecond timestamp as a CBOR integer.
///
/// Values that fit in u64 are encoded as a normal CBOR unsigned integer.
/// Larger values (e.g., mainnet Byron end time ~9e19) are encoded as a CBOR
/// positive bignum (tag 2 + big-endian byte string), matching Haskell's
/// Serialise instance for `Fixed E12` (Pico).
fn encode_pico(enc: &mut minicbor::Encoder<&mut Vec<u8>>, value: u128) {
    if value <= u64::MAX as u128 {
        enc.u64(value as u64).ok();
    } else {
        // CBOR tag 2 = positive bignum, followed by big-endian bytes.
        enc.tag(minicbor::data::Tag::new(2)).ok();
        let bytes = value.to_be_bytes();
        // Strip leading zero bytes.
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
        enc.bytes(&bytes[start..]).ok();
    }
}

fn encode_era_history(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    summaries: &[crate::node::n2c_query::types::EraSummary],
) {
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
        encode_pico(enc, summary.start_time_pico);
        enc.u64(summary.start_slot).ok();
        enc.u64(summary.start_epoch).ok();
        // Era end: EraEnd(bound) = Bound directly, EraUnbounded = null
        if let Some(end) = &summary.end {
            enc.array(3).ok();
            encode_pico(enc, end.time_pico);
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

fn encode_genesis_config(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    gc: &crate::node::n2c_query::types::GenesisConfigSnapshot,
    n2c_version: u16,
) {
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

    // [11] protocolParams: version-gated encoding
    // V16-V20: array(18) with flat ProtocolVersion at [14] and [15]
    // V21+: array(17) with ProtocolVersion as array(2) [major, minor] at [14]
    if n2c_version >= 21 {
        encode_shelley_pparams_v21(enc, &gc.protocol_params);
    } else {
        encode_shelley_pparams(enc, &gc.protocol_params);
    }

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

// ─── Ledger peer snapshot encoding ───────────────────────────────────────────

fn encode_ledger_peer_snapshot(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    peers: &[crate::node::n2c_query::types::LedgerPeerEntry],
) {
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

// ─── Hash-size regression tests ───────────────────────────────────────────────
//
// These tests verify that all credential/pool-ID/DRep hashes are encoded as
// exactly 28 bytes in N2C query responses, matching the Cardano wire format.
//
// Background: internally the ledger stores Blake2b-224 (28-byte) hashes as
// Hash32 (zero-padded to 32 bytes) for use as uniform HashMap keys.  When
// building N2C responses these must be truncated back to 28 bytes.  Sending
// 32-byte hashes causes cardano-cli to reject with "hash bytes wrong size".
//
// See GitHub issue #97.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::n2c_query::types::{
        CommitteeMemberSnapshot, CommitteeSnapshot, DRepDelegationEntry, DRepSnapshot,
        DRepStakeEntry, PoolParamsSnapshot, PoolStakeSnapshotEntry, StakeAddressSnapshot,
        StakeDelegDepositEntry, StakePoolSnapshot, StakeSnapshotsResult, VoteDelegateeEntry,
    };
    use minicbor::Decoder;

    // ── Helper: strip the MsgResult [4, [result]] wrappers from a full
    //    encode_query_result() output and return just the inner payload. ──────
    fn strip_wrappers(cbor: &[u8]) -> Vec<u8> {
        let mut dec = Decoder::new(cbor);
        // [4, [payload]]
        dec.array().unwrap();
        dec.u32().unwrap(); // 4 = MsgResult tag
        dec.array().unwrap(); // HFC EitherMismatch Right wrapper
        cbor[dec.position()..].to_vec()
    }

    // ─── Stake distribution (tags 5, 10, 30) — pool IDs must be 28 bytes ────

    #[test]
    fn test_stake_distribution_pool_id_is_28_bytes() {
        // Build a query result with a pool ID stored as 28 bytes (normal path).
        let result = QueryResult::StakeDistribution(vec![StakePoolSnapshot {
            pool_id: vec![0xAB; 28],
            stake: 1_000_000,
            vrf_keyhash: vec![0u8; 32],
            total_active_stake: 1_000_000,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { pool_id_bytes => array(2)[rational, vrf_hash] }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap(); // map header
        let pool_id_bytes = dec.bytes().unwrap();
        assert_eq!(
            pool_id_bytes.len(),
            28,
            "StakeDistribution pool_id must be 28 bytes, got {}",
            pool_id_bytes.len()
        );
    }

    #[test]
    fn test_pool_distr_pool_id_is_28_bytes() {
        let result = QueryResult::PoolDistr(vec![StakePoolSnapshot {
            pool_id: vec![0xCD; 28],
            stake: 500_000,
            vrf_keyhash: vec![0u8; 32],
            total_active_stake: 1_000_000,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        let pool_id_bytes = dec.bytes().unwrap();
        assert_eq!(
            pool_id_bytes.len(),
            28,
            "PoolDistr pool_id must be 28 bytes, got {}",
            pool_id_bytes.len()
        );
    }

    // ─── DRep state (tag 25) — credential hashes must be 28 bytes ───────────

    #[test]
    fn test_drep_state_credential_hash_is_28_bytes() {
        let result = QueryResult::DRepState(vec![DRepSnapshot {
            credential_hash: vec![0x11; 28],
            credential_type: 0,
            deposit: 500_000_000,
            anchor_url: None,
            anchor_hash: None,
            expiry_epoch: 200,
            delegator_hashes: vec![],
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { [cred_type, cred_hash_bytes] => DRepState }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap(); // map header
        dec.array().unwrap(); // Credential = array(2)
        dec.u8().unwrap(); // cred type
        let cred_hash_bytes = dec.bytes().unwrap();
        assert_eq!(
            cred_hash_bytes.len(),
            28,
            "DRepState credential_hash must be 28 bytes, got {}",
            cred_hash_bytes.len()
        );
    }

    #[test]
    fn test_drep_state_delegator_hash_is_28_bytes() {
        // A DRep with one delegator. The delegator credential hash must also be 28 bytes.
        let result = QueryResult::DRepState(vec![DRepSnapshot {
            credential_hash: vec![0x22; 28],
            credential_type: 0,
            deposit: 500_000_000,
            anchor_url: None,
            anchor_hash: None,
            expiry_epoch: 200,
            delegator_hashes: vec![vec![0x33; 28]],
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Skip past the outer map key (credential)
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        // Key: array(2) [type, hash]
        dec.array().unwrap();
        dec.u8().unwrap();
        dec.bytes().unwrap();
        // Value: DRepState array(4) [expiry, anchor, deposit, delegators_set]
        dec.array().unwrap();
        dec.u64().unwrap(); // expiry
        dec.array().unwrap(); // anchor SNothing = array(0)
        dec.u64().unwrap(); // deposit
        dec.tag().unwrap(); // tag(258) set
        dec.array().unwrap(); // array(1)
                              // Delegator: array(2) [type, hash]
        dec.array().unwrap();
        dec.u8().unwrap();
        let delegator_hash = dec.bytes().unwrap();
        assert_eq!(
            delegator_hash.len(),
            28,
            "DRep delegator credential hash must be 28 bytes, got {}",
            delegator_hash.len()
        );
    }

    // ─── Committee state (tag 27) — cold/hot credentials must be 28 bytes ───

    #[test]
    fn test_committee_state_cold_credential_is_28_bytes() {
        let result = QueryResult::CommitteeState(CommitteeSnapshot {
            members: vec![CommitteeMemberSnapshot {
                cold_credential: vec![0x44; 28],
                cold_credential_type: 0,
                hot_status: 0,
                hot_credential: Some(vec![0x55; 28]),
                hot_credential_type: 0,
                member_status: 0,
                expiry_epoch: Some(500),
            }],
            threshold: Some((2, 3)),
            current_epoch: 100,
        });
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: array(3) [member_map, maybe_threshold, epoch]
        let mut dec = Decoder::new(&inner);
        dec.array().unwrap(); // array(3)
        dec.map().unwrap(); // member_map(1)
                            // Key: Credential array(2) [type, cold_hash]
        dec.array().unwrap();
        dec.u8().unwrap();
        let cold_hash = dec.bytes().unwrap();
        assert_eq!(
            cold_hash.len(),
            28,
            "CommitteeState cold credential hash must be 28 bytes, got {}",
            cold_hash.len()
        );
    }

    #[test]
    fn test_committee_state_hot_credential_is_28_bytes() {
        let result = QueryResult::CommitteeState(CommitteeSnapshot {
            members: vec![CommitteeMemberSnapshot {
                cold_credential: vec![0x66; 28],
                cold_credential_type: 0,
                hot_status: 0, // Authorized
                hot_credential: Some(vec![0x77; 28]),
                hot_credential_type: 0,
                member_status: 0,
                expiry_epoch: Some(500),
            }],
            threshold: Some((2, 3)),
            current_epoch: 100,
        });
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        dec.array().unwrap(); // array(3)
        dec.map().unwrap(); // member_map(1)
                            // Skip map key (cold cred)
        dec.array().unwrap(); // Credential array(2)
        dec.u8().unwrap();
        dec.bytes().unwrap(); // cold hash

        // Value: CommitteeMemberState array(4)
        dec.array().unwrap();
        // [0] HotCredAuthStatus: MemberAuthorized = array(2) [0, credential]
        dec.array().unwrap(); // array(2)
        dec.u32().unwrap(); // 0 = Authorized
                            // Inner credential: array(2) [type, hot_hash]
        dec.array().unwrap();
        dec.u8().unwrap();
        let hot_hash = dec.bytes().unwrap();
        assert_eq!(
            hot_hash.len(),
            28,
            "CommitteeState hot credential hash must be 28 bytes, got {}",
            hot_hash.len()
        );
    }

    // ─── Stake address info (tag 10) — credential hashes must be 28 bytes ───

    #[test]
    fn test_stake_address_info_credential_is_28_bytes() {
        let result = QueryResult::StakeAddressInfo(vec![StakeAddressSnapshot {
            credential_hash: vec![0x88; 28],
            delegated_pool: Some(vec![0x99; 28]),
            reward_balance: 1_000_000,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: array(2) [delegations_map, rewards_map]
        let mut dec = Decoder::new(&inner);
        dec.array().unwrap(); // array(2)
        dec.map().unwrap(); // delegations_map(1)
                            // Key: Credential array(2) [type, hash]
        dec.array().unwrap();
        dec.u32().unwrap(); // 0 = KeyHashObj
        let cred_hash = dec.bytes().unwrap();
        assert_eq!(
            cred_hash.len(),
            28,
            "StakeAddressInfo credential hash in delegations_map must be 28 bytes, got {}",
            cred_hash.len()
        );
    }

    // ─── Stake deleg deposits (tag 22) — credential hashes must be 28 bytes ─

    #[test]
    fn test_stake_deleg_deposits_credential_is_28_bytes() {
        let result = QueryResult::StakeDelegDeposits(vec![StakeDelegDepositEntry {
            credential_hash: vec![0xAA; 28],
            credential_type: 0,
            deposit: 2_000_000,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { array(2)[type, hash] => deposit }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        dec.array().unwrap(); // Credential array(2)
        dec.u8().unwrap();
        let cred_hash = dec.bytes().unwrap();
        assert_eq!(
            cred_hash.len(),
            28,
            "StakeDelegDeposits credential hash must be 28 bytes, got {}",
            cred_hash.len()
        );
    }

    // ─── DRep stake distribution (tag 26) — DRep hashes must be 28 bytes ────

    #[test]
    fn test_drep_stake_distr_keyhash_is_28_bytes() {
        let result = QueryResult::DRepStakeDistr(vec![DRepStakeEntry {
            drep_type: 0, // KeyHash
            drep_hash: Some(vec![0xBB; 28]),
            stake: 1_000_000,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { DRep => stake }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        // DRep: array(2) [0, hash]
        dec.array().unwrap();
        dec.u8().unwrap(); // 0 = KeyHash
        let drep_hash = dec.bytes().unwrap();
        assert_eq!(
            drep_hash.len(),
            28,
            "DRepStakeDistr KeyHash DRep hash must be 28 bytes, got {}",
            drep_hash.len()
        );
    }

    // ─── Filtered vote delegatees (tag 28) — credential hashes must be 28 B ─

    #[test]
    fn test_filtered_vote_delegatees_credential_is_28_bytes() {
        let result = QueryResult::FilteredVoteDelegatees(vec![VoteDelegateeEntry {
            credential_hash: vec![0xCC; 28],
            credential_type: 0,
            drep_type: 0, // KeyHash DRep
            drep_hash: Some(vec![0xDD; 28]),
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { Credential => DRep }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        // Key: Credential array(2) [type, hash]
        dec.array().unwrap();
        dec.u8().unwrap();
        let cred_hash = dec.bytes().unwrap();
        assert_eq!(
            cred_hash.len(),
            28,
            "FilteredVoteDelegatees stake credential hash must be 28 bytes, got {}",
            cred_hash.len()
        );
    }

    #[test]
    fn test_filtered_vote_delegatees_drep_hash_is_28_bytes() {
        let result = QueryResult::FilteredVoteDelegatees(vec![VoteDelegateeEntry {
            credential_hash: vec![0xEE; 28],
            credential_type: 0,
            drep_type: 0, // KeyHash DRep
            drep_hash: Some(vec![0xFF; 28]),
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        // Skip map key (credential)
        dec.array().unwrap(); // Credential array(2)
        dec.u8().unwrap();
        dec.bytes().unwrap();
        // Value: DRep array(2) [type, hash]
        dec.array().unwrap();
        dec.u8().unwrap(); // 0 = KeyHash
        let drep_hash = dec.bytes().unwrap();
        assert_eq!(
            drep_hash.len(),
            28,
            "FilteredVoteDelegatees DRep KeyHash must be 28 bytes, got {}",
            drep_hash.len()
        );
    }

    // ─── Stake pools set (tag 16) — pool IDs must be 28 bytes ───────────────

    #[test]
    fn test_stake_pools_set_pool_id_is_28_bytes() {
        let result = QueryResult::StakePools(vec![vec![0x12; 28]]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: tag(258) array(1) [pool_id_bytes]
        let mut dec = Decoder::new(&inner);
        dec.tag().unwrap(); // tag(258)
        dec.array().unwrap();
        let pool_id = dec.bytes().unwrap();
        assert_eq!(
            pool_id.len(),
            28,
            "StakePools pool ID must be 28 bytes, got {}",
            pool_id.len()
        );
    }

    // ─── Pool params (tag 17/19) — pool IDs and owner hashes must be 28 B ───

    #[test]
    fn test_pool_params_pool_id_is_28_bytes() {
        let result = QueryResult::PoolParams(vec![PoolParamsSnapshot {
            pool_id: vec![0x34; 28],
            vrf_keyhash: vec![0u8; 32],
            pledge: 100_000_000,
            cost: 340_000_000,
            margin_num: 5,
            margin_den: 100,
            reward_account: vec![0u8; 29],
            owners: vec![vec![0x56; 28]],
            relays: vec![],
            metadata_url: None,
            metadata_hash: None,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: map(1) { pool_id_bytes => PoolParams(array(9)) }
        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();
        let pool_id = dec.bytes().unwrap();
        assert_eq!(
            pool_id.len(),
            28,
            "PoolParams map key (pool_id) must be 28 bytes, got {}",
            pool_id.len()
        );

        // PoolParams array(9): [operator, vrf, pledge, cost, margin, reward_acct, owners, relays, metadata]
        dec.array().unwrap(); // array(9)
        let operator = dec.bytes().unwrap(); // operator = pool_id again
        assert_eq!(
            operator.len(),
            28,
            "PoolParams operator field must be 28 bytes, got {}",
            operator.len()
        );
        dec.bytes().unwrap(); // vrf_keyhash (32 bytes — genuine hash)
        dec.u64().unwrap(); // pledge
        dec.u64().unwrap(); // cost
        dec.tag().unwrap(); // tag(30) rational for margin
        dec.array().unwrap();
        dec.u64().unwrap();
        dec.u64().unwrap();
        dec.bytes().unwrap(); // reward_account
        dec.tag().unwrap(); // tag(258) owners set
        dec.array().unwrap(); // array(1)
        let owner_hash = dec.bytes().unwrap();
        assert_eq!(
            owner_hash.len(),
            28,
            "PoolParams owner hash must be 28 bytes, got {}",
            owner_hash.len()
        );
    }

    // ─── PoolDistr2 (tags 36/37) — pool IDs must be 28 bytes ───────────────

    #[test]
    fn test_pool_distr2_pool_id_is_28_bytes() {
        let result = QueryResult::PoolDistr2 {
            pools: vec![StakePoolSnapshot {
                pool_id: vec![0x78; 28],
                stake: 1_000_000,
                vrf_keyhash: vec![0u8; 32],
                total_active_stake: 2_000_000,
            }],
            total_active_stake: 2_000_000,
        };
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: array(2) [pool_map, total_active_stake]
        let mut dec = Decoder::new(&inner);
        dec.array().unwrap(); // array(2)
        dec.map().unwrap(); // pool_map(1)
        let pool_id = dec.bytes().unwrap();
        assert_eq!(
            pool_id.len(),
            28,
            "PoolDistr2 pool_id must be 28 bytes, got {}",
            pool_id.len()
        );
    }

    // ─── Stake snapshots (tag 20) — pool IDs must be 28 bytes ───────────────

    #[test]
    fn test_stake_snapshots_pool_id_is_28_bytes() {
        let result = QueryResult::StakeSnapshots(StakeSnapshotsResult {
            pools: vec![PoolStakeSnapshotEntry {
                pool_id: vec![0x9A; 28],
                mark_stake: 100,
                set_stake: 200,
                go_stake: 300,
            }],
            total_mark_stake: 100,
            total_set_stake: 200,
            total_go_stake: 300,
        });
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: array(4) [pool_map, mark_total, set_total, go_total]
        let mut dec = Decoder::new(&inner);
        dec.array().unwrap();
        dec.map().unwrap(); // pool_map(1)
        let pool_id = dec.bytes().unwrap();
        assert_eq!(
            pool_id.len(),
            28,
            "StakeSnapshots pool_id must be 28 bytes, got {}",
            pool_id.len()
        );
    }

    // ─── Default vote (tag 35) — bare word8 encoding ───────────────────────

    #[test]
    fn test_stake_pool_default_vote_bare_word8() {
        let result = QueryResult::StakePoolDefaultVote(1); // DefaultAbstain
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Inner: bare word8 (0=DefaultNo, 1=DefaultAbstain, 2=DefaultNoConfidence)
        let mut dec = Decoder::new(&inner);
        assert_eq!(dec.u8().unwrap(), 1);
    }

    // ─── SPO stake distribution (tag 30) — Map<pool_hash, Coin> ─────────

    #[test]
    fn test_spo_stake_distr_map_encoding() {
        let result = QueryResult::SPOStakeDistr(vec![
            (vec![0x33; 28], 1_000_000),
            (vec![0x44; 28], 2_000_000),
        ]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 2);
        // First entry
        assert_eq!(dec.bytes().unwrap(), &[0x33; 28]);
        assert_eq!(dec.u64().unwrap(), 1_000_000);
        // Second entry
        assert_eq!(dec.bytes().unwrap(), &[0x44; 28]);
        assert_eq!(dec.u64().unwrap(), 2_000_000);
    }

    // ─── DRep delegations (tag 39, V23+) — Map<Credential, DRep> ────────

    /// GetDRepDelegations with a KeyHash credential delegating to a KeyHash DRep.
    /// Verifies credential and DRep hashes are 28 bytes on the wire, and that the
    /// overall CBOR structure is `map { array(2)[type, hash] => array(2)[type, hash] }`.
    #[test]
    fn test_drep_delegations_keyhash_credential_and_drep() {
        let result = QueryResult::DRepDelegations(vec![DRepDelegationEntry {
            credential_hash: vec![0xAA; 28],
            credential_type: 0, // KeyHash
            drep_type: 0,       // KeyHash DRep
            drep_hash: Some(vec![0xBB; 28]),
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        // Map(1) { Credential => DRep }
        let mut dec = Decoder::new(&inner);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1, "Expected exactly one map entry");

        // Key: Credential = array(2) [0, hash(28)]
        let cred_len = dec.array().unwrap().unwrap();
        assert_eq!(cred_len, 2);
        assert_eq!(
            dec.u8().unwrap(),
            0,
            "Credential type should be 0 (KeyHash)"
        );
        let cred_hash = dec.bytes().unwrap();
        assert_eq!(cred_hash.len(), 28, "Credential hash must be 28 bytes");

        // Value: DRep = array(2) [0, hash(28)]
        let drep_len = dec.array().unwrap().unwrap();
        assert_eq!(drep_len, 2);
        assert_eq!(dec.u8().unwrap(), 0, "DRep type should be 0 (KeyHash)");
        let drep_hash = dec.bytes().unwrap();
        assert_eq!(drep_hash.len(), 28, "DRep hash must be 28 bytes");
    }

    /// GetDRepDelegations with an AlwaysAbstain DRep.
    /// Verifies AlwaysAbstain (type 2) encodes as `array(1) [2]` with no hash.
    #[test]
    fn test_drep_delegations_always_abstain() {
        let result = QueryResult::DRepDelegations(vec![DRepDelegationEntry {
            credential_hash: vec![0xCC; 28],
            credential_type: 0, // KeyHash
            drep_type: 2,       // AlwaysAbstain
            drep_hash: None,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();

        // Key: Credential
        dec.array().unwrap();
        dec.u8().unwrap(); // type
        dec.bytes().unwrap(); // hash

        // Value: DRep = array(1) [2]
        let drep_arr_len = dec.array().unwrap().unwrap();
        assert_eq!(
            drep_arr_len, 1,
            "AlwaysAbstain DRep should encode as array(1)"
        );
        assert_eq!(dec.u8().unwrap(), 2, "AlwaysAbstain DRep type should be 2");
    }

    /// GetDRepDelegations with an AlwaysNoConfidence DRep (type 3).
    #[test]
    fn test_drep_delegations_always_no_confidence() {
        let result = QueryResult::DRepDelegations(vec![DRepDelegationEntry {
            credential_hash: vec![0xDD; 28],
            credential_type: 1, // ScriptHash credential
            drep_type: 3,       // AlwaysNoConfidence
            drep_hash: None,
        }]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        dec.map().unwrap();

        // Key: Credential array(2) [1, hash(28)]
        dec.array().unwrap();
        assert_eq!(
            dec.u8().unwrap(),
            1,
            "Credential type should be 1 (ScriptHash)"
        );
        dec.bytes().unwrap();

        // Value: DRep = array(1) [3]
        let drep_arr_len = dec.array().unwrap().unwrap();
        assert_eq!(
            drep_arr_len, 1,
            "AlwaysNoConfidence DRep should encode as array(1)"
        );
        assert_eq!(
            dec.u8().unwrap(),
            3,
            "AlwaysNoConfidence DRep type should be 3"
        );
    }

    /// GetDRepDelegations with multiple entries covering different DRep types.
    #[test]
    fn test_drep_delegations_multi_entry_map_length() {
        let result = QueryResult::DRepDelegations(vec![
            DRepDelegationEntry {
                credential_hash: vec![0x11; 28],
                credential_type: 0,
                drep_type: 0,
                drep_hash: Some(vec![0x22; 28]),
            },
            DRepDelegationEntry {
                credential_hash: vec![0x33; 28],
                credential_type: 0,
                drep_type: 2, // AlwaysAbstain
                drep_hash: None,
            },
            DRepDelegationEntry {
                credential_hash: vec![0x44; 28],
                credential_type: 1, // ScriptHash
                drep_type: 1,       // ScriptHash DRep
                drep_hash: Some(vec![0x55; 28]),
            },
        ]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 3, "Three entries should produce map(3)");
    }

    /// GetDRepDelegations empty result encodes as empty map.
    #[test]
    fn test_drep_delegations_empty_is_empty_map() {
        let result = QueryResult::DRepDelegations(vec![]);
        let encoded = encode_query_result(&result);
        let inner = strip_wrappers(&encoded);

        let mut dec = Decoder::new(&inner);
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 0, "Empty DRepDelegations should encode as map(0)");
    }
}
