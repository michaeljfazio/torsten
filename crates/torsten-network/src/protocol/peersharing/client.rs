//! PeerSharing client — requests peer addresses from hot peers.

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use std::net::SocketAddr;

use super::{decode_message, encode_message, PeerSharingMessage};

/// PeerSharing client that requests addresses from a remote peer.
pub struct PeerSharingClient;

impl PeerSharingClient {
    /// Request peer addresses from the remote peer.
    ///
    /// Sends `MsgShareRequest(amount)` and returns the received addresses.
    pub async fn request_peers(
        channel: &mut MuxChannel,
        amount: u8,
    ) -> Result<Vec<SocketAddr>, ProtocolError> {
        let req = encode_message(&PeerSharingMessage::MsgShareRequest(amount));
        channel.send(req).await.map_err(ProtocolError::from)?;

        let response_bytes = channel.recv().await.map_err(ProtocolError::from)?;
        let response = decode_message(&response_bytes).map_err(|e| ProtocolError::CborDecode {
            protocol: "PeerSharing",
            reason: e,
        })?;

        match response {
            PeerSharingMessage::MsgSharePeers(addrs) => Ok(addrs),
            other => Err(ProtocolError::StateViolation {
                protocol: "PeerSharing",
                expected: "MsgSharePeers".to_string(),
                actual: format!("{other:?}"),
            }),
        }
    }

    /// Send MsgDone to terminate the PeerSharing protocol.
    pub async fn done(channel: &mut MuxChannel) -> Result<(), ProtocolError> {
        let msg = encode_message(&PeerSharingMessage::MsgDone);
        channel.send(msg).await.map_err(ProtocolError::from)
    }
}
