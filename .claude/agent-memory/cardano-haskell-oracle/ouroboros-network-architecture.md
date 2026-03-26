---
name: Ouroboros Network Architecture
description: Complete networking layer architecture from ouroboros-network - mux, protocols, state machines, CDDL, connection management, peer selection
type: reference
---

## Repository Structure (ouroboros-network, main branch)

### Key paths:
- Mux: `network-mux/src/Network/Mux/`
- Protocol types: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/<Proto>/Type.hs`
- Protocol codecs: same path `/Codec.hs`
- N2N versions: `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs`
- N2C versions: `cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs`
- N2N protocol config: `cardano-diffusion/lib/Cardano/Network/NodeToNode.hs`
- N2C protocol config: `cardano-diffusion/lib/Cardano/Network/NodeToClient.hs`
- CDDL specs: `cardano-diffusion/protocols/cddl/specs/`
- Connection manager: `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/`
- Peer selection: `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/`
- Handshake: `ouroboros-network/framework/lib/Ouroboros/Network/Protocol/Handshake/`

## Mux Wire Format
- Header: 8 bytes (timestamp:u32be, protocol_id_and_dir:u16be, length:u16be)
- Direction bit: bit 15 of protocol_id field (0=initiator, 1=responder)
- Protocol number: bits 0-14
- SDU payload max: 12,288 bytes for sockets (SDUSize), 32,768 for pipes
- Batch size: 131,072 for sockets
- Max SDUs per batch: 100
- Read buffer: 131,072 bytes (Linux default)

## N2N Protocol IDs & Versions
- Active: V14 (Plomin HF), V15 (SRV), V16 (experimental, Peras)
- ChainSync=2, BlockFetch=3, TxSubmission2=4, KeepAlive=8, PeerSharing=10
- Peras (V16 only): CertDiffusion=16, VoteDiffusion=17
- Handshake=0 (separate, pre-mux)

## N2C Protocol IDs & Versions
- Active: V16-V23
- Version encoding: bit 15 set (e.g., V16 = 32784)
- ChainSync=5, TxSubmission=6, StateQuery=7, TxMonitor=9
- Handshake=0

## N2N Version Data (V14-V15)
- CBOR: [networkMagic:u32, initiatorOnly:bool, peerSharing:0|1, query:bool]
- V16 adds: perasSupport:bool as 5th element

## N2C Version Data
- CBOR: [networkMagic:uint, query:bool]

## ChainSync Pipelining
- Default: lowMark=200, highMark=300 (configurable in MiniProtocolParameters)
- Ingress queue limit: highMark * 1400 * 1.1 (safety margin)

## TxSubmission2 Policy Constants
- max_TX_SIZE = 65,540 bytes
- maxUnacknowledgedTxIds = 10
- maxNumTxIdsToRequest = 3
- txsSizeInflightPerPeer = max_TX_SIZE * 6
- txInflightMultiplicity = 2

## Ingress Queue Limits (with 10% safety margin)
- ChainSync: highMark * 1400 * 1.1
- BlockFetch: max(10 * 2,097,154, pipelineMax * 90,112) * 1.1
- TxSubmission2: maxUnacked * (44 + max_TX_SIZE) * 1.1
- KeepAlive: 1280 * 1.1 = 1408
- PeerSharing: 4 * 1440 = 5760 (no safety margin)
- N2C (all): 0xffffffff (effectively unlimited)
