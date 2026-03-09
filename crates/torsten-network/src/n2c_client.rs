use std::path::Path;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::debug;

use crate::multiplexer::Segment;

#[derive(Error, Debug)]
pub enum N2CClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Handshake rejected")]
    HandshakeRejected,
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Timeout")]
    Timeout,
}

/// N2C mini-protocol IDs
const MINI_PROTOCOL_HANDSHAKE: u16 = 0;
const MINI_PROTOCOL_TX_SUBMISSION: u16 = 6;
const MINI_PROTOCOL_STATE_QUERY: u16 = 7;
const MINI_PROTOCOL_TX_MONITOR: u16 = 12;

/// Node-to-Client client for connecting to a Cardano node via Unix socket.
pub struct N2CClient {
    stream: UnixStream,
}

/// Result of a tip query
#[derive(Debug, Clone)]
pub struct TipResult {
    pub slot: u64,
    pub hash: Vec<u8>,
    pub block_no: u64,
    pub epoch: u64,
    pub era: u32,
}

/// Result of a generic query
#[derive(Debug, Clone)]
pub enum LocalQueryResult {
    Tip(TipResult),
    EpochNo(u64),
    Era(u32),
    SystemStart(String),
    BlockNo(u64),
    Raw(Vec<u8>),
    Error(String),
}

impl N2CClient {
    /// Connect to a node's Unix domain socket
    pub async fn connect(socket_path: &Path) -> Result<Self, N2CClientError> {
        let stream = UnixStream::connect(socket_path).await.map_err(|e| {
            N2CClientError::ConnectionFailed(format!(
                "Cannot connect to {}: {e}",
                socket_path.display()
            ))
        })?;
        debug!("Connected to N2C socket: {}", socket_path.display());
        Ok(N2CClient { stream })
    }

    /// Perform the N2C handshake
    pub async fn handshake(&mut self, network_magic: u64) -> Result<(), N2CClientError> {
        // Build handshake proposal: [0, { version: [magic, false] }]
        // Propose versions 14-17 (N2C versions for recent eras)
        let mut proposal = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut proposal);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgProposeVersions
        enc.map(4)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        for version in [14u32, 15, 16, 17] {
            enc.u32(version)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.array(2)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.u64(network_magic)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.bool(false)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        }

