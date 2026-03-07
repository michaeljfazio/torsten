use torsten_primitives::hash::TransactionHash;

/// TxSubmission2 mini-protocol messages
///
/// This protocol handles transaction submission between nodes.
/// The server requests transaction IDs, then selectively requests
/// full transactions it hasn't seen before.
#[derive(Debug, Clone)]
pub enum TxSubmissionMessage {
    // Server messages (server has agency in this protocol)
    RequestTxIds {
        blocking: bool,
        ack_count: u16,
        req_count: u16,
    },
    RequestTxs(Vec<TransactionHash>),

    // Client messages
    ReplyTxIds(Vec<(TransactionHash, u32)>), // (hash, size_in_bytes)
    ReplyTxs(Vec<Vec<u8>>),                  // CBOR-encoded transactions
    Done,
}

/// TxSubmission state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxSubmissionState {
    /// Server has agency
    StIdle,
    /// Client has agency (responding to RequestTxIds)
    StTxIds,
    /// Client has agency (responding to RequestTxs)
    StTxs,
    /// Protocol done
    StDone,
}

pub struct TxSubmissionClient {
    pub state: TxSubmissionState,
}

impl Default for TxSubmissionClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TxSubmissionClient {
    pub fn new() -> Self {
        TxSubmissionClient {
            state: TxSubmissionState::StIdle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_client() {
        let client = TxSubmissionClient::new();
        assert_eq!(client.state, TxSubmissionState::StIdle);
    }
}
