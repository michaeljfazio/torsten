/// Local TxSubmission mini-protocol (node-to-client)
///
/// Used by wallets and tools to submit transactions to a local node
/// via Unix domain socket.

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LocalTxSubmissionMessage {
    SubmitTx(Vec<u8>),
    AcceptTx,
    RejectTx(Vec<String>),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)]
pub enum LocalTxSubmissionState {
    StIdle,
    StBusy,
    StDone,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LocalTxSubmissionState ────────────────────────────────────────────────

    #[test]
    fn test_state_variants_are_distinct() {
        let states = [
            LocalTxSubmissionState::StIdle,
            LocalTxSubmissionState::StBusy,
            LocalTxSubmissionState::StDone,
        ];
        for (i, s1) in states.iter().enumerate() {
            for (j, s2) in states.iter().enumerate() {
                if i != j {
                    assert_ne!(s1, s2, "All LocalTxSubmission states must be distinct");
                }
            }
        }
    }

    // ── LocalTxSubmissionMessage CBOR wire format ────────────────────────────

    #[test]
    fn test_submit_tx_message_cbor_structure() {
        // LocalTxSubmission MsgSubmitTx: [0, tx_cbor:bytes]
        // The outer array tag is 0, followed by the raw transaction CBOR.
        let tx_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgSubmitTx tag
        enc.bytes(&tx_bytes).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 2, "MsgSubmitTx array length must be 2");
        assert_eq!(dec.u32().unwrap(), 0, "MsgSubmitTx tag must be 0");
        let decoded_tx = dec.bytes().unwrap();
        assert_eq!(decoded_tx, tx_bytes.as_slice());
    }

    #[test]
    fn test_accept_tx_message_cbor_structure() {
        // LocalTxSubmission MsgAcceptTx: [1]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(1).unwrap(); // MsgAcceptTx tag

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 1, "MsgAcceptTx array length must be 1");
        assert_eq!(dec.u32().unwrap(), 1, "MsgAcceptTx tag must be 1");
    }

    #[test]
    fn test_reject_tx_message_cbor_structure() {
        // LocalTxSubmission MsgRejectTx: [2, rejection_reason_cbor:bytes]
        // The rejection reason is opaque CBOR bytes in practice.
        let reason = b"input already spent";
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap(); // MsgRejectTx tag
        enc.bytes(reason).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 2, "MsgRejectTx array length must be 2");
        assert_eq!(dec.u32().unwrap(), 2, "MsgRejectTx tag must be 2");
        assert_eq!(dec.bytes().unwrap(), reason.as_slice());
    }

    #[test]
    fn test_done_message_cbor_structure() {
        // LocalTxSubmission MsgDone: [3]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(3).unwrap(); // MsgDone tag

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 1, "MsgDone array length must be 1");
        assert_eq!(dec.u32().unwrap(), 3, "MsgDone tag must be 3");
    }

    // ── Message variant identity ──────────────────────────────────────────────

    #[test]
    fn test_submit_tx_carries_bytes() {
        let payload = vec![1u8, 2, 3, 4];
        let msg = LocalTxSubmissionMessage::SubmitTx(payload.clone());
        match msg {
            LocalTxSubmissionMessage::SubmitTx(bytes) => assert_eq!(bytes, payload),
            _ => panic!("Expected SubmitTx"),
        }
    }

    #[test]
    fn test_reject_tx_carries_error_list() {
        let errors = vec!["BadInputsUTxO".to_string(), "FeeTooSmall".to_string()];
        let msg = LocalTxSubmissionMessage::RejectTx(errors.clone());
        match msg {
            LocalTxSubmissionMessage::RejectTx(errs) => assert_eq!(errs, errors),
            _ => panic!("Expected RejectTx"),
        }
    }
}