        // Wrap in multiplexer segment
        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_HANDSHAKE,
            is_responder: false,
            payload: proposal,
        };
        self.send_segment(&segment).await?;

        // Read response
        let resp = self.recv_segment().await?;
        if resp.protocol_id != MINI_PROTOCOL_HANDSHAKE {
            return Err(N2CClientError::Protocol(format!(
                "Expected handshake response, got protocol {}",
                resp.protocol_id
            )));
        }

        // Parse response: [1, version, params] = accept, [2, ...] = refuse
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let msg_tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad handshake response: {e}")))?;

        match msg_tag {
            1 => {
                let version = decoder.u32().unwrap_or(0);
                debug!("N2C handshake accepted, version {version}");
                Ok(())
            }
            2 => Err(N2CClientError::HandshakeRejected),
            _ => Err(N2CClientError::Protocol(format!(
                "unexpected handshake msg tag: {msg_tag}"
            ))),
        }
    }

    /// Acquire the ledger state at the current tip
    pub async fn acquire(&mut self) -> Result<(), N2CClientError> {
        // MsgAcquire: [0, point]
        // For "tip", we send [0, []] (acquire at tip)
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgAcquire

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        // Expect MsgAcquired [1]
        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad acquire response: {e}")))?;
        if tag != 1 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgAcquired(1), got {tag}"
            )));
        }
        debug!("State acquired");
        Ok(())
    }

    /// Release the acquired state
    pub async fn release(&mut self) -> Result<(), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(7)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgRelease

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;
        Ok(())
    }

    /// Send MsgDone to end the protocol
    pub async fn done(&mut self) -> Result<(), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(9)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgDone

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;
        Ok(())
    }

    /// Query the chain tip (GetChainPoint - Shelley query tag 11)
    pub async fn query_tip(&mut self) -> Result<TipResult, N2CClientError> {
        let result = self.send_query(11).await?;
        parse_tip_result(&result)
    }

    /// Query the current epoch number (GetEpochNo - Shelley query tag 0)
    pub async fn query_epoch(&mut self) -> Result<u64, N2CClientError> {
        let result = self.send_query(0).await?;
        parse_epoch_result(&result)
    }

    /// Query the current era (GetCurrentEra - hardcoded query tag 0)
    pub async fn query_era(&mut self) -> Result<u32, N2CClientError> {
        // GetCurrentEra is a top-level query, not era-wrapped
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgQuery
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetCurrentEra

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder.u32().unwrap_or(999);
        if tag != 4 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgResult(4), got {tag}"
            )));
        }
        let era = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad era result: {e}")))?;
        Ok(era)
    }

    /// Query the chain block number (GetChainBlockNo - Shelley query tag 10)
    pub async fn query_block_no(&mut self) -> Result<u64, N2CClientError> {
        let result = self.send_query(10).await?;
        parse_u64_result(&result)
    }

    /// Query protocol parameters (GetCurrentPParams - Shelley query tag 7)
    pub async fn query_protocol_params(&mut self) -> Result<String, N2CClientError> {
        let result = self.send_query(7).await?;
        parse_protocol_params_cbor(&result)
    }

    /// Query stake distribution (GetStakeDistribution - Shelley query tag 5)
    pub async fn query_stake_distribution(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(5).await?;
        // Return raw CBOR for the CLI to parse
        Ok(result)
    }

    /// Query account state (GetAccountState - query tag 3)
    /// Returns treasury and reserves in lovelace
    pub async fn query_account_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(3).await?;
        Ok(result)
    }

    /// Query governance state (GetGovState - query tag 20)
    pub async fn query_gov_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(20).await?;
        Ok(result)
    }

    /// Query DRep state (GetDRepState - query tag 21)
    pub async fn query_drep_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(21).await?;
        Ok(result)
    }

    /// Query committee state (GetCommitteeState - query tag 22)
    pub async fn query_committee_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(22).await?;
        Ok(result)
    }

    /// Query stake address info (GetStakeAddressInfo - query tag 23)
    pub async fn query_stake_address_info(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(23).await?;
        Ok(result)
    }

    /// Query stake snapshots (GetStakeSnapshots - query tag 24)
    pub async fn query_stake_snapshot(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(24).await?;
        Ok(result)
    }

    /// Query stake pool params (GetStakePoolParams - query tag 12)
    pub async fn query_pool_params(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(12).await?;
        Ok(result)
    }

    /// Query UTxOs at a specific address (GetUTxOByAddress - query tag 4)
    pub async fn query_utxo_by_address(
        &mut self,
        addr_bytes: &[u8],
    ) -> Result<Vec<u8>, N2CClientError> {
        // Build MsgQuery with address parameter: [3, [era, [4, addr_bytes]]]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgQuery
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // era tag (Shelley+)
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(4)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetUTxOByAddress
        enc.bytes(addr_bytes)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        Ok(resp.payload)
    }

    /// Submit a transaction via LocalTxSubmission
    ///
    /// The tx_cbor should be the raw CBOR bytes of the signed transaction.
    /// Returns Ok(()) if accepted, Err with rejection reason if rejected.
    pub async fn submit_tx(&mut self, tx_cbor: &[u8]) -> Result<(), N2CClientError> {
        // Build MsgSubmitTx: [0, [era_id, tx_bytes]]
        // era_id 6 = Conway
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgSubmitTx
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(6)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // Conway era
        enc.bytes(tx_cbor)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        // Read response: MsgAcceptTx [1] or MsgRejectTx [2, reason]
        let resp = self.recv_segment().await?;
        if resp.protocol_id != MINI_PROTOCOL_TX_SUBMISSION {
            return Err(N2CClientError::Protocol(format!(
                "Expected tx submission response, got protocol {}",
                resp.protocol_id
            )));
        }

        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let msg_tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad tx response tag: {e}")))?;

        match msg_tag {
            1 => Ok(()), // MsgAcceptTx
            2 => {
                // MsgRejectTx - extract reason
                let reason = if let Ok(Some(_)) = decoder.array() {
                    decoder
                        .str()
                        .unwrap_or("unknown rejection reason")
                        .to_string()
                } else {
                    "transaction rejected".to_string()
                };
                Err(N2CClientError::Protocol(format!(
                    "Transaction rejected: {reason}"
                )))
            }
            other => Err(N2CClientError::Protocol(format!(
                "unexpected tx submission response tag: {other}"
            ))),
        }
    }

    /// Send MsgDone for the LocalTxSubmission protocol
    pub async fn tx_submission_done(&mut self) -> Result<(), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgDone

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;
        Ok(())
    }

    // --- LocalTxMonitor protocol methods ---

    /// Acquire a mempool snapshot. Returns the slot number.
    pub async fn monitor_acquire(&mut self) -> Result<u64, N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgAcquire

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        if tag != 1 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgAcquired(1), got {tag}"
            )));
        }
        let slot = decoder.u64().unwrap_or(0);
        Ok(slot)
    }

    /// Check if a transaction is in the mempool
    pub async fn monitor_has_tx(&mut self, tx_hash: &[u8; 32]) -> Result<bool, N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(4)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgHasTx
        enc.bytes(tx_hash)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        if tag != 5 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgHasTxReply(5), got {tag}"
            )));
        }
        let has_tx = decoder.bool().unwrap_or(false);
        Ok(has_tx)
    }

    /// Get mempool sizes (capacity, size, num_txs)
    pub async fn monitor_get_sizes(&mut self) -> Result<(u32, u32, u32), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(8)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgGetSizes

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        if tag != 9 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgGetSizesReply(9), got {tag}"
            )));
        }
        let _ = decoder.array();
        let capacity = decoder.u32().unwrap_or(0);
        let size = decoder.u32().unwrap_or(0);
        let num_txs = decoder.u32().unwrap_or(0);
        Ok((capacity, size, num_txs))
    }

    /// Get the next transaction from the mempool snapshot.
    /// Returns `Some((era_id, tx_bytes))` if a transaction is available, or `None` if empty.
    pub async fn monitor_next_tx(&mut self) -> Result<Option<(u32, Vec<u8>)>, N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(6)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgNextTx

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        if tag != 7 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgNextTxReply(7), got {tag}"
            )));
        }
        // Check if the response is null (no tx) or [era_id, tx_bytes]
        match decoder.datatype() {
            Ok(minicbor::data::Type::Null) => {
                let _ = decoder.null();
                Ok(None)
            }
            Ok(minicbor::data::Type::Array) => {
                let _ = decoder.array();
                let era_id = decoder.u32().unwrap_or(0);
                let tx_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                Ok(Some((era_id, tx_bytes)))
            }
            _ => Ok(None),
        }
    }

    /// Release the mempool snapshot
    pub async fn monitor_release(&mut self) -> Result<(), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgRelease

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;
        Ok(())
    }

    /// Send MsgDone for the LocalTxMonitor protocol
    pub async fn monitor_done(&mut self) -> Result<(), N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgDone

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_TX_MONITOR,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;
        Ok(())
    }

    /// Send a Shelley-era query and return the raw MsgResult payload
    async fn send_query(&mut self, shelley_tag: u32) -> Result<Vec<u8>, N2CClientError> {
        // Build MsgQuery: [3, [era, [shelley_tag]]]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgQuery
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // era tag (Shelley+)
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(shelley_tag)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        Ok(resp.payload)
    }

    /// Send a multiplexer segment
    async fn send_segment(&mut self, segment: &Segment) -> Result<(), N2CClientError> {
        let encoded = segment.encode();
        self.stream.write_all(&encoded).await?;
        Ok(())
    }

    /// Receive a multiplexer segment
    async fn recv_segment(&mut self) -> Result<Segment, N2CClientError> {
        let mut buf = vec![0u8; 65536];
        let mut offset = 0;

        loop {
            let n = self.stream.read(&mut buf[offset..]).await?;
            if n == 0 {
                return Err(N2CClientError::Protocol("connection closed".into()));
            }
            offset += n;

            // Try to decode a segment
            match Segment::decode(&buf[..offset]) {
                Ok((segment, _consumed)) => {
                    return Ok(segment);
                }
                Err(_) => {
                    if offset >= buf.len() {
                        return Err(N2CClientError::Protocol("response too large".into()));
                    }
                    continue; // Need more data
                }
            }
        }
    }
}

