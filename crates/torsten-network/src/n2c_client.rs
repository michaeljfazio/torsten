use std::path::Path;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::debug;

use crate::multiplexer::{Segment, SEGMENT_HEADER_SIZE};

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
const MINI_PROTOCOL_TX_MONITOR: u16 = 9;

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
#[allow(dead_code)]
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
        // Propose versions 16-17 (current N2C versions for Conway era)
        // Version numbers are sent both with and without bit 15 set
        // to be compatible with both torsten and Haskell cardano-node servers
        let n2c_versions = [16u32, 17];
        let mut proposal = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut proposal);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(0)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgProposeVersions
        enc.map(n2c_versions.len() as u64)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;

        for version in n2c_versions {
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
                let version = decoder.u32().unwrap_or(0); // version only for logging
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

    /// Query the chain tip (GetLedgerTip - Shelley query tag 0)
    pub async fn query_tip(&mut self) -> Result<TipResult, N2CClientError> {
        let result = self.send_query(0).await?;
        parse_tip_result(&result)
    }

    /// Query the current epoch number (GetEpochNo - Shelley query tag 1)
    pub async fn query_epoch(&mut self) -> Result<u64, N2CClientError> {
        let result = self.send_query(1).await?;
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
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad MsgResult tag: {e}")))?;
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

    /// Query the system start time (GetSystemStart - top-level query tag 1)
    ///
    /// Returns the system start time as an ISO-8601 UTC string, e.g. "2022-10-25T00:00:00Z"
    pub async fn query_system_start(&mut self) -> Result<String, N2CClientError> {
        // GetSystemStart is a top-level (non-era-wrapped) query
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgQuery
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetSystemStart

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        let mut decoder = minicbor::Decoder::new(&resp.payload);
        // MsgResult [4, result]
        let _ = decoder.array();
        let tag = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad MsgResult tag: {e}")))?;
        if tag != 4 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgResult(4), got {tag}"
            )));
        }
        // SystemStart is encoded as UTCTime: [year, day_of_year, pico_of_day]
        let _ = decoder.array(); // array(3)
        let year = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad year: {e}")))?;
        let day_of_year = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad day: {e}")))?;
        let pico_of_day = decoder
            .u64()
            .map_err(|e| N2CClientError::Protocol(format!("bad pico: {e}")))?;

        // Convert to ISO-8601 UTC timestamp
        // day_of_year is 1-based, convert to month/day
        let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_months: [u32; 12] = if is_leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut remaining = day_of_year;
        let mut month = 1u32;
        for &days in &days_in_months {
            if remaining <= days {
                break;
            }
            remaining -= days;
            month += 1;
        }
        let day = remaining;
        let secs_of_day = (pico_of_day / 1_000_000_000_000) as u32;
        let hours = secs_of_day / 3600;
        let minutes = (secs_of_day % 3600) / 60;
        let seconds = secs_of_day % 60;

        Ok(format!(
            "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"
        ))
    }

    /// Query the chain block number (GetChainBlockNo - top-level query tag 2)
    pub async fn query_block_no(&mut self) -> Result<u64, N2CClientError> {
        // GetChainBlockNo is a top-level (non-era-wrapped) query
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(3)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // MsgQuery
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetChainBlockNo

        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: false,
            payload,
        };
        self.send_segment(&segment).await?;

        let resp = self.recv_segment().await?;
        parse_u64_result(&resp.payload)
    }

    /// Query protocol parameters (GetCurrentPParams - Shelley query tag 3)
    pub async fn query_protocol_params(&mut self) -> Result<String, N2CClientError> {
        let result = self.send_query(3).await?;
        parse_protocol_params_cbor(&result)
    }

    /// Query stake distribution (GetStakeDistribution - Shelley query tag 5)
    pub async fn query_stake_distribution(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(5).await?;
        // Return raw CBOR for the CLI to parse
        Ok(result)
    }

    /// Query account state (GetAccountState - Shelley query tag 29)
    /// Returns treasury and reserves in lovelace
    pub async fn query_account_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(29).await?;
        Ok(result)
    }

    /// Query constitution (GetConstitution - Shelley query tag 23)
    pub async fn query_constitution(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(23).await?;
        Ok(result)
    }

    /// Query governance state (GetGovState - Shelley query tag 24)
    pub async fn query_gov_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(24).await?;
        Ok(result)
    }

    /// Query ratification state (GetRatifyState - Shelley query tag 32)
    pub async fn query_ratify_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(32).await?;
        Ok(result)
    }

    /// Query DRep state (GetDRepState - Shelley query tag 25)
    pub async fn query_drep_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(25).await?;
        Ok(result)
    }

    /// Query committee state (GetCommitteeMembersState - Shelley query tag 27)
    pub async fn query_committee_state(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(27).await?;
        Ok(result)
    }

    /// Query stake address info for a specific credential (GetFilteredDelegationsAndRewardAccounts - Shelley query tag 10).
    ///
    /// The `credential_hash` is the 28-byte staking credential extracted from a stake address.
    /// Sends tag(258) Set<Credential> as the query argument, matching the Haskell wire format.
    /// If `credential_hash` is empty, sends an empty set (returns all stake addresses from the node).
    pub async fn query_stake_address_info(
        &mut self,
        credential_hash: &[u8],
    ) -> Result<Vec<u8>, N2CClientError> {
        // Build MsgQuery: [3, [era, [10, tag(258) [credential]]]]
        // Credential = [0, hash(28)]  (0 = KeyHash credential type)
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
        enc.u32(10)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetFilteredDelegationsAndRewardAccounts

        if credential_hash.is_empty() {
            // Empty set: tag(258) array(0) — returns all stake address info
            enc.tag(minicbor::data::Tag::new(258))
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.array(0)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        } else {
            // Single-element set: tag(258) array(1) [credential]
            // Credential = [0, hash(28)]
            enc.tag(minicbor::data::Tag::new(258))
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.array(1)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.array(2)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.u32(0)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // KeyHash credential
            enc.bytes(credential_hash)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        }

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

    /// Query UTxOs by specific transaction inputs (GetUTxOByTxIn - Shelley query tag 15).
    ///
    /// Each input is a `(tx_hash_bytes, output_index)` pair. The query sends a
    /// tag(258) canonical CBOR set of `[tx_hash, index]` pairs and returns the
    /// matching UTxO outputs in the same wire format as `GetUTxOByAddress`.
    pub async fn query_utxo_by_txin(
        &mut self,
        inputs: &[(Vec<u8>, u32)],
    ) -> Result<Vec<u8>, N2CClientError> {
        // Build MsgQuery: [3, [era, [15, tag(258) [[tx_hash, idx], ...]]]]
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
        enc.u32(15)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?; // GetUTxOByTxIn

        // Encode as tag(258) Set<TxIn> where TxIn = [tx_hash, output_index]
        enc.tag(minicbor::data::Tag::new(258))
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.array(inputs.len() as u64)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        for (tx_hash, index) in inputs {
            enc.array(2)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.bytes(tx_hash)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
            enc.u32(*index)
                .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        }

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

    /// Query stake snapshots (GetStakeSnapshots - Shelley query tag 20)
    pub async fn query_stake_snapshot(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(20).await?;
        Ok(result)
    }

    /// Query stake pool params (GetStakePoolParams - Shelley query tag 17)
    pub async fn query_pool_params(&mut self) -> Result<Vec<u8>, N2CClientError> {
        let result = self.send_query(17).await?;
        Ok(result)
    }

    /// Query UTxOs at a specific address (GetUTxOByAddress - Shelley query tag 6)
    pub async fn query_utxo_by_address(
        &mut self,
        addr_bytes: &[u8],
    ) -> Result<Vec<u8>, N2CClientError> {
        // Build MsgQuery with address parameter: [3, [era, [6, addr_bytes]]]
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
        enc.u32(6)
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
        enc.u32(1)
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
        if tag != 2 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgAcquired(2), got {tag}"
            )));
        }
        let slot = decoder
            .u64()
            .map_err(|e| N2CClientError::Protocol(format!("bad mempool slot: {e}")))?;
        Ok(slot)
    }

    /// Check if a transaction is in the mempool
    pub async fn monitor_has_tx(&mut self, tx_hash: &[u8; 32]) -> Result<bool, N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(7)
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
        if tag != 8 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgReplyHasTx(8), got {tag}"
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
        enc.u32(9)
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
        if tag != 10 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgReplyGetSizes(10), got {tag}"
            )));
        }
        let _ = decoder.array();
        let capacity = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad mempool capacity: {e}")))?;
        let size = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad mempool size: {e}")))?;
        let num_txs = decoder
            .u32()
            .map_err(|e| N2CClientError::Protocol(format!("bad mempool num_txs: {e}")))?;
        Ok((capacity, size, num_txs))
    }

    /// Get the next transaction from the mempool snapshot.
    /// Returns `Some((era_id, tx_bytes))` if a transaction is available, or `None` if empty.
    pub async fn monitor_next_tx(&mut self) -> Result<Option<(u32, Vec<u8>)>, N2CClientError> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1)
            .map_err(|e| N2CClientError::Protocol(e.to_string()))?;
        enc.u32(5)
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
        if tag != 6 {
            return Err(N2CClientError::Protocol(format!(
                "expected MsgReplyNextTx(6), got {tag}"
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
        enc.u32(3)
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

    /// Receive a complete multiplexer message, reassembling multiple wire segments.
    ///
    /// The Ouroboros multiplexer splits messages larger than 65535 bytes into
    /// multiple wire segments. This method reads all consecutive segments for
    /// the same protocol and concatenates their payloads into a single logical
    /// Segment.
    async fn recv_segment(&mut self) -> Result<Segment, N2CClientError> {
        // Initial buffer: enough for one max wire segment (header + 65535 payload)
        let mut buf = vec![0u8; SEGMENT_HEADER_SIZE + 65535 + 4096];
        let mut buf_offset = 0;

        let mut combined_payload: Vec<u8> = Vec::new();
        let mut first_segment: Option<Segment> = None;

        loop {
            // Try to decode a wire segment from buffered data
            match Segment::decode(&buf[..buf_offset]) {
                Ok((segment, consumed)) => {
                    // Accumulate payload
                    combined_payload.extend_from_slice(&segment.payload);

                    if first_segment.is_none() {
                        first_segment = Some(Segment {
                            transmission_time: segment.transmission_time,
                            protocol_id: segment.protocol_id,
                            is_responder: segment.is_responder,
                            payload: Vec::new(), // will be replaced
                        });
                    }

                    // Shift remaining data to front of buffer
                    let remaining = buf_offset - consumed;
                    if remaining > 0 {
                        buf.copy_within(consumed..buf_offset, 0);
                    }
                    buf_offset = remaining;

                    // If the segment payload was less than max, this is the last chunk
                    if segment.payload.len() < 65535 {
                        let mut result = first_segment.ok_or_else(|| {
                            N2CClientError::Protocol("no first segment in reassembly".into())
                        })?;
                        result.payload = combined_payload;
                        return Ok(result);
                    }
                    // Otherwise, keep reading — more chunks expected
                    continue;
                }
                Err(_) => {
                    // Need more data from the network
                    if buf_offset >= buf.len() {
                        // Grow the buffer
                        buf.resize(buf.len() + 65536, 0);
                    }
                    let n = self.stream.read(&mut buf[buf_offset..]).await?;
                    if n == 0 {
                        return Err(N2CClientError::Protocol("connection closed".into()));
                    }
                    buf_offset += n;
                }
            }
        }
    }
}

/// Parse a tip result from MsgResult CBOR
/// Strip the MsgResult [4, ...] envelope from a response payload.
/// Returns the position in the decoder after the tag.
fn strip_msg_result(decoder: &mut minicbor::Decoder) -> Result<(), N2CClientError> {
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(N2CClientError::Protocol(format!(
            "expected MsgResult(4), got {tag}"
        )));
    }
    Ok(())
}

/// Strip the HardFork Combinator success wrapper [result] (1-element array).
/// Handles both wrapped (from Haskell node) and unwrapped (from Torsten) responses.
fn strip_hfc_wrapper(decoder: &mut minicbor::Decoder) -> Result<(), N2CClientError> {
    // The HFC wrapper is array(1). Save position so we can restore if no wrapper.
    let pos = decoder.position();
    if let Ok(Some(1)) = decoder.array() {
        // Consumed the HFC wrapper
        Ok(())
    } else {
        // No wrapper — restore position so the caller can read the actual value
        decoder.set_position(pos);
        Ok(())
    }
}

fn parse_tip_result(payload: &[u8]) -> Result<TipResult, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    strip_msg_result(&mut decoder)?;
    strip_hfc_wrapper(&mut decoder)?;

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
    strip_msg_result(&mut decoder)?;
    strip_hfc_wrapper(&mut decoder)?;
    decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad epoch: {e}")))
}

/// Format a tagged rational (num/den) as a cardano-cli compatible decimal string.
///
/// cardano-cli renders protocol-parameter rationals as JSON numbers using the
/// minimum decimal representation that round-trips exactly, not as
/// `{numerator, denominator}` objects.
///
/// Examples:
///   3/10  → 0.3
///   3/1000 → 0.003
///   1/20  → 0.05
///   15/1  → 15.0
///   577/10000 → 0.0577
fn format_rational_decimal(num: u64, den: u64) -> String {
    if den == 0 {
        return "0.0".to_string();
    }
    // Compute the exact decimal using integer arithmetic.
    // We need enough decimal places to represent the fraction exactly (up to 12 sig figs).
    let int_part = num / den;
    let remainder = num % den;
    if remainder == 0 {
        // Exact integer — cardano-cli still outputs decimal point for rational fields
        return format!("{int_part}.0");
    }
    // Find exact decimal representation (up to 12 decimal places)
    let mut frac_digits = String::new();
    let mut rem = remainder;
    for _ in 0..12 {
        rem *= 10;
        frac_digits.push(char::from_digit((rem / den) as u32, 10).unwrap_or('0'));
        rem %= den;
        if rem == 0 {
            break;
        }
    }
    // Trim trailing zeros (keep at least one digit after decimal)
    let trimmed = frac_digits.trim_end_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    format!("{int_part}.{trimmed}")
}

/// Parse protocol params CBOR into JSON string matching cardano-cli 10.x output format.
///
/// Handles both array(31) format (Haskell/new Torsten) and legacy CBOR map format.
///
/// Key format compatibility notes:
/// - Rational fields (poolPledgeInfluence, monetaryExpansion, treasuryCut,
///   executionUnitPrices.priceMemory/priceSteps, voting thresholds,
///   minFeeRefScriptCostPerByte) are output as decimal JSON numbers, matching
///   cardano-cli 10.x — NOT as `{numerator, denominator}` objects.
/// - costModels keys are "PlutusV1", "PlutusV2", "PlutusV3" (matching cardano-cli).
/// - Fields appear in positional (array index) order, matching Haskell encoding.
fn parse_protocol_params_cbor(payload: &[u8]) -> Result<String, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    strip_msg_result(&mut decoder)?;
    strip_hfc_wrapper(&mut decoder)?;

    match decoder.datatype() {
        Ok(minicbor::data::Type::Array) => {
            // Positional array(31) format (Haskell ConwayPParams encoding)
            let arr_len = decoder
                .array()
                .map_err(|e| N2CClientError::Protocol(format!("bad array: {e}")))?
                .unwrap_or(0);

            // Integer fields [0]..[8]
            let field_names: &[&str] = &[
                "txFeePerByte",        // [0]
                "txFeeFixed",          // [1]
                "maxBlockBodySize",    // [2]
                "maxTxSize",           // [3]
                "maxBlockHeaderSize",  // [4]
                "stakeAddressDeposit", // [5]
                "stakePoolDeposit",    // [6]
                "poolRetireMaxEpoch",  // [7]
                "stakePoolTargetNum",  // [8]
            ];

            // Rational fields [9]..[11] — output as decimals
            let rational_fields: &[&str] = &[
                "poolPledgeInfluence", // [9]
                "monetaryExpansion",   // [10]
                "treasuryCut",         // [11]
            ];

            let mut entries = Vec::new();

            for i in 0..arr_len {
                match i {
                    0..=8 => {
                        let val = decoder.u64().unwrap_or(0);
                        if let Some(name) = field_names.get(i as usize) {
                            entries.push(format!("  \"{name}\": {val}"));
                        }
                    }
                    9..=11 => {
                        // Rational: tag(30) [num, den] — output as decimal number
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let num = decoder.u64().unwrap_or(0);
                        let den = decoder.u64().unwrap_or(1);
                        if let Some(name) = rational_fields.get((i - 9) as usize) {
                            let decimal = format_rational_decimal(num, den);
                            entries.push(format!("  \"{name}\": {decimal}"));
                        }
                    }
                    12 => {
                        // protocolVersion [major, minor]
                        let _ = decoder.array();
                        let major = decoder.u64().unwrap_or(0);
                        let minor = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"protocolVersion\": {{\n    \"major\": {major},\n    \"minor\": {minor}\n  }}"
                        ));
                    }
                    13 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"minPoolCost\": {val}"));
                    }
                    14 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"utxoCostPerByte\": {val}"));
                    }
                    15 => {
                        // costModels (CBOR map: {0: [v1_costs], 1: [v2_costs], 2: [v3_costs]})
                        // Output as {"PlutusV1": [...], "PlutusV2": [...], "PlutusV3": [...]}
                        let mut cm_entries = Vec::new();
                        if let Ok(Some(map_len)) = decoder.map() {
                            for _ in 0..map_len {
                                let lang = decoder.u32().unwrap_or(0);
                                let lang_name = match lang {
                                    0 => "PlutusV1",
                                    1 => "PlutusV2",
                                    2 => "PlutusV3",
                                    _ => "Unknown",
                                };
                                let mut costs = Vec::new();
                                if let Ok(Some(arr_len)) = decoder.array() {
                                    for _ in 0..arr_len {
                                        costs.push(decoder.i64().unwrap_or(0));
                                    }
                                }
                                let costs_str: Vec<String> =
                                    costs.iter().map(|c| c.to_string()).collect();
                                cm_entries.push(format!(
                                    "    \"{lang_name}\": [{}]",
                                    costs_str.join(", ")
                                ));
                            }
                        } else {
                            let _ = decoder.skip();
                        }
                        entries.push(format!(
                            "  \"costModels\": {{\n{}\n  }}",
                            cm_entries.join(",\n")
                        ));
                    }
                    16 => {
                        // prices [mem_price, step_price] as tagged rationals
                        // Output as decimal numbers (matching cardano-cli)
                        let _ = decoder.array();
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let mem_num = decoder.u64().unwrap_or(0);
                        let mem_den = decoder.u64().unwrap_or(1);
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let step_num = decoder.u64().unwrap_or(0);
                        let step_den = decoder.u64().unwrap_or(1);
                        let mem_decimal = format_rational_decimal(mem_num, mem_den);
                        let step_decimal = format_rational_decimal(step_num, step_den);
                        entries.push(format!(
                            "  \"executionUnitPrices\": {{\n    \"priceMemory\": {mem_decimal},\n    \"priceSteps\": {step_decimal}\n  }}"
                        ));
                    }
                    17 => {
                        // maxTxExUnits [mem, steps]
                        let _ = decoder.array();
                        let mem = decoder.u64().unwrap_or(0);
                        let steps = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"maxTxExecutionUnits\": {{\n    \"memory\": {mem},\n    \"steps\": {steps}\n  }}"
                        ));
                    }
                    18 => {
                        // maxBlockExUnits [mem, steps]
                        let _ = decoder.array();
                        let mem = decoder.u64().unwrap_or(0);
                        let steps = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"maxBlockExecutionUnits\": {{\n    \"memory\": {mem},\n    \"steps\": {steps}\n  }}"
                        ));
                    }
                    19 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxValueSize\": {val}"));
                    }
                    20 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"collateralPercentage\": {val}"));
                    }
                    21 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxCollateralInputs\": {val}"));
                    }
                    22 => {
                        // poolVotingThresholds (5 tagged rationals) — output as decimals
                        // Order matches Haskell PoolVotingThresholds field encoding
                        let pvt_names = [
                            "pvtMotionNoConfidence",
                            "pvtCommitteeNormal",
                            "pvtCommitteeNoConfidence",
                            "pvtHardForkInitiation",
                            "pvtPPSecurityGroup",
                        ];
                        let _ = decoder.array();
                        let mut pvt_entries = Vec::new();
                        for name in &pvt_names {
                            let _ = decoder.tag();
                            let _ = decoder.array();
                            let num = decoder.u64().unwrap_or(0);
                            let den = decoder.u64().unwrap_or(1);
                            let decimal = format_rational_decimal(num, den);
                            pvt_entries.push(format!("    \"{name}\": {decimal}"));
                        }
                        entries.push(format!(
                            "  \"poolVotingThresholds\": {{\n{}\n  }}",
                            pvt_entries.join(",\n")
                        ));
                    }
                    23 => {
                        // drepVotingThresholds (10 tagged rationals) — output as decimals
                        // Order matches Haskell DRepVotingThresholds field encoding
                        let dvt_names = [
                            "dvtMotionNoConfidence",
                            "dvtCommitteeNormal",
                            "dvtCommitteeNoConfidence",
                            "dvtUpdateToConstitution",
                            "dvtHardForkInitiation",
                            "dvtPPNetworkGroup",
                            "dvtPPEconomicGroup",
                            "dvtPPTechnicalGroup",
                            "dvtPPGovGroup",
                            "dvtTreasuryWithdrawal",
                        ];
                        let _ = decoder.array();
                        let mut dvt_entries = Vec::new();
                        for name in &dvt_names {
                            let _ = decoder.tag();
                            let _ = decoder.array();
                            let num = decoder.u64().unwrap_or(0);
                            let den = decoder.u64().unwrap_or(1);
                            let decimal = format_rational_decimal(num, den);
                            dvt_entries.push(format!("    \"{name}\": {decimal}"));
                        }
                        entries.push(format!(
                            "  \"drepVotingThresholds\": {{\n{}\n  }}",
                            dvt_entries.join(",\n")
                        ));
                    }
                    24 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"committeeMinSize\": {val}"));
                    }
                    25 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"committeeMaxTermLength\": {val}"));
                    }
                    26 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"govActionLifetime\": {val}"));
                    }
                    27 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"govActionDeposit\": {val}"));
                    }
                    28 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"dRepDeposit\": {val}"));
                    }
                    29 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"dRepActivity\": {val}"));
                    }
                    30 => {
                        // minFeeRefScriptCostPerByte (tagged rational) — output as decimal
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let num = decoder.u64().unwrap_or(0);
                        let den = decoder.u64().unwrap_or(1);
                        let decimal = format_rational_decimal(num, den);
                        entries.push(format!("  \"minFeeRefScriptCostPerByte\": {decimal}"));
                    }
                    _ => {
                        let _ = decoder.skip();
                    }
                }
            }
            Ok(format!("{{\n{}\n}}", entries.join(",\n")))
        }
        Ok(minicbor::data::Type::Map) => {
            // Legacy CBOR map format — kept for backward compatibility
            let map_len = decoder
                .map()
                .map_err(|e| N2CClientError::Protocol(format!("bad map: {e}")))?
                .unwrap_or(0);
            let mut entries = Vec::new();
            for _ in 0..map_len {
                let key = decoder.u32().unwrap_or(999);
                let _ = decoder.skip();
                entries.push(format!("  \"key_{key}\": \"skipped\""));
            }
            Ok(format!("{{\n{}\n}}", entries.join(",\n")))
        }
        Ok(minicbor::data::Type::String) => decoder
            .str()
            .map(|s| s.to_string())
            .map_err(|e| N2CClientError::Protocol(format!("bad string: {e}"))),
        Ok(dt) => Err(N2CClientError::Protocol(format!(
            "unexpected CBOR type for protocol params: {dt:?}"
        ))),
        Err(e) => Err(N2CClientError::Protocol(format!("decode error: {e}"))),
    }
}

