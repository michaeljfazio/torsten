//! LocalStateQuery mini-protocol implementation.
//!
//! Dispatches incoming N2C `MsgQuery` payloads to `QueryHandler` and serializes
//! the result back over the wire.  The public surface is intentionally narrow:
//!
//! - `handle_state_query` — async entry point called by the N2C connection loop
//! - `encode_query_result` — re-exported for use in tests and the CLI
//!
//! Sub-modules:
//!
//! | Module      | Responsibility |
//! |-------------|----------------|
//! | `encoding`  | `encode_query_result` + all CBOR helpers |
//! | `ledger`    | UTxO, stake, and pool query handlers |
//! | `governance`| DRep, committee, constitution, proposals |
//! | `protocol`  | PParams, epoch info, system start, era history |
//! | `debug`     | Debug epoch/chain state, GetCBOR (tag 9) |

mod debug;
mod encoding;
mod governance;
mod ledger;
mod protocol;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::multiplexer::Segment;
use crate::query_handler::QueryHandler;

use super::{N2CServerError, MINI_PROTOCOL_STATE_QUERY};

// Re-export the public encoder so callers in n2c/mod.rs and tests can use it.
pub use encoding::encode_query_result;

/// Handle LocalStateQuery messages.
///
/// Protocol flow:
/// ```text
/// Client: MsgAcquire(point) → Server: MsgAcquired
/// Client: MsgQuery(query)   → Server: MsgResult(result)
/// Client: MsgRelease        → (back to idle)
/// Client: MsgDone           → (end)
/// ```
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

// ── test-only helper (used by golden tests in torsten-network) ─────────────────

/// Test-only access to `parse_utctime`.
#[cfg(test)]
pub(crate) fn parse_utctime_for_test(s: &str) -> (u64, u64, u64) {
    encoding::parse_utctime(s)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::encoding::{encode_protocol_params_cbor, encode_query_result, encode_tagged_rational};
    use crate::query_handler::*;

    /// Helper: encode a QueryResult and return the raw CBOR bytes.
    fn encode(result: &QueryResult) -> Vec<u8> {
        encode_query_result(result)
    }

    /// Helper: decode and verify the MsgResult `[4, ...]` envelope.
    /// Returns the decoder positioned after the envelope.
    fn decode_msg_result(buf: &[u8]) -> minicbor::Decoder<'_> {
        let mut dec = minicbor::Decoder::new(buf);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 2, "MsgResult outer array must be 2");
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 4, "MsgResult tag must be 4");
        dec
    }

    /// Helper: strip HFC EitherMismatch Right wrapper `array(1)`.
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
        let _ = dec.array().unwrap();
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

    /// Verify tagged rational encoding: `tag(30)[num, den]`
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
        let buf = encode(&QueryResult::EraHistory(vec![EraSummary {
            start_slot: 0,
            start_epoch: 0,
            start_time_pico: 0,
            end: None,
            slot_length_ms: 20_000,
            epoch_size: 4320,
            safe_zone: 4320,
            genesis_window: 36000,
        }]));
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

    /// Golden test: tagged rational `tag(30)[n, d]` produces exact bytes.
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

    /// Golden test: full `QueryResult::ProtocolParams` envelope (MsgResult + HFC wrapper).
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
            EraSummary {
                start_slot: 0,
                start_epoch: 0,
                start_time_pico: 0,
                end: Some(EraBound {
                    time_pico: 1_000_000_000_000,
                    slot: 100,
                    epoch: 10,
                }),
                slot_length_ms: 20_000,
                epoch_size: 100,
                safe_zone: 200,
                genesis_window: 36000,
            },
            EraSummary {
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
