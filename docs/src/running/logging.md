# Logging

Dugite uses the [tracing](https://docs.rs/tracing) ecosystem for structured logging. It supports multiple output targets, structured and human-readable formats, log rotation for file output, and fine-grained level control.

## Output Formats

Dugite supports two log formats, selectable via the `--log-format` flag:

### Text (default)

Human-readable compact output with timestamps, level, target module, and structured fields:

```bash
dugite-node run --log-format text ...
```

```
2026-03-12T12:34:56.789Z  INFO dugite_node::node: Syncing progress="95.42%" epoch=512 block=11283746 tip=11300000 remaining=16254 speed="312 blk/s" utxos=15234892
2026-03-12T12:34:56.790Z  INFO dugite_node::node: Peer connected peer=1.2.3.4:3001 rtt_ms=42
```

### JSON

Structured JSON output, one object per line. Ideal for log aggregation systems (ELK, Loki, Datadog):

```bash
dugite-node run --log-format json ...
```

```json
{"timestamp":"2026-03-12T12:34:56.789Z","level":"INFO","target":"dugite_node::node","fields":{"message":"Syncing","progress":"95.42%","epoch":512,"block":11283746}}
```

## Output Targets

Dugite can log to one or more output targets simultaneously using the `--log-output` flag. You can specify this flag multiple times to enable multiple targets:

```bash
# Stdout only (default)
dugite-node run --log-output stdout ...

# File only
dugite-node run --log-output file ...

# Both stdout and file
dugite-node run --log-output stdout --log-output file ...

# Systemd journal (requires journald feature)
dugite-node run --log-output journald ...
```

### Stdout

The default output target. Logs are written to standard output with ANSI color codes when the output is a terminal. Colors can be disabled with `--log-no-color`.

### File

Logs are written to rotating log files in the directory specified by `--log-dir` (default: `logs/`). The rotation strategy is configured with `--log-file-rotation`:

| Strategy | Description |
|----------|-------------|
| `daily` | Rotate log files daily (default) |
| `hourly` | Rotate log files every hour |
| `never` | Write to a single `dugite.log` file with no rotation |

```bash
dugite-node run \
  --log-output file \
  --log-dir /var/log/dugite \
  --log-file-rotation daily \
  ...
```

File output uses non-blocking I/O with buffered writes. The buffer is flushed automatically on shutdown.

### Journald

Native systemd journal integration. This requires building Dugite with the `journald` feature:

```bash
cargo build --release --features journald
```

Then run with:

```bash
dugite-node run --log-output journald ...
```

View logs with `journalctl`:

```bash
journalctl -u dugite-node -f
journalctl -u dugite-node --since "1 hour ago"
```

## Log Levels

The log level can be set via the `--log-level` CLI flag or the `RUST_LOG` environment variable. If both are set, `RUST_LOG` takes priority.

```bash
# Via CLI flag
dugite-node run --log-level debug ...

# Via environment variable (takes priority)
RUST_LOG=debug dugite-node run ...
```

Available levels (from most to least verbose):

| Level | Description |
|-------|-------------|
| `trace` | Very detailed internal diagnostics |
| `debug` | Internal operations: genesis loading, storage ops, network handshakes, epoch transitions |
| `info` | Operator-relevant events: sync progress, peer connections, block production (default) |
| `warn` | Potential issues: stale snapshots, replay failures |
| `error` | Errors that may affect node operation |

### Per-Crate Filtering

Use `RUST_LOG` for fine-grained control over which components produce output:

```bash
# Debug only for specific crates
RUST_LOG=dugite_network=debug,dugite_consensus=debug dugite-node run ...

# Trace storage operations, debug everything else
RUST_LOG=dugite_storage=trace,debug dugite-node run ...

# Silence noisy crates
RUST_LOG=info,dugite_network=warn dugite-node run ...
```

## CLI Reference

All logging flags are shared between the `run` and `mithril-import` subcommands:

| Flag | Default | Description |
|------|---------|-------------|
| `--log-output` | `stdout` | Log output target: `stdout`, `file`, or `journald`. Can be specified multiple times. |
| `--log-format` | `text` | Log format: `text` (human-readable) or `json` (structured). |
| `--log-level` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error`. Overridden by `RUST_LOG`. |
| `--log-dir` | `logs` | Directory for log files (used with `--log-output file`) |
| `--log-file-rotation` | `daily` | Log file rotation: `daily`, `hourly`, or `never` |
| `--log-no-color` | `false` | Disable ANSI colors in stdout output |

## Production Recommendations

For production deployments with log aggregation:

```bash
dugite-node run \
  --log-output file \
  --log-output journald \
  --log-format json \
  --log-dir /var/log/dugite \
  --log-file-rotation daily \
  ...
```

This configuration:
- Writes structured JSON logs to systemd journal for `journalctl` integration
- Writes rotated JSON log files for archival and ingestion by log aggregators
- JSON format ensures all structured fields are machine-parseable

For human operators monitoring the console:

```bash
dugite-node run --log-output stdout --log-format text ...
```

For containerized deployments (Docker, Kubernetes), stdout with JSON is ideal since the container runtime captures output and log drivers can parse the structured format:

```bash
dugite-node run --log-output stdout --log-format json ...
```