/// Parse a u64 from MsgResult CBOR (for BlockQuery results with HFC wrapper)
fn parse_u64_result(payload: &[u8]) -> Result<u64, N2CClientError> {
    let mut decoder = minicbor::Decoder::new(payload);
    strip_msg_result(&mut decoder)?;
    strip_hfc_wrapper(&mut decoder)?;
    decoder
        .u64()
        .map_err(|e| N2CClientError::Protocol(format!("bad u64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── format_rational_decimal tests ───────────────────────────────────────

    #[test]
    fn test_format_rational_decimal_basic() {
        // 3/10 = 0.3
        assert_eq!(format_rational_decimal(3, 10), "0.3");
        // 3/1000 = 0.003
        assert_eq!(format_rational_decimal(3, 1000), "0.003");
        // 1/20 = 0.05
        assert_eq!(format_rational_decimal(1, 20), "0.05");
        // 2/10 = 0.2
        assert_eq!(format_rational_decimal(2, 10), "0.2");
    }

    #[test]
    fn test_format_rational_decimal_integers() {
        // 15/1 = 15.0  (integer-valued rationals keep .0 suffix)
        assert_eq!(format_rational_decimal(15, 1), "15.0");
        // 0/1 = 0.0
        assert_eq!(format_rational_decimal(0, 1), "0.0");
    }

    #[test]
    fn test_format_rational_decimal_execution_unit_prices() {
        // 577/10000 = 0.0577  (priceMemory)
        assert_eq!(format_rational_decimal(577, 10000), "0.0577");
        // 721/10000000 = 0.0000721  (priceSteps)
        assert_eq!(format_rational_decimal(721, 10_000_000), "0.0000721");
    }

    #[test]
    fn test_format_rational_decimal_voting_thresholds() {
        // 51/100 = 0.51
        assert_eq!(format_rational_decimal(51, 100), "0.51");
        // 67/100 = 0.67
        assert_eq!(format_rational_decimal(67, 100), "0.67");
        // 60/100 = 0.6
        assert_eq!(format_rational_decimal(60, 100), "0.6");
        // 75/100 = 0.75
        assert_eq!(format_rational_decimal(75, 100), "0.75");
    }

    // ─── parse_tip_result tests ───────────────────────────────────────────────

    #[test]
    fn test_parse_tip_result() {
        // Build a MsgResult: [4, [[[slot, hash], block_no]]] (with HFC wrapper)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.array(1).unwrap(); // HFC success wrapper
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
    fn test_parse_tip_result_no_hfc_wrapper() {
        // Verify strip_hfc_wrapper handles responses without wrapper gracefully
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        // No HFC wrapper — directly the result
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
        enc.array(1).unwrap(); // HFC success wrapper
        enc.u64(500).unwrap();

        assert_eq!(parse_epoch_result(&buf).unwrap(), 500);
    }

    #[test]
    fn test_parse_u64_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.array(1).unwrap(); // HFC success wrapper
        enc.u64(42000).unwrap();

        assert_eq!(parse_u64_result(&buf).unwrap(), 42000);
    }

    #[test]
    fn test_parse_u64_result_no_hfc_wrapper() {
        // ChainBlockNo does NOT have HFC wrapper — just [4, value]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.u64(42000).unwrap(); // No HFC wrapper

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
        enc.array(1).unwrap(); // HFC success wrapper
        enc.str("{\"min_fee_a\": 44}").unwrap();

        let result = parse_protocol_params_cbor(&buf).unwrap();
        assert!(result.contains("min_fee_a"));
        assert!(result.contains("44"));
    }

    #[test]
    fn test_parse_protocol_params_array() {
        // Haskell-compatible array(31) format with HFC wrapper
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.array(1).unwrap(); // HFC success wrapper

        // Build a minimal array(31) PParams
        enc.array(31).unwrap();
        enc.u64(44).unwrap(); // [0] txFeePerByte
        enc.u64(155381).unwrap(); // [1] txFeeFixed
        enc.u64(90112).unwrap(); // [2] maxBBSize
        enc.u64(16384).unwrap(); // [3] maxTxSize
        enc.u64(1100).unwrap(); // [4] maxBHSize
        enc.u64(2_000_000).unwrap(); // [5] keyDeposit
        enc.u64(500_000_000).unwrap(); // [6] poolDeposit
        enc.u64(18).unwrap(); // [7] eMax
        enc.u64(500).unwrap(); // [8] nOpt
                               // [9] a0
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(3).unwrap();
        enc.u64(10).unwrap();
        // [10] rho
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(3).unwrap();
        enc.u64(1000).unwrap();
        // [11] tau
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(2).unwrap();
        enc.u64(10).unwrap();
        // [12] protocolVersion
        enc.array(2).unwrap();
        enc.u64(10).unwrap();
        enc.u64(0).unwrap();
        // [13-30]: fill remaining with zeros/empty
        enc.u64(340_000_000).unwrap(); // [13] minPoolCost
        enc.u64(4310).unwrap(); // [14] coinsPerUTxOByte
        enc.map(0).unwrap(); // [15] costModels (empty)
                             // [16] prices
        enc.array(2).unwrap();
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(577).unwrap();
        enc.u64(10000).unwrap();
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(721).unwrap();
        enc.u64(10000000).unwrap();
        // [17] maxTxExUnits
        enc.array(2).unwrap();
        enc.u64(14_000_000).unwrap();
        enc.u64(10_000_000_000).unwrap();
        // [18] maxBlockExUnits
        enc.array(2).unwrap();
        enc.u64(62_000_000).unwrap();
        enc.u64(20_000_000_000).unwrap();
        enc.u64(5000).unwrap(); // [19] maxValSize
        enc.u64(150).unwrap(); // [20] collateralPercentage
        enc.u64(3).unwrap(); // [21] maxCollateralInputs
                             // [22] poolVotingThresholds (5 tagged rationals)
        enc.array(5).unwrap();
        for _ in 0..5 {
            enc.tag(minicbor::data::Tag::new(30)).unwrap();
            enc.array(2).unwrap();
            enc.u64(51).unwrap();
            enc.u64(100).unwrap();
        }
        // [23] drepVotingThresholds (10 tagged rationals)
        enc.array(10).unwrap();
        for _ in 0..10 {
            enc.tag(minicbor::data::Tag::new(30)).unwrap();
            enc.array(2).unwrap();
            enc.u64(67).unwrap();
            enc.u64(100).unwrap();
        }
        enc.u64(7).unwrap(); // [24] committeeMinSize
        enc.u64(146).unwrap(); // [25] committeeMaxTermLength
        enc.u64(6).unwrap(); // [26] govActionLifetime
        enc.u64(100_000_000_000).unwrap(); // [27] govActionDeposit
        enc.u64(500_000_000).unwrap(); // [28] dRepDeposit
        enc.u64(20).unwrap(); // [29] dRepActivity
                              // [30] minFeeRefScriptCostPerByte (tagged rational)
        enc.tag(minicbor::data::Tag::new(30)).unwrap();
        enc.array(2).unwrap();
        enc.u64(15).unwrap();
        enc.u64(1).unwrap();

        let result = parse_protocol_params_cbor(&buf).unwrap();
        assert!(result.contains("\"txFeePerByte\": 44"));
        assert!(result.contains("\"txFeeFixed\": 155381"));
        // Rational fields must be decimal numbers, not {numerator, denominator} objects
        // (cardano-cli 10.x compatibility)
        assert!(result.contains("\"poolPledgeInfluence\": 0.3"));
        assert!(
            !result.contains("\"numerator\""),
            "rationals must not be objects — got: {result}"
        );
        assert!(result.contains("\"monetaryExpansion\": 0.003"));
        assert!(result.contains("\"treasuryCut\": 0.2"));
        // Verify new fields are parsed
        assert!(result.contains("\"costModels\""));
        assert!(result.contains("\"executionUnitPrices\""));
        // Execution unit prices as decimals
        assert!(result.contains("\"priceMemory\": 0.0577"));
        assert!(result.contains("\"priceSteps\":"));
        assert!(result.contains("\"poolVotingThresholds\""));
        assert!(result.contains("\"pvtMotionNoConfidence\": 0.51"));
        assert!(result.contains("\"drepVotingThresholds\""));
        assert!(result.contains("\"dvtMotionNoConfidence\": 0.67"));
        assert!(result.contains("\"committeeMinSize\": 7"));
        assert!(result.contains("\"committeeMaxTermLength\": 146"));
        assert!(result.contains("\"govActionLifetime\": 6"));
        assert!(result.contains("\"govActionDeposit\": 100000000000"));
        assert!(result.contains("\"dRepDeposit\": 500000000"));
        assert!(result.contains("\"dRepActivity\": 20"));
        // minFeeRefScriptCostPerByte as decimal (15/1 = 15.0)
        assert!(result.contains("\"minFeeRefScriptCostPerByte\": 15.0"));
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

    /// Test that recv_segment correctly reassembles multi-segment messages.
    ///
    /// Large payloads (>65535 bytes) are chunked by Segment::encode() into
    /// multiple wire segments. recv_segment must read all chunks and
    /// concatenate them.
    #[tokio::test]
    async fn test_recv_segment_multi_chunk_reassembly() {
        // Create a payload larger than MAX_SEGMENT_PAYLOAD (65535 bytes)
        let large_payload: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: true,
            payload: large_payload.clone(),
        };

        // encode() splits into multiple wire segments
        let wire_bytes = segment.encode();
        // Should be at least 2 wire segments (100KB > 65535)
        assert!(
            wire_bytes.len() > 65535 + SEGMENT_HEADER_SIZE,
            "expected multi-segment encoding"
        );

        // Use a Unix socket pair for the test
        let (sock_a, sock_b) = tokio::net::UnixStream::pair().unwrap();

        // Spawn writer on one end
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut writer_stream = sock_b;
            writer_stream.write_all(&wire_bytes).await.unwrap();
            writer_stream.shutdown().await.unwrap();
        });

        // Create an N2CClient using the other end
        let mut client = N2CClient { stream: sock_a };

        let result = client.recv_segment().await.unwrap();
        assert_eq!(result.payload.len(), large_payload.len());
        assert_eq!(result.payload, large_payload);
        assert_eq!(result.protocol_id, MINI_PROTOCOL_STATE_QUERY);

        writer.await.unwrap();
    }

    /// Test that recv_segment works for single-segment (small) messages.
    #[tokio::test]
    async fn test_recv_segment_single_chunk() {
        let small_payload = vec![0x83, 0x01, 0x02, 0x03]; // 4 bytes
        let segment = Segment {
            transmission_time: 0,
            protocol_id: MINI_PROTOCOL_STATE_QUERY,
            is_responder: true,
            payload: small_payload.clone(),
        };

        let wire_bytes = segment.encode();
        let (sock_a, sock_b) = tokio::net::UnixStream::pair().unwrap();

        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut writer_stream = sock_b;
            writer_stream.write_all(&wire_bytes).await.unwrap();
            writer_stream.shutdown().await.unwrap();
        });

        let mut client = N2CClient { stream: sock_a };

        let result = client.recv_segment().await.unwrap();
        assert_eq!(result.payload, small_payload);

        writer.await.unwrap();
    }
}