/// Parse a tip result from MsgResult CBOR
fn parse_tip_result(payload: &[u8]) -> Result<TipResult, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(N2CClientError::Protocol(format!(
            "expected MsgResult(4), got {tag}"
        )));
    }

    // Result is: [[ slot, hash ], block_no]
    let _ = decoder.array();
    let _ = decoder.array();
    let slot = decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad slot: {e}")))?;
    let hash = decoder
        .bytes()
        .map_err(|e| N2CClientError::Protocol(format!("bad hash: {e}")))?
        .to_vec();
    let block_no = decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad block_no: {e}")))?;

    Ok(TipResult {
        slot,
        hash,
        block_no,
        epoch: 0, // Will be filled in by caller
        era: 0,
    })
}

/// Parse an epoch number from MsgResult CBOR
fn parse_epoch_result(payload: &[u8]) -> Result<u64, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(N2CClientError::Protocol(format!(
            "expected MsgResult(4), got {tag}"
        )));
    }
    decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad epoch: {e}")))
}

/// Parse protocol params CBOR map into JSON string.
///
/// Decodes the Cardano protocol params CBOR map (integer keys 0-33)
/// and produces a cardano-cli compatible JSON representation.
fn parse_protocol_params_cbor(payload: &[u8]) -> Result<String, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(N2CClientError::Protocol(format!(
            "expected MsgResult(4), got {tag}"
        )));
    }

    // Try to parse as CBOR map first (new format), fall back to string (legacy)
    match decoder.datatype() {
        Ok(minicbor::data::Type::Map) => {
            let map_len = decoder
                .map()
                .map_err(|e| N2CClientError::Protocol(format!("bad map: {e}")))?
                .unwrap_or(0);

            let key_names: &[&str] = &[
                "txFeePerByte",        // 0
                "txFeeFixed",          // 1
                "maxBlockBodySize",    // 2
                "maxTxSize",           // 3
                "maxBlockHeaderSize",  // 4
                "stakeAddressDeposit", // 5
                "stakePoolDeposit",    // 6
                "poolRetireMaxEpoch",  // 7
                "stakePoolTargetNum",  // 8
            ];

            let rational_names: &[&str] = &[
                "poolPledgeInfluence", // 9
                "monetaryExpansion",   // 10
                "treasuryCut",         // 11
            ];

            let mut entries = Vec::new();

            for _ in 0..map_len {
                let key = decoder
                    .u32()
                    .map_err(|e| N2CClientError::Protocol(format!("bad key: {e}")))?;

                match key {
                    0..=8 => {
                        let val = decoder
                            .u64()
                            .map_err(|e| N2CClientError::Protocol(format!("bad u64: {e}")))?;
                        if let Some(name) = key_names.get(key as usize) {
                            entries.push(format!("  \"{name}\": {val}"));
                        }
                    }
                    9..=11 => {
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let num = decoder.u64().unwrap_or(0);
                        let den = decoder.u64().unwrap_or(1);
                        if let Some(name) = rational_names.get((key - 9) as usize) {
                            entries.push(format!(
                                "  \"{name}\": {{\n    \"numerator\": {num},\n    \"denominator\": {den}\n  }}"
                            ));
                        }
                    }
                    16 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"minPoolCost\": {val}"));
                    }
                    17 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"utxoCostPerByte\": {val}"));
                    }
                    22 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxValueSize\": {val}"));
                    }
                    23 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"collateralPercentage\": {val}"));
                    }
                    24 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxCollateralInputs\": {val}"));
                    }
                    _ => {
                        let _ = decoder.skip();
                    }
                }
            }
            Ok(format!("{{\n{}\n}}", entries.join(",\n")))
        }
        Ok(minicbor::data::Type::String) => {
            // Legacy string format fallback
            decoder
                .str()
                .map(|s| s.to_string())
                .map_err(|e| N2CClientError::Protocol(format!("bad string: {e}")))
        }
        Ok(dt) => Err(N2CClientError::Protocol(format!(
            "unexpected CBOR type for protocol params: {dt:?}"
        ))),
        Err(e) => Err(N2CClientError::Protocol(format!("decode error: {e}"))),
    }
}

