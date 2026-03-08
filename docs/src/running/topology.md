# Topology

The topology file defines the peers that the node connects to. Torsten supports the full cardano-node 10.x+ P2P topology format.

## Topology File Format

```json
{
  "bootstrapPeers": [
    { "address": "backbone.cardano.iog.io", "port": 3001 },
    { "address": "backbone.mainnet.cardanofoundation.org", "port": 3001 },
    { "address": "backbone.mainnet.emurgornd.com", "port": 3001 }
  ],
  "localRoots": [
    {
      "accessPoints": [
        { "address": "192.168.1.100", "port": 3001 }
      ],
      "advertise": false,
      "hotValency": 1,
      "warmValency": 2,
      "trustable": true
    }
  ],
  "publicRoots": [
    {
      "accessPoints": [
        { "address": "relays-new.cardano-mainnet.iohk.io", "port": 3001 }
      ],
      "advertise": false
    }
  ],
  "useLedgerAfterSlot": 177724800
}
```

## Peer Categories

### Bootstrap Peers

Trusted peers from founding organizations, used during initial sync. These are the first peers the node contacts when starting.

```json
"bootstrapPeers": [
  { "address": "backbone.cardano.iog.io", "port": 3001 }
]
```

Set to `null` or an empty array to disable bootstrap peers:

```json
"bootstrapPeers": null
```

### Local Roots

Peers the node should always maintain connections with. Typically used for:
- Your block producer (if running a relay)
- Peer arrangements with other stake pool operators
- Trusted relay nodes you operate

```json
"localRoots": [
  {
    "accessPoints": [
      { "address": "192.168.1.100", "port": 3001 }
    ],
    "advertise": true,
    "hotValency": 2,
    "warmValency": 3,
    "trustable": true,
    "behindFirewall": false,
    "diffusionMode": "InitiatorAndResponder"
  }
]
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `accessPoints` | array | required | List of `{address, port}` entries |
| `advertise` | boolean | `false` | Whether to share these peers via peer sharing protocol |
| `valency` | integer | 1 | *Deprecated.* Target number of active connections. Use `hotValency` instead |
| `hotValency` | integer | valency | Target number of hot (actively syncing) peers |
| `warmValency` | integer | hotValency+1 | Target number of warm (connected, not syncing) peers |
| `trustable` | boolean | `false` | Whether these peers are trusted for sync. Trusted peers are preferred during initial sync |
| `behindFirewall` | boolean | `false` | If `true`, the node waits for inbound connections from these peers instead of connecting outbound |
| `diffusionMode` | string | `"InitiatorAndResponder"` | Per-group diffusion mode. `"InitiatorOnly"` for unidirectional connections |

### Public Roots

Publicly known nodes (e.g., IOG relays) serving as fallback peers before the node has synced to the `useLedgerAfterSlot` threshold.

```json
"publicRoots": [
  {
    "accessPoints": [
      { "address": "relays-new.cardano-mainnet.iohk.io", "port": 3001 }
    ],
    "advertise": false
  }
]
```

### Ledger-Based Peer Discovery

After the node syncs past the `useLedgerAfterSlot` slot, it discovers peers from stake pool registrations in the ledger state. This provides decentralized peer discovery without relying on centralized relay lists.

```json
"useLedgerAfterSlot": 177724800
```

Set to a negative value or omit to disable ledger peer discovery.

### Peer Snapshot File

Optional path to a big ledger peer snapshot file for Genesis bootstrap:

```json
"peerSnapshotFile": "peer-snapshot.json"
```

## Example Topologies

### Preview Testnet Relay

```json
{
  "bootstrapPeers": [
    { "address": "preview-node.play.dev.cardano.org", "port": 3001 }
  ],
  "localRoots": [
    { "accessPoints": [], "advertise": false, "valency": 1 }
  ],
  "publicRoots": [
    { "accessPoints": [], "advertise": false }
  ],
  "useLedgerAfterSlot": 102729600
}
```

### Mainnet Relay

```json
{
  "bootstrapPeers": [
    { "address": "backbone.cardano.iog.io", "port": 3001 },
    { "address": "backbone.mainnet.cardanofoundation.org", "port": 3001 },
    { "address": "backbone.mainnet.emurgornd.com", "port": 3001 }
  ],
  "localRoots": [
    { "accessPoints": [], "advertise": false, "valency": 1 }
  ],
  "publicRoots": [
    { "accessPoints": [], "advertise": false }
  ],
  "useLedgerAfterSlot": 177724800
}
```

### Relay with Block Producer

A relay node that maintains a connection to your block producer:

```json
{
  "bootstrapPeers": [
    { "address": "backbone.cardano.iog.io", "port": 3001 },
    { "address": "backbone.mainnet.cardanofoundation.org", "port": 3001 }
  ],
  "localRoots": [
    {
      "accessPoints": [
        { "address": "10.0.0.10", "port": 3001 }
      ],
      "advertise": false,
      "hotValency": 1,
      "warmValency": 2,
      "trustable": true,
      "behindFirewall": true
    }
  ],
  "publicRoots": [
    { "accessPoints": [], "advertise": false }
  ],
  "useLedgerAfterSlot": 177724800
}
```

## SIGHUP Topology Reload

Torsten supports live topology reloading. Send a `SIGHUP` signal to the running node process, and it will re-read the topology file and update the peer manager with the new configuration:

```bash
kill -HUP $(pidof torsten-node)
```

This allows you to add or remove peers without restarting the node.
