# PeerSharing Mini-Protocol (N2N Protocol ID 10)

## Source Locations (ouroboros-network main branch)
- Protocol types: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Type.hs`
- Generic codec: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Codec.hs`
- Client: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Client.hs`
- Server: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Server.hs`
- High-level logic: `ouroboros-network/lib/Ouroboros/Network/PeerSharing.hs`
- Address codec: `ouroboros-network/api/lib/Ouroboros/Network/PeerSelection/PeerSharing/Codec.hs`
- Cardano codec wrapper: `cardano-diffusion/protocols/lib/Cardano/Network/Protocol/PeerSharing/Codec.hs`
- Governor integration: `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/KnownPeers.hs`
- Protocol limits: `ouroboros-network/api/lib/Ouroboros/Network/Protocol/Limits.hs`
- Default policies: `ouroboros-network/lib/Ouroboros/Network/Diffusion/Policies.hs`
- CDDL: `cardano-diffusion/protocols/cddl/specs/peer-sharing-v14.cddl`

## CBOR Wire Format
- MsgShareRequest: `[0, word8]` — CBOR list(2), word 0, then word8 amount
- MsgSharePeers:   `[1, [*peerAddress]]` — CBOR list(2), word 1, then list of addresses
- MsgDone:         `[2]` — CBOR list(1), word 2

## Peer Address Encoding (SockAddr)
- IPv4: `[0, word32, word16]` — tag 0, IPv4 as single word32, port as word16
- IPv6: `[1, word32, word32, word32, word32, word16]` — tag 1, 4x word32, port as word16

## Protocol Limits
- Max message size: 5760 bytes (4 * 1440 TCP segments)
- StIdle timeout: waitForever (None)
- StBusy timeout: longWait (60 seconds)

## Policy Constants (defaults)
- policyMaxInProgressPeerShareReqs: 2
- policyPeerShareRetryTime: 900s (15 min between re-asking same peer)
- policyPeerShareBatchWaitTime: 3s (phase 1 timeout)
- policyPeerShareOverallTimeout: 10s (phase 2 total)
- policyPeerShareActivationDelay: 300s (5 min before peer eligible)
- ps_POLICY_PEER_SHARE_STICKY_TIME: 823s (salt rotation for server randomization)
- ps_POLICY_PEER_SHARE_MAX_PEERS: 10 (max peers returned per request)

## Handshake Negotiation
- PeerSharing flag in NodeToNodeVersionData: 0=Disabled, 1=Enabled
- Negotiated as `local <> remote` (Semigroup: both must be Enabled for Enabled)
- InitiatorOnly nodes automatically disable PeerSharing (they can't serve)

## Server: Which Peers to Share
- Only peers with: knownPeerAdvertise=DoAdvertisePeer AND knownSuccessfulConnection=True AND knownPeerFailCount=0
- Randomized using hashWithSalt, salt rotates every 823 seconds
- Capped at min(ps_POLICY_PEER_SHARE_MAX_PEERS, requested_amount)

## Client: When/How Many to Request
- Triggered when numKnownPeers < targetNumberOfKnownPeers AND PeerSharingEnabled
- Picks from established peers that have PeerSharingEnabled and haven't been asked recently
- Amount per request: min(255, max(8, objective / numPeerShareReqs))
  where objective = targetNumberOfKnownPeers - numKnownPeers
- Two-phase job: phase1 waits 3s for all results, phase2 waits remaining 7s for stragglers
- Filters out already-known peers and big ledger peers from results
