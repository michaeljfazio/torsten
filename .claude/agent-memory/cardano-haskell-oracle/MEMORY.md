# Cardano Haskell Oracle - Agent Memory

## Key Reference Files
- Conway UTXO rules: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Utxo.hs`
- Conway UTXOW rules: `...Rules/Utxow.hs`
- Conway UTXOS (Phase-2): `...Rules/Utxos.hs`
- Conway LEDGER: `...Rules/Ledger.hs` (order: CERTS→GOV→UTXOW)
- Conway BBODY: `...Rules/Bbody.hs`
- Conway CERTS: `...Rules/Certs.hs` (sequential L→R processing)
- Conway DELEG: `...Rules/Deleg.hs`
- Conway GovCert: `...Rules/GovCert.hs`
- Conway GOV: `...Rules/Gov.hs` (19 predicate failures for proposals/votes)
- Conway Ratify: `...Rules/Ratify.hs`
- Conway Enact: `...Rules/Enact.hs`
- Conway Epoch: `...Rules/Epoch.hs`
- Conway NewEpoch: `...Rules/NewEpoch.hs`
- Conway PParams: `...Conway/PParams.hs` — PParams=array(31), PParamsUpdate=map(keys 0-33)
- Shelley Rewards: `shelley/impl/src/.../Shelley/Rewards.hs`
- maxPool function: `libs/cardano-ledger-core/src/.../State/SnapShots.hs`
- Pool desirability: `shelley/impl/src/.../Shelley/PoolRank.hs`

## Topic Files
- [conway-validation-rules.md](conway-validation-rules.md) - Complete validation rules, predicate failures, reward formula, epoch transition order
- [n2n-protocols.md](n2n-protocols.md) - N2N protocol reference: mini-protocol IDs, CBOR/CDDL encodings, version negotiation, time limits, queue limits
- [n2c-protocol-details.md](n2c-protocol-details.md) - N2C protocol: all 4 mini-protocols, message tags, 40 query types with CBOR tags, wire format
- [vrf-input-construction.md](vrf-input-construction.md) - VRF seed construction: TPraos vs Praos, mkInputVRF, domain separation, Torsten bugs
- [pparams-cbor-encoding.md](pparams-cbor-encoding.md) - PParams array(31) encoding, PParamsUpdate map encoding, nested types, field ordering
- [lsq-result-encoding.md](lsq-result-encoding.md) - MsgResult wire format, HFC success wrapper [result], era mismatch encoding
- [peer-sharing-protocol.md](peer-sharing-protocol.md) - PeerSharing mini-protocol: wire format, address encoding, policy constants, governor integration
- [gov-state-cbor-encoding.md](gov-state-cbor-encoding.md) - GetGovState (tag 24) response: ConwayGovState array(7), Proposals, GovActionState, Committee, Constitution, DRepPulsingState encoding

## N2C Key Facts
- Shelley query CBOR tags: 40 queries (0-39), see n2c-protocol-details.md
- Hard-fork wrapping: QueryIfCurrent=[tag 0], QueryAnytime=[tag 1], QueryHardFork=[tag 2]
- N2C mini-protocol IDs: Handshake=0, ChainSync=5, TxSubmission=6, StateQuery=7, TxMonitor=12
- N2C ChainSync sends full blocks (not headers), wrapped as [era_id, CBOR_tag_24(block_bytes)]
- NodeToClientVersion V16-V19=QueryVersion2, V20-V23=QueryVersion3

## ouroboros-network Repo Structure (main branch)
- Protocol types: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/<Proto>/Type.hs`
- Protocol codecs: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/<Proto>/Codec.hs`
- Cardano N2N versions: `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs`
- CDDL specs: `cardano-diffusion/protocols/cddl/specs/`
- Diffusion config: `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs`
- KeepAlive client: `ouroboros-network/lib/Ouroboros/Network/KeepAlive.hs`

## N2N Protocol Status (cardano-node 10.6.2)
- Active versions: V14 (Plomin HF, mandatory), V15 (SRV DNS)
- Versions 7-13 obsolete
- Mini-protocol IDs: Handshake=0, ChainSync=2, BlockFetch=3, TxSubmission=4, KeepAlive=8, PeerSharing=10