/// Parse a u64 from MsgResult CBOR
fn parse_u64_result(payload: &[u8]) -> Result<u64, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(N2CClientError::Protocol(format!(
            "expected MsgResult(4), got {tag}"
        )));
    }
    decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad u64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tip_result() {
        // Build a MsgResult: [4, [[slot, hash], block_no]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.array(2).unwrap();
        enc.array(2).unwrap();
        enc.u64(12345).unwrap();
        enc.bytes(&[0xab; 32]).unwrap();
        enc.u64(100).unwrap();

        let result = parse_tip_result(&buf).unwrap();
        assert_eq!(result.slot, 12345);
        assert_eq!(result.hash, vec![0xab; 32]);
        assert_eq!(result.block_no, 100);
    }

    #[test]
    fn test_parse_epoch_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.u64(500).unwrap();

        assert_eq!(parse_epoch_result(&buf).unwrap(), 500);
    }

    #[test]
    fn test_parse_u64_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.u64(42000).unwrap();

        assert_eq!(parse_u64_result(&buf).unwrap(), 42000);
    }

    #[test]
    fn test_parse_bad_tag_rejected() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(5).unwrap(); // Wrong tag
        enc.u64(100).unwrap();

        assert!(parse_u64_result(&buf).is_err());
    }

    #[test]
    fn test_parse_protocol_params_legacy_string() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.str("{\"min_fee_a\": 44}").unwrap();

        let result = parse_protocol_params_cbor(&buf).unwrap();
        assert!(result.contains("min_fee_a"));
        assert!(result.contains("44"));
    }

    #[test]
    fn test_parse_protocol_params_cbor_map() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        // CBOR map with integer keys
        enc.map(3).unwrap();
        enc.u32(0).unwrap().u64(44).unwrap(); // txFeePerByte
        enc.u32(1).unwrap().u64(155381).unwrap(); // txFeeFixed
        enc.u32(9).unwrap(); // poolPledgeInfluence (tagged rational)
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(3).unwrap();
        enc.u64(10).unwrap();

        let result = parse_protocol_params_cbor(&buf).unwrap();
        assert!(result.contains("\"txFeePerByte\": 44"));
        assert!(result.contains("\"txFeeFixed\": 155381"));
        assert!(result.contains("\"poolPledgeInfluence\""));
        assert!(result.contains("\"numerator\": 3"));
        assert!(result.contains("\"denominator\": 10"));
    }

    #[test]
    fn test_parse_tx_accept_response() {
        // Simulate what submit_tx would parse: MsgAcceptTx [1]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(1).unwrap(); // MsgAcceptTx

        let mut decoder = minicbor::Decoder::new(&buf);
        let _ = decoder.array();
        let tag = decoder.u32().unwrap();
        assert_eq!(tag, 1);
    }

    #[test]
    fn test_parse_tx_reject_response() {
        // Simulate MsgRejectTx [2, ["reason"]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap(); // MsgRejectTx
        enc.array(1).unwrap();
        enc.str("mempool full").unwrap();

        let mut decoder = minicbor::Decoder::new(&buf);
        let _ = decoder.array();
        let tag = decoder.u32().unwrap();
        assert_eq!(tag, 2);

        if let Ok(Some(_)) = decoder.array() {
            let reason = decoder.str().unwrap();
            assert_eq!(reason, "mempool full");
        } else {
            panic!("expected array");
        }
    }
}
