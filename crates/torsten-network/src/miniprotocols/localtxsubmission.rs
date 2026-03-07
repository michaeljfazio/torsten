/// Local TxSubmission mini-protocol (node-to-client)
///
/// Used by wallets and tools to submit transactions to a local node
/// via Unix domain socket.

#[derive(Debug, Clone)]
pub enum LocalTxSubmissionMessage {
    SubmitTx(Vec<u8>),
    AcceptTx,
    RejectTx(Vec<String>),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTxSubmissionState {
    StIdle,
    StBusy,
    StDone,
}
