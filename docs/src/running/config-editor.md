# Configuration Editor (torsten-config)

`torsten-config` is a standalone TUI tool for creating and editing Torsten configuration files interactively. It provides a full-screen terminal interface with tree navigation, inline editing, type validation, and a diff view — no need to remember field names or look up valid ranges.

## Installation

`torsten-config` is built as part of the standard workspace:

```bash
cargo build --release -p torsten-config
cp target/release/torsten-config /usr/local/bin/
```

## Commands

| Command | Description |
|---------|-------------|
| `init` | Interactively create a new configuration file |
| `edit` | Launch the full-screen TUI editor for an existing file |
| `validate` | Validate a configuration file and report all errors |
| `get` | Print the value of a single field |
| `set` | Set the value of a single field non-interactively |

### init

Create a new configuration file, guided step by step:

```bash
torsten-config init --out-file config.json
```

The init wizard prompts for the network (mainnet/preview/preprod), genesis file paths, P2P targets, and tracing options, then writes a validated JSON file.

### edit

Launch the full-screen interactive editor:

```bash
torsten-config edit config.json
```

### validate

Check a configuration file for errors without modifying it:

```bash
torsten-config validate config.json
```

Output on success:

```
config.json: OK (all fields valid)
```

Output on failure:

```
config.json: 2 error(s)
  Line 7 — TargetNumberOfActivePeers: value 200 exceeds maximum (100)
  Line 12 — MinSeverity: unknown value "Verbose" (expected: Trace, Debug, Info, Warning, Error)
```

### get / set

Non-interactive field access for scripting:

```bash
# Get a field
torsten-config get config.json TargetNumberOfActivePeers
# Output: 20

# Set a field
torsten-config set config.json TargetNumberOfActivePeers 30

# Set a nested field
torsten-config set config.json TraceOptions.TraceForge true
```

## Interactive Editor

The interactive editor (`torsten-config edit`) renders a full-screen TUI with three panes:

```
┌─ Fields ──────────────────────┬─ Value ───────────┬─ Hints ───────────────────────────┐
│ > Network Settings            │                   │                                   │
│     Network                   │ Testnet           │ Network identifier. Use "Mainnet" │
│     NetworkMagic              │ 2                 │ for mainnet or "Testnet" for      │
│     EnableP2P                 │ true              │ testnets. If omitted, defaults    │
│ > Genesis Files               │                   │ based on Network field.           │
│     ShelleyGenesisFile        │ shelley-gen...    │                                   │
│     ByronGenesisFile          │ byron-genesi...   │                                   │
│     AlonzoGenesisFile         │ alonzo-genes...   │                                   │
│     ConwayGenesisFile         │ conway-genes...   │                                   │
│ > P2P Parameters              │                   │                                   │
│     TargetNumberOfActivePeers │ 20                │                                   │
└───────────────────────────────┴───────────────────┴───────────────────────────────────┘
```

### Navigation

| Key | Action |
|-----|--------|
| Arrow Up / Down | Move between fields |
| Arrow Right / Enter | Expand a group or edit a field |
| Arrow Left / Escape | Collapse a group or cancel edit |
| `/` | Open search/filter |
| `d` | Toggle diff view |
| `Ctrl+S` | Save and exit |
| `Ctrl+Q` | Discard changes and exit |
| `?` | Toggle help overlay |

### Inline Editing

Pressing `Enter` on a field opens it for editing in place. The current value is pre-filled. Type a new value and press `Enter` to confirm or `Escape` to cancel.

Type validation runs immediately on confirmation. If the value is invalid (for example, a string where an integer is expected, or a number outside the valid range), an inline error message appears below the field. The cursor stays on the field until a valid value is entered or the edit is cancelled.

### Tuning Hints

The right-hand pane shows contextual hints for the selected field, including:

- A description of what the field controls
- The valid type and range
- Practical advice on the impact of different values

For example, `TargetNumberOfActivePeers` shows advice on the trade-off between connectivity and bandwidth, and notes that values above 50 are rarely beneficial for relay nodes.

### Search and Filter

Press `/` to open the search bar. Typing narrows the visible fields to those whose names match the query. Press `Escape` to clear the filter and return to the full tree.

### Diff View

Press `d` to toggle the diff view, which shows a side-by-side comparison of the original file and your pending changes. Fields with modified values are highlighted. Use this before saving to confirm your edits.

## Scripted Workflows

`torsten-config` can be used in deployment scripts for automated configuration management:

```bash
#!/usr/bin/env bash
# Example: configure a relay node for preview testnet

CONFIG="config/preview-config.json"

torsten-config init --out-file "$CONFIG" \
  --network Testnet \
  --network-magic 2 \
  --shelley-genesis shelley-genesis.json \
  --byron-genesis byron-genesis.json \
  --alonzo-genesis alonzo-genesis.json \
  --conway-genesis conway-genesis.json

torsten-config set "$CONFIG" TargetNumberOfActivePeers 20
torsten-config set "$CONFIG" TargetNumberOfEstablishedPeers 40
torsten-config set "$CONFIG" TargetNumberOfKnownPeers 100
torsten-config validate "$CONFIG"
```
