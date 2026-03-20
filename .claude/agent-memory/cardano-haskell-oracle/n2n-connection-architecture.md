---
name: N2N Connection Architecture
description: Complete deep-dive into Haskell ouroboros-network N2N connection management — MuxMode, DataFlow, protocol assignment, bit-15 convention, Hot/Warm/Cold transitions, TxSubmission2 specifics
type: reference
---

## Key Source Locations (ouroboros-network repo, main branch)
- MuxMode/Types: `network-mux/src/Network/Mux/Types.hs` (Mode GADT: InitiatorMode, ResponderMode, InitiatorResponderMode)
- Mux codec (SDU wire format, bit-15): `network-mux/src/Network/Mux/Codec.hs`
- Demuxer (flipMiniProtocolDir): `network-mux/src/Network/Mux/Ingress.hs` line 34-36
- Egress/Wanton: `network-mux/src/Network/Mux/Egress.hs`
- DiffusionMode: `ouroboros-network/api/lib/Ouroboros/Network/DiffusionMode.hs`
- Diffusion main: `ouroboros-network/lib/Ouroboros/Network/Diffusion.hs`
- OuroborosBundle/TemperatureBundle: `ouroboros-network/framework/lib/Ouroboros/Network/Mux.hs`
- Protocol numbers & limits: `cardano-diffusion/lib/Cardano/Network/NodeToNode.hs`
- NodeToNodeVersionData (handshake): `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs`
- DataFlow/ConnectionType: `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/Types.hs` lines 220-261
- PeerStateActions (Hot/Warm/Cold): `ouroboros-network/lib/Ouroboros/Network/PeerSelection/PeerStateActions.hs`
- ConnectionManager Core: `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/Core.hs`
- InboundGovernor: `ouroboros-network/framework/lib/Ouroboros/Network/InboundGovernor.hs`
- Configuration defaults: `ouroboros-network/lib/Ouroboros/Network/Diffusion/Configuration.hs`
- TxSubmission init delay: `ouroboros-network/lib/Ouroboros/Network/TxSubmission/Inbound/V2/Types.hs` line 469 (60s default)

## Critical Facts
- Bit-15 convention: initiator sends with bit-15 CLEAR (0x0000+num), responder sends with bit-15 SET (0x8000+num)
- Demuxer FLIPS direction on receive: incoming InitiatorDir delivered to ResponderDir queue, incoming ResponderDir to InitiatorDir queue
- ntnDataFlow: InitiatorAndResponderDiffusionMode -> Duplex, InitiatorOnlyDiffusionMode -> Unidirectional
- acceptableVersion: min(local.diffusionMode, remote.diffusionMode) — if either side is InitiatorOnly, connection is Unidirectional
- Protocol temperature assignment: Hot=[ChainSync(2), BlockFetch(3), TxSubmission2(4)], Warm=[], Established=[KeepAlive(8), PeerSharing(10)]
- InitiatorAndResponderProtocol: BOTH initiator AND responder callbacks registered for each mini-protocol
- Single TCP connection, two independent protocol instances (initiator+responder) per mini-protocol on duplex connections
- defaultProtocolIdleTimeout: 5s, defaultTimeWaitTimeout: 60s
- TxSubmission init delay: 60s default (TxSubmissionInitDelay 60)
- Mini-protocol error kills entire mux connection (MiniProtocolException -> monitor sets status Failed -> throws)
- Peras mini-protocols: CertDiffusion(16), VoteDiffusion(17) — V16+ only
