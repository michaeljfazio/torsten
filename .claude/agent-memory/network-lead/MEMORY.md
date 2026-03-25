# Network Lead Agent Memory

## Protocol Compliance

- [credential-type-discrimination.md](credential-type-discrimination.md) — How credential_type (0=KeyHash, 1=Script) is tracked and served in N2C query responses. Covers the HashSet solution for lost type info, the DRep special case (DRepRegistration stores full Credential), and snapshot version bump.

## Diagnostics

- [n2n-chainsync-server-direction-bug.md](n2n-chainsync-server-direction-bug.md) — Root cause of Haskell cardano-node ChainSync terminating in <2ms: TxSubmission2 role inversion deadlock in InitiatorAndResponder mode. Server waits for MsgInit but Haskell waits for MsgRequestTxIds. Direction-unaware dispatch is a secondary issue.
