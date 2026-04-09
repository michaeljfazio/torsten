use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct QueryCmd {
    #[command(subcommand)]
    command: QuerySubcommand,
}

#[derive(Subcommand, Debug)]
enum QuerySubcommand {
    /// Query the current tip
    Tip {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query UTxOs at an address or by specific transaction input(s).
    ///
    /// Use --address to query all UTxOs at a bech32 address, or --tx-in to query
    /// one or more specific UTxOs by transaction input reference (tx_hash#index).
    /// Both flags may be repeated. If neither flag is provided, --address is required.
    Utxo {
        /// Bech32 address to query UTxOs for
        #[arg(long)]
        address: Option<String>,
        /// Specific UTxO input reference to query (format: tx_hash#index). May be repeated.
        #[arg(long = "tx-in", value_name = "TX_HASH#INDEX")]
        tx_in: Vec<String>,
        /// Return the entire UTxO set (warning: very large on mainnet)
        #[arg(long)]
        whole: bool,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query protocol parameters
    ProtocolParameters {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake distribution
    StakeDistribution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake address info
    StakeAddressInfo {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query governance state (Conway era)
    GovState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query DRep state (Conway era)
    DrepState {
        #[arg(long)]
        drep_key_hash: Option<String>,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query committee state (Conway era)
    CommitteeState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query the transaction mempool
    TxMempool {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        /// Subcommand: info, next-tx, has-tx
        #[arg(default_value = "info")]
        subcmd: String,
        /// Transaction ID (for has-tx)
        #[arg(long)]
        tx_id: Option<String>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query registered stake pools
    StakePools {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query stake snapshots (mark/set/go)
    StakeSnapshot {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
        /// Filter by pool ID (bech32 or hex)
        #[arg(long)]
        stake_pool_id: Option<String>,
    },
    /// Query stake pool parameters
    PoolParams {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
        /// Pool ID to query (bech32 or hex)
        #[arg(long)]
        stake_pool_id: Option<String>,
    },
    /// Query treasury and reserves (account state)
    Treasury {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query the current constitution
    Constitution {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query ratification state (enacted/expired proposals, delayed flag)
    RatifyState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Get the slots the node is expected to mint a block in
    LeadershipSchedule {
        /// Path to the node socket. Overrides CARDANO_NODE_SOCKET_PATH env.
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        /// Use the mainnet magic id
        #[arg(long, group = "network")]
        mainnet: bool,
        /// Specify a testnet magic id
        #[arg(long, group = "network")]
        testnet_magic: Option<u64>,
        /// Shelley genesis filepath
        #[arg(long)]
        genesis: PathBuf,
        /// Stake pool ID (hex-encoded hash)
        #[arg(long)]
        stake_pool_id: Option<String>,
        /// Filepath of the cold verification key
        #[arg(long)]
        cold_verification_key_file: Option<PathBuf>,
        /// Input filepath of the VRF signing key
        #[arg(long)]
        vrf_signing_key_file: PathBuf,
        /// Get the leadership schedule for the current epoch
        #[arg(long, group = "epoch_choice")]
        current: bool,
        /// Get the leadership schedule for the following epoch
        #[arg(long, group = "epoch_choice")]
        next: bool,
        /// Format output to JSON (default)
        #[arg(long)]
        output_json: bool,
        /// Format output to TEXT
        #[arg(long)]
        output_text: bool,
        /// Optional output file. Default is stdout.
        #[arg(long)]
        out_file: Option<PathBuf>,
    },
    /// Convert a UTC timestamp to a Cardano slot number.
    ///
    /// Queries the node for its era history (GetInterpreter) to obtain the slot
    /// config (zero_time, zero_slot, slot_length_ms) and then computes:
    ///
    ///   slot = zero_slot + (utc_unix_secs - zero_time_secs) * 1000 / slot_length_ms
    ///
    /// The UTC timestamp must be provided as a positional argument in ISO-8601
    /// format with a Z suffix, e.g. "2024-01-15T00:00:00Z".
    #[command(name = "slot-number")]
    SlotNumber {
        /// UTC timestamp in ISO-8601 format, e.g. "2024-01-15T00:00:00Z"
        utc_time: String,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Show KES key period information for an operational certificate.
    ///
    /// Reads the opcert file, decodes the KES counter and issue period, then
    /// queries the node for the current KES period via GetCurrentKESPeriod
    /// (Shelley query tag 31).  Prints a summary including whether the cert
    /// is currently valid, expired, or not-yet-valid.
    ///
    /// Matches the output of `cardano-cli query kes-period-info`.
    #[command(name = "kes-period-info")]
    KesPeriodInfo {
        /// Operational certificate file (text envelope)
        #[arg(long)]
        op_cert_file: PathBuf,
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query the ledger state (debug endpoint)
    LedgerState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
    /// Query the protocol state (debug endpoint)
    ProtocolState {
        #[arg(long, default_value = "node.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        out_file: Option<PathBuf>,
        #[arg(long)]
        testnet_magic: Option<u64>,
    },
}

/// Convert an f64 active-slot coefficient to the nearest exact rational p/q.
///
/// Tries common denominators first (1, 2, 4, 5, 10, 20, …) before falling
/// back to 1 000 000.  The result is in lowest terms.  This matches the
/// f64_to_rational helper inside dugite-crypto's leader_check module, so
/// callers that pass `f = 0.05` will always get `(1, 20)` rather than some
/// floating-point approximation.
#[allow(dead_code)]
fn f64_to_rational_approx(value: f64) -> (u64, u64) {
    for den in [1u64, 2, 4, 5, 10, 20, 25, 50, 100, 200, 1000, 10000] {
        let num = (value * den as f64).round() as u64;
        let reconstructed = num as f64 / den as f64;
        if (reconstructed - value).abs() < 1e-15 {
            let g = gcd_u64(num, den);
            return (num / g, den / g);
        }
    }
    let den = 1_000_000u64;
    let num = (value * den as f64).round() as u64;
    let g = gcd_u64(num, den);
    (num / g, den / g)
}

#[allow(dead_code)]
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Map era index to era name
/// Parse a simple ISO-8601 UTC timestamp to Unix seconds.
/// Supports "YYYY-MM-DDThh:mm:ssZ" format.
fn parse_iso8601_to_unix(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u64 = parts[0].parse().ok()?;
    let month: u64 = parts[1].parse().ok()?;
    let day: u64 = parts[2].parse().ok()?;

    let time_parts: Vec<&str> = time.split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hours: u64 = time_parts[0].parse().ok()?;
    let minutes: u64 = time_parts[1].parse().ok()?;
    let seconds: u64 = time_parts[2].parse().ok()?;

    // Days from year 1970 to start of given year
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    // Days from start of year to start of given month
    let is_leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_months: [u64; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for &d in &days_in_months[..(month - 1) as usize] {
        days += d;
    }
    days += day - 1;

    Some(days * 86400 + hours * 3600 + minutes * 60 + seconds)
}

fn era_name(era: u32) -> &'static str {
    match era {
        0 => "Byron",
        1 => "Shelley",
        2 => "Allegra",
        3 => "Mary",
        4 => "Alonzo",
        5 => "Babbage",
        6 => "Conway",
        _ => "Unknown",
    }
}

/// Connect to the node, perform handshake, and acquire state
async fn connect_and_handshake(
    socket_path: &std::path::Path,
    testnet_magic: Option<u64>,
) -> Result<dugite_network::N2CClient> {
    let magic = testnet_magic.unwrap_or(764824073);

    let client = dugite_network::N2CClient::connect(socket_path, magic)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Cannot connect to node socket '{}': {e}\nIs the node running?",
                socket_path.display()
            )
        })?;

    Ok(client)
}

async fn connect_and_acquire(
    socket_path: &std::path::Path,
    testnet_magic: Option<u64>,
) -> Result<dugite_network::N2CClient> {
    let mut client = connect_and_handshake(socket_path, testnet_magic).await?;

    client
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to acquire state: {e}"))?;

    Ok(client)
}

/// Release state and disconnect
async fn release_and_done(client: &mut dugite_network::N2CClient) {
    client.release().await.ok();
    client.done().await.ok();
}

// ── Task 2: Protocol State CBOR Parser ──────────────────────────────────────

/// Nonce values extracted from PraosState (protocol state query).
#[allow(dead_code)]
struct PraosNonces {
    /// The epoch nonce used for VRF leader checks in the current epoch.
    epoch_nonce: [u8; 32],
    /// The candidate nonce (becomes the epoch nonce for the next epoch).
    candidate_nonce: [u8; 32],
}

/// Parse a Nonce CBOR value: array(1)[0] = NeutralNonce, array(2)[1, bytes32] = Nonce.
/// Returns 32 zero bytes for NeutralNonce.
#[allow(dead_code)]
fn parse_cbor_nonce(d: &mut minicbor::Decoder<'_>) -> Result<[u8; 32]> {
    let len = d
        .array()?
        .ok_or_else(|| anyhow::anyhow!("Expected definite-length nonce array"))?;
    let tag = d.u8()?;
    if tag == 0 && len == 1 {
        Ok([0u8; 32])
    } else if tag == 1 && len == 2 {
        let bytes = d.bytes()?;
        let mut nonce = [0u8; 32];
        if bytes.len() == 32 {
            nonce.copy_from_slice(bytes);
        }
        Ok(nonce)
    } else {
        anyhow::bail!("Invalid nonce encoding: tag={tag}, len={len}");
    }
}

/// Parse the raw MsgResult CBOR from `query_chain_dep_state()` and extract nonces.
///
/// Wire format: MsgResult [tag, HFC [array(2)[version=0, array(7)[...PraosState fields...]]]]
#[allow(dead_code)]
fn parse_protocol_state_nonces(raw: &[u8]) -> Result<PraosNonces> {
    let mut d = minicbor::Decoder::new(raw);

    // MsgResult: array(2)[4, payload]
    // Haskell LocalStateQuery codec: MsgResult tag = 4 (not 6, which is MsgReAcquire)
    let _ = d.array();
    let tag = d.u32()?;
    if tag != 4 {
        anyhow::bail!("Protocol state query failed: expected MsgResult tag 4, got {tag}");
    }

    // Strip HFC EitherMismatch success wrapper: array(1)[payload]
    // Discriminant is array LENGTH (1=success, 2=mismatch), not a leading integer.
    let pos = d.position();
    if let Ok(Some(1)) = d.array() {
        // Success: wrapper consumed
    } else {
        d.set_position(pos);
    }

    // Versioned wrapper: array(2)[0, payload]
    let _ = d.array();
    let version = d.u8()?;
    if version != 0 {
        anyhow::bail!("Unexpected PraosState version: {version}");
    }

    // PraosState: array(7)
    let _ = d.array();

    // [0] lastSlot (WithOrigin) — skip
    let slot_len = d
        .array()?
        .ok_or_else(|| anyhow::anyhow!("Expected definite-length slot array"))?;
    let _ = d.u8();
    if slot_len == 2 {
        let _ = d.u64();
    }

    // [1] ocertCounters (Map) — skip
    let map_len = d.map()?.unwrap_or(0);
    for _ in 0..map_len {
        let _ = d.bytes();
        let _ = d.u64();
    }

    // [2] evolvingNonce — skip
    let _ = parse_cbor_nonce(&mut d)?;

    // [3] candidateNonce
    let candidate_nonce = parse_cbor_nonce(&mut d)?;

    // [4] epochNonce
    let epoch_nonce = parse_cbor_nonce(&mut d)?;

    Ok(PraosNonces {
        epoch_nonce,
        candidate_nonce,
    })
}

// ── Task 3: Stake Snapshot Parser ───────────────────────────────────────────

/// Pool stake and total active stake from a stake snapshot query.
#[allow(dead_code)]
struct PoolStakeInfo {
    /// Pool's delegated stake (lovelace) from the "set" snapshot (current) or "mark" (next).
    pool_stake: u64,
    /// Total active stake across all pools (lovelace).
    total_active_stake: u64,
}

/// Parse raw MsgResult from `query_stake_snapshot()` and extract a specific pool's stake.
///
/// Wire format: MsgResult [tag, HFC [array(4) [pool_map, mark_total, set_total, go_total]]]
/// pool_map: Map<pool_hash(28B), array(3)[mark, set, go]>
///
/// `use_mark` controls which snapshot to use: true = mark (for --next), false = set (for --current).
#[allow(dead_code)]
fn parse_stake_for_pool(raw: &[u8], pool_id_hex: &str, use_mark: bool) -> Result<PoolStakeInfo> {
    let mut d = minicbor::Decoder::new(raw);

    // MsgResult: array(2)[4, payload]
    // Haskell LocalStateQuery: MsgResult tag = 4
    let _ = d.array();
    let tag = d.u32()?;
    if tag != 4 {
        anyhow::bail!("Stake snapshot query failed: expected MsgResult tag 4, got {tag}");
    }

    // Strip HFC EitherMismatch success wrapper: array(1)[result]
    let pos = d.position();
    if let Ok(Some(1)) = d.array() {
        // Success: consumed
    } else {
        d.set_position(pos);
    }

    // array(4) [pool_map, mark_total, set_total, go_total]
    let _ = d.array();

    // pool_map: Map<pool_hash(28B), array(3)[mark, set, go]>
    let pool_count = d.map()?.unwrap_or(0);
    let mut pool_stake: Option<u64> = None;
    let pool_id_lower = pool_id_hex.to_lowercase();

    for _ in 0..pool_count {
        let pool_hash = hex::encode(d.bytes().unwrap_or(&[]));
        let _ = d.array();
        let mark = d.u64().unwrap_or(0);
        let set = d.u64().unwrap_or(0);
        let _go = d.u64().unwrap_or(0);

        if pool_hash == pool_id_lower {
            pool_stake = Some(if use_mark { mark } else { set });
        }
    }

    let total_mark = d.u64().unwrap_or(0);
    let total_set = d.u64().unwrap_or(0);
    let _total_go = d.u64().unwrap_or(0);

    let total_active_stake = if use_mark { total_mark } else { total_set };

    let pool_stake = pool_stake.ok_or_else(|| {
        anyhow::anyhow!(
            "Pool {} not found in stake snapshot. Is the pool registered and has delegated stake?",
            pool_id_hex
        )
    })?;

    Ok(PoolStakeInfo {
        pool_stake,
        total_active_stake,
    })
}

// ── Task 4: Genesis Reader Helper ───────────────────────────────────────────

/// Parameters extracted from the Shelley genesis file needed for leadership schedule.
#[allow(dead_code)]
struct ShelleyGenesisParams {
    active_slots_coeff: f64,
    epoch_length: u64,
    slot_length: u64,
    system_start_unix: u64,
}

/// Read the shelley genesis file and extract timing parameters.
#[allow(dead_code)]
fn read_shelley_genesis(path: &std::path::Path) -> Result<ShelleyGenesisParams> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read genesis file '{}': {e}", path.display()))?;
    let genesis: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| anyhow::anyhow!("Invalid genesis JSON: {e}"))?;

    let active_slots_coeff = genesis["activeSlotsCoeff"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("Missing activeSlotsCoeff in genesis"))?;

    let epoch_length = genesis["epochLength"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing epochLength in genesis"))?;

    let slot_length = genesis["slotLength"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing slotLength in genesis"))?;

    let system_start_str = genesis["systemStart"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing systemStart in genesis"))?;

    let system_start_unix = parse_iso8601_to_unix(system_start_str)
        .ok_or_else(|| anyhow::anyhow!("Cannot parse systemStart: {system_start_str}"))?;

    Ok(ShelleyGenesisParams {
        active_slots_coeff,
        epoch_length,
        slot_length,
        system_start_unix,
    })
}

// ── Task 5: Pool ID from Cold Key ───────────────────────────────────────────

/// Derive pool ID (hex) from a cold verification key file.
///
/// The key file is a Cardano text envelope with `cborHex` containing a CBOR-wrapped
/// Ed25519 public key (32 bytes). The pool ID is Blake2b-224 of the raw key bytes.
#[allow(dead_code)]
fn pool_id_from_cold_vkey(path: &std::path::Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read cold key file '{}': {e}", path.display()))?;
    let env: serde_json::Value = serde_json::from_str(&content)?;
    let cbor_hex = env["cborHex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing cborHex in cold verification key file"))?;
    let cbor_bytes = hex::decode(cbor_hex)?;

    // Strip CBOR wrapper (5820 prefix for 32-byte bytestring)
    let key_bytes = if cbor_bytes.len() > 2 && cbor_bytes[0] == 0x58 && cbor_bytes[1] == 0x20 {
        &cbor_bytes[2..]
    } else if cbor_bytes.len() > 1 && (cbor_bytes[0] & 0xe0) == 0x40 {
        &cbor_bytes[1..]
    } else {
        &cbor_bytes
    };

    if key_bytes.len() != 32 {
        anyhow::bail!(
            "Cold verification key must be 32 bytes, got {}",
            key_bytes.len()
        );
    }

    // Pool ID = Blake2b-224(vkey)
    use blake2::digest::{consts::U28, Digest};
    type Blake2b224 = blake2::Blake2b<U28>;
    let hash = Blake2b224::digest(key_bytes);
    Ok(hex::encode(hash))
}

// ── Task 6: Slot-to-UTC helper ──────────────────────────────────────────────

/// Returns true for leap years using the Gregorian calendar rule.
fn is_leap_year(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Convert a Cardano slot number to a UTC timestamp string ("YYYY-MM-DDThh:mm:ssZ").
///
/// `system_start_unix` is the Unix timestamp (seconds since 1970) of slot 0.
/// `slot_length` is the duration of each slot in seconds (usually 1 for mainnet/testnet).
#[allow(dead_code)]
fn slot_to_utc(slot: u64, system_start_unix: u64, slot_length: u64) -> String {
    let unix_secs = system_start_unix + slot * slot_length;
    let secs_per_day = 86_400u64;
    let days = unix_secs / secs_per_day;
    let day_secs = unix_secs % secs_per_day;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Walk forward from 1970 to find the calendar year.
    let mut y = 1970i64;
    let mut remaining = days;
    loop {
        let year_days: u64 = if is_leap_year(y as u64) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        y += 1;
    }

    // Find month and day within the year.
    let leap = is_leap_year(y as u64);
    let month_days: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md {
            m = i;
            break;
        }
        remaining -= md;
    }
    let d = remaining + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        d,
        hours,
        minutes,
        seconds
    )
}

/// Parse and print a UTxO query response in the cardano-cli tabular format.
///
/// The `raw` bytes are a full LocalStateQuery MsgResult payload:
///   `[4, [Map<[tx_hash, index], TransactionOutput>]]`
/// where the inner array(1) is the HFC success wrapper.
///
/// Output format matches cardano-cli `query utxo` exactly:
/// ```
///                            TxHash                                 TxIx        Amount
/// --------------------------------------------------------------------------------------
/// <64-char-hash>                                                        0        5000000 lovelace + TxOutDatumNone
/// ```
/// Drain a CBOR map entry by entry using a callback.
///
/// Handles both definite-length maps (`map_len = Some(n)`) and indefinite-length
/// maps (`map_len = None`, terminated by a CBOR "break" code).  minicbor
/// returns `None` from `decoder.map()` for indefinite-length maps; callers that
/// use `unwrap_or(0)` silently skip all entries.  This helper encapsulates the
/// correct iteration pattern for both cases.
fn decode_map_entries<F>(decoder: &mut minicbor::Decoder, mut f: F) -> Result<()>
where
    F: FnMut(&mut minicbor::Decoder) -> Result<()>,
{
    match decoder.map()? {
        Some(n) => {
            // Definite-length map: iterate exactly n times.
            for _ in 0..n {
                f(decoder)?;
            }
        }
        None => {
            // Indefinite-length map: iterate until CBOR break (datatype Undefined
            // signals the end in minicbor's streaming API).
            //
            // minicbor does not expose a "peek" API, so we detect the break by
            // attempting to decode each key as a `u32` and stopping when a
            // datatype error is returned (which happens on the break code 0xff).
            loop {
                let key_pos = decoder.position();
                match decoder.datatype() {
                    Ok(minicbor::data::Type::Break) => {
                        decoder.skip().ok(); // consume the break byte
                        break;
                    }
                    Ok(_) => {
                        decoder.set_position(key_pos);
                        f(decoder)?;
                    }
                    Err(_) => break,
                }
            }
        }
    }
    Ok(())
}

fn print_utxo_result(raw: &[u8]) -> Result<()> {
    // Parse MsgResult [4, [utxo_map]]
    // Haskell LocalStateQuery codec: MsgResult = array(2)[4, payload]
    // HFC EitherMismatch success = array(1)[result]  (discriminant is array LENGTH not a tag)
    let mut decoder = minicbor::Decoder::new(raw);
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        anyhow::bail!("Expected MsgResult tag 4, got {tag}");
    }

    // Strip HFC EitherMismatch success wrapper.
    // Haskell encodes success as array(1)[result], mismatch as array(2)[era1, era2].
    // Discriminant is array LENGTH, not a leading integer tag.
    let pos = decoder.position();
    if let Ok(Some(1)) = decoder.array() {
        // Success: array(1) wrapper consumed, result follows directly
    } else {
        // Not wrapped (top-level or QueryAnytime result) — reset
        decoder.set_position(pos);
    }

    // cardano-cli header: TxHash is 64 chars wide, TxIx 4 chars, Amount fills the rest
    println!(
        "{:<64} {:<4}         Amount",
        "                           TxHash", "TxIx"
    );
    println!("{}", "-".repeat(86));

    // UTxO result: CBOR Map<[tx_hash, index], TransactionOutput>
    //
    // Use decode_map_entries to handle both definite- and indefinite-length
    // outer maps.  The Dugite N2C server currently emits definite-length maps,
    // but a Haskell node may emit indefinite-length maps; both must work so
    // `dugite-cli` is usable against either node.
    decode_map_entries(&mut decoder, |dec| {
        // Key: [tx_hash_bytes, output_index]
        let _ = dec.array(); // consume array(2)
        let tx_hash = hex::encode(dec.bytes().unwrap_or(&[]));
        let output_index = dec.u32().unwrap_or(0);

        // Value: PostAlonzo TransactionOutput as CBOR map
        // {0: address_bytes, 1: value, 2?: datum_option, 3?: script_ref}
        // Value field 1: plain integer for ADA-only, [coin, multiasset_map] for multi-asset.
        let mut lovelace = 0u64;
        let mut has_datum = false;

        decode_map_entries(dec, |inner| {
            let key = inner.u32().unwrap_or(999);
            match key {
                0 => {
                    // address bytes — skip
                    inner.skip().ok();
                }
                1 => {
                    // value: either integer (ADA-only) or [coin, multiasset_map]
                    let val_pos = inner.position();
                    if let Ok(coin) = inner.u64() {
                        lovelace = coin;
                    } else {
                        inner.set_position(val_pos);
                        if let Ok(Some(_)) = inner.array() {
                            lovelace = inner.u64().unwrap_or(0);
                            inner.skip().ok(); // skip multiasset map
                        }
                    }
                }
                2 => {
                    // datum_option present
                    has_datum = true;
                    inner.skip().ok();
                }
                3 => {
                    // script_ref
                    inner.skip().ok();
                }
                _ => {
                    inner.skip().ok();
                }
            }
            Ok(())
        })?;

        // cardano-cli amount format: "<lovelace> lovelace + TxOutDatumNone"
        // (or "+ TxOutDatumInline ..." for inline datums)
        let datum_suffix = if has_datum {
            "+ TxOutDatumInline"
        } else {
            "+ TxOutDatumNone"
        };
        println!("{tx_hash:<64} {output_index:<4} {lovelace} lovelace {datum_suffix}");
        Ok(())
    })?;

    Ok(())
}

impl QueryCmd {
    pub fn run(self) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(self.run_async())
    }

    async fn run_async(self) -> Result<()> {
        match self.command {
            QuerySubcommand::Tip {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;

                let epoch = client.query_epoch().await.unwrap_or(0);
                let era = client.query_era().await.unwrap_or(6);
                let block_no = client.query_block_no().await.unwrap_or(tip.block_no);

                // Query system start time from the node for accurate sync progress
                let system_start_str = client.query_system_start().await.ok();

                release_and_done(&mut client).await;

                let hash_hex = hex::encode(&tip.hash);
                let era_str = era_name(era);

                // Calculate sync progress using the node's system start time
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let genesis_unix = system_start_str
                    .as_deref()
                    .and_then(parse_iso8601_to_unix)
                    .unwrap_or(match testnet_magic {
                        Some(2) => 1_666_656_000u64, // Preview
                        Some(1) => 1_654_041_600u64, // Preprod
                        _ => 1_596_059_091u64,       // Mainnet Shelley
                    });
                let elapsed_secs = now_secs.saturating_sub(genesis_unix);
                let expected_tip_slot = elapsed_secs; // 1 slot ≈ 1 second (Shelley+)
                let sync_progress = if expected_tip_slot > 0 {
                    (tip.slot as f64 / expected_tip_slot as f64 * 100.0).min(100.0)
                } else {
                    100.0
                };

                // Epoch slot position.
                // Epoch length: Preview=86400 (1 day), Preprod/Mainnet=432000 (5 days).
                // Compute slot-in-epoch from (slot - byron_slots) % epoch_length.
                // Byron: mainnet 21600 epochs × 21600 slots/epoch = 466560000 slots total.
                // For simplicity we use the well-known epoch-length per network.
                let epoch_length: u64 = match testnet_magic {
                    Some(2) => 86400,  // Preview
                    Some(1) => 432000, // Preprod
                    _ => 432000,       // Mainnet (and others)
                };
                // Byron offset in slots (only mainnet has Byron blocks in current tip).
                // We derive slot-in-epoch from tip.slot and the known epoch number.
                // epoch_length * epoch gives the slot at start of epoch (approximately).
                // Use the simple modular formula which holds once we are in Shelley+.
                let slot_in_epoch = if epoch_length > 0 {
                    tip.slot % epoch_length
                } else {
                    0
                };
                let slots_to_epoch_end = epoch_length.saturating_sub(slot_in_epoch);

                // cardano-cli 10.x JSON output format (fields in alphabetical order):
                // block, epoch, era, hash, slot, slotInEpoch, slotsToEpochEnd, syncProgress
                // NOTE: cardano-cli does NOT include a "network" field.
                println!("{{");
                println!("    \"block\": {block_no},");
                println!("    \"epoch\": {epoch},");
                println!("    \"era\": \"{era_str}\",");
                println!("    \"hash\": \"{hash_hex}\",");
                println!("    \"slot\": {},", tip.slot);
                println!("    \"slotInEpoch\": {slot_in_epoch},");
                println!("    \"slotsToEpochEnd\": {slots_to_epoch_end},");
                println!("    \"syncProgress\": \"{sync_progress:.2}\"");
                println!("}}");
                Ok(())
            }
            QuerySubcommand::Utxo {
                address,
                tx_in,
                whole,
                socket_path,
                testnet_magic,
            } => {
                // Validate: must have at least --address, --tx-in, or --whole
                if address.is_none() && tx_in.is_empty() && !whole {
                    anyhow::bail!(
                        "At least one of --address or --tx-in must be provided.\n\
                         Examples:\n  \
                         --address addr1...\n  \
                         --tx-in <txhash>#<index>"
                    );
                }

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Collect all UTxO responses into a single raw CBOR payload.
                // For --tx-in queries we issue a single GetUTxOByTxIn (tag 15) with all inputs;
                // for --address we issue GetUTxOByAddress (tag 6).
                let raw: Vec<u8> = if whole {
                    client
                        .query_utxo_whole()
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to query whole UTxO set: {e}"))?
                } else if !tx_in.is_empty() {
                    // Parse each "tx_hash#index" argument
                    let mut inputs: Vec<(Vec<u8>, u32)> = Vec::new();
                    for arg in &tx_in {
                        let (hash_str, idx_str) = arg.split_once('#').ok_or_else(|| {
                            anyhow::anyhow!(
                                "Invalid --tx-in format '{arg}': expected tx_hash#index"
                            )
                        })?;
                        let hash_bytes = hex::decode(hash_str)
                            .map_err(|e| anyhow::anyhow!("Invalid tx hash in '{arg}': {e}"))?;
                        if hash_bytes.len() != 32 {
                            anyhow::bail!(
                                "Tx hash must be 32 bytes (64 hex chars), got {} bytes in '{arg}'",
                                hash_bytes.len()
                            );
                        }
                        let index: u32 = idx_str
                            .parse()
                            .map_err(|e| anyhow::anyhow!("Invalid output index in '{arg}': {e}"))?;
                        inputs.push((hash_bytes, index));
                    }
                    client
                        .query_utxo_by_txin(&inputs)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to query UTxOs by tx-in: {e}"))?
                } else {
                    // --address mode
                    let addr = address.as_deref().unwrap();
                    let (_, addr_bytes) = bech32::decode(addr)
                        .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;
                    client
                        .query_utxo_by_address(&addr_bytes)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to query UTxOs: {e}"))?
                };

                release_and_done(&mut client).await;

                print_utxo_result(&raw)?;
                Ok(())
            }
            QuerySubcommand::ProtocolParameters {
                socket_path,
                out_file,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let json = match client.query_protocol_params().await {
                    Ok(params_json) if !params_json.is_empty() => params_json,
                    _ => {
                        // Fall back to defaults if node doesn't have params
                        let params =
                            dugite_primitives::protocol_params::ProtocolParameters::mainnet_defaults();
                        serde_json::to_string_pretty(&params)?
                    }
                };

                release_and_done(&mut client).await;

                if let Some(out) = out_file {
                    std::fs::write(&out, &json)?;
                    println!("Protocol parameters written to: {}", out.display());
                } else {
                    println!("{json}");
                }
                Ok(())
            }
            QuerySubcommand::StakeDistribution {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_stake_distribution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake distribution: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [map{pool_id => [tag(30)[num,den], vrf_hash]}]]
                // Haskell: MsgResult tag=4, HFC success wrapper=array(1)[result]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult tag 4, got {tag}");
                }

                // Strip HFC EitherMismatch success wrapper: array(1)[result]
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // Success: array(1) consumed, result follows
                } else {
                    decoder.set_position(pos);
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                println!("{:<60} {:>18}", "PoolId", "Stake Fraction");
                println!("{}", "-".repeat(80));

                for _ in 0..map_len {
                    let pool_id = decoder.bytes().unwrap_or(&[]);
                    let pool_hex = hex::encode(pool_id);
                    let _ = decoder.array(); // IndividualPoolStake array(2)
                                             // tag(30) rational
                    let _ = decoder.tag();
                    let _ = decoder.array();
                    let num = decoder.u64().unwrap_or(0);
                    let den = decoder.u64().unwrap_or(1);
                    let fraction = num as f64 / den.max(1) as f64;
                    decoder.skip().ok(); // vrf_hash
                    println!("{pool_hex:<60} {fraction:>18.10}");
                }

                println!("\nTotal pools: {map_len}");
                Ok(())
            }
            QuerySubcommand::StakeAddressInfo {
                address,
                socket_path,
                testnet_magic,
            } => {
                // Decode bech32 stake address to extract the 28-byte staking credential hash.
                // A Shelley reward address has the format: [header(1)] [stake_cred(28)]
                // The header byte encodes network and credential type; the credential starts at byte 1.
                let (_, addr_bytes) = bech32::decode(&address)
                    .map_err(|e| anyhow::anyhow!("Invalid bech32 address: {e}"))?;

                // Extract 28-byte credential hash (bytes 1..29 of the decoded address)
                let credential_bytes: &[u8] = if addr_bytes.len() >= 29 {
                    &addr_bytes[1..29]
                } else {
                    &addr_bytes
                };

                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Pass the credential to the node so it filters server-side.
                // This avoids fetching the entire stake address table over the socket.
                let raw = client
                    .query_stake_address_info(credential_bytes)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake address info: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(2) [delegations_map, rewards_map]]]
                // Haskell: MsgResult tag=4, HFC success wrapper=array(1)[result]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 4 {
                    anyhow::bail!("Expected MsgResult tag 4, got {tag}");
                }
                // Strip HFC EitherMismatch success wrapper: array(1)[result]
                let pos = decoder.position();
                if let Ok(Some(1)) = decoder.array() {
                    // Success: array(1) consumed, result follows
                } else {
                    decoder.set_position(pos);
                }

                // Result: array(2) [delegations_map, rewards_map]
                // delegations_map: Map<Credential, KeyHash StakePool>
                // rewards_map:     Map<Credential, Coin>
                let _ = decoder.array(); // array(2)

                // Parse delegations map: Map<Credential, pool_hash>
                let deleg_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut delegations: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for _ in 0..deleg_len {
                    // Credential: [0|1, hash(28)]
                    let _ = decoder.array();
                    let _ = decoder.u32(); // credential type (0=key, 1=script)
                    let cred = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let pool = hex::encode(decoder.bytes().unwrap_or(&[]));
                    delegations.insert(cred, pool);
                }

                // Parse rewards map: Map<Credential, Coin>
                // Since we filtered server-side the result should contain only the queried address,
                // but we iterate all entries for robustness.
                let rewards_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                // Collect entries so we can emit well-formed JSON (correct trailing comma)
                let mut entries: Vec<(String, u64)> = Vec::new();
                for _ in 0..rewards_len {
                    let _ = decoder.array();
                    let _ = decoder.u32(); // credential type
                    let cred = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let reward_balance = decoder.u64().unwrap_or(0);
                    entries.push((cred, reward_balance));
                }

                // Output JSON array matching cardano-cli 10.x format:
                // [{"address": "stake1...", "delegation": "pool1...", "rewardAccountBalance": N}]
                //
                // cardano-cli returns an empty array [] for addresses that are not registered.
                // Do NOT synthesize a zero-balance entry — that diverges from cardano-cli.
                println!("[");
                let last = entries.len().saturating_sub(1);
                for (i, (cred, reward_balance)) in entries.iter().enumerate() {
                    let pool = delegations.get(cred).cloned().unwrap_or_default();
                    let comma = if i < last { "," } else { "" };
                    println!("  {{");
                    println!("    \"address\": \"{address}\",");
                    if pool.is_empty() {
                        println!("    \"delegation\": null,");
                    } else {
                        println!("    \"delegation\": \"{pool}\",");
                    }
                    println!("    \"rewardAccountBalance\": {reward_balance}");
                    println!("  }}{comma}");
                }
                println!("]");

                Ok(())
            }
            QuerySubcommand::GovState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_gov_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query governance state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(7)]] — ConwayGovState with HFC wrapper
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC success wrapper array(1)
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                let mut proposals = Vec::new();
                let mut committee_count = 0usize;
                let mut constitution_url = String::new();

                // ConwayGovState = array(7)
                if let Ok(Some(7)) = decoder.array() {
                    // [0] Proposals = array(2) [roots, values]
                    if let Ok(Some(2)) = decoder.array() {
                        // roots: array(4) of StrictMaybe
                        decoder.skip().ok(); // skip roots

                        // values: array(n) of GovActionState
                        if let Ok(Some(n)) = decoder.array() {
                            for _ in 0..n {
                                // GovActionState = array(7)
                                if let Ok(Some(7)) = decoder.array() {
                                    // [0] GovActionId = array(2) [tx_hash, index]
                                    let _ = decoder.array();
                                    let tx_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                                    let action_idx = decoder.u32().unwrap_or(0);
                                    // [1] committeeVotes, [2] drepVotes, [3] spoVotes
                                    decoder.skip().ok();
                                    decoder.skip().ok();
                                    decoder.skip().ok();
                                    // [4] ProposalProcedure = array(4)
                                    let mut action_type = String::new();
                                    let mut deposit = 0u64;
                                    if let Ok(Some(4)) = decoder.array() {
                                        deposit = decoder.u64().unwrap_or(0);
                                        decoder.skip().ok(); // return_addr
                                                             // gov_action = sum type
                                        if let Ok(Some(_)) = decoder.array() {
                                            let gov_tag = decoder.u32().unwrap_or(6);
                                            action_type = match gov_tag {
                                                0 => "ParameterChange",
                                                1 => "HardForkInitiation",
                                                2 => "TreasuryWithdrawals",
                                                3 => "NoConfidence",
                                                4 => "UpdateCommittee",
                                                5 => "NewConstitution",
                                                _ => "InfoAction",
                                            }
                                            .to_string();
                                            // Skip remaining gov_action fields based on tag
                                            let skip_count = match gov_tag {
                                                0 => 3, // ParameterChange: prev, params, policy
                                                1 => 2, // HardFork: prev, version
                                                2 => 2, // Treasury: map, policy
                                                3 => 1, // NoConfidence: prev
                                                4 => 4, // UpdateCommittee: prev, remove, add, quorum
                                                5 => 2, // NewConstitution: prev, constitution
                                                _ => 0, // InfoAction: no fields
                                            };
                                            for _ in 0..skip_count {
                                                decoder.skip().ok();
                                            }
                                        }
                                        // anchor = array(2) [url, hash]
                                        decoder.skip().ok();
                                    }
                                    // [5] proposedIn, [6] expiresAfter
                                    let proposed = decoder.u64().unwrap_or(0);
                                    let expires = decoder.u64().unwrap_or(0);
                                    proposals.push((
                                        tx_id,
                                        action_idx,
                                        action_type,
                                        deposit,
                                        proposed,
                                        expires,
                                    ));
                                } else {
                                    decoder.skip().ok();
                                }
                            }
                        }
                    } else {
                        decoder.skip().ok();
                    }

                    // [1] Committee
                    if let Ok(Some(len)) = decoder.array() {
                        if len == 1 {
                            // StrictMaybe Just — array(2) [Map<Cred,Epoch>, threshold]
                            if let Ok(Some(2)) = decoder.array() {
                                let members = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                                committee_count = members as usize;
                                for _ in 0..members {
                                    decoder.skip().ok(); // credential
                                    decoder.skip().ok(); // epoch
                                }
                                decoder.skip().ok(); // threshold
                            }
                        }
                        // else: Nothing (0), no committee
                    }

                    // [2] Constitution = array(2) [Anchor, MaybeScriptHash]
                    if let Ok(Some(2)) = decoder.array() {
                        // Anchor = array(2) [url, hash]
                        if let Ok(Some(2)) = decoder.array() {
                            constitution_url = decoder.str().unwrap_or("").to_string();
                            decoder.skip().ok(); // hash
                        }
                        decoder.skip().ok(); // script hash
                    }

                    // [3]-[6] curPParams, prevPParams, FuturePParams, DRepPulsingState
                    // We don't display these, just note they exist
                }

                println!("Governance State (Conway)");
                println!("========================");
                println!("Committee Members: {committee_count}");
                if !constitution_url.is_empty() {
                    println!("Constitution:     {constitution_url}");
                }
                println!("Active Proposals: {}", proposals.len());

                if !proposals.is_empty() {
                    println!("\nProposals:");
                    println!(
                        "{:<20} {:<14} {:>15} {:>8} {:>8}",
                        "Type", "TxId", "Deposit (ADA)", "Proposed", "Expires"
                    );
                    println!("{}", "-".repeat(68));
                    for (tx_id, idx, action_type, deposit, proposed, expires) in &proposals {
                        let short_tx = if tx_id.len() > 8 {
                            format!("{}#{idx}", &tx_id[..8])
                        } else {
                            format!("{tx_id}#{idx}")
                        };
                        println!(
                            "{action_type:<20} {short_tx:<14} {:>15} {:>8} {:>8}",
                            deposit / 1_000_000,
                            proposed,
                            expires
                        );
                    }
                }

                Ok(())
            }
            QuerySubcommand::DrepState {
                drep_key_hash,
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_drep_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query DRep state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, array[map{...}]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // Parse Map<Credential, DRepState>
                // Credential: [type, hash(28)], DRepState: array(4) [expiry, maybe_anchor, deposit, delegators]
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                // Each entry: (cip0129_id, hex_hash, deposit, anchor_url, expiry_epoch)
                let mut dreps: Vec<(String, String, u64, String, u64)> = Vec::new();

                for _ in 0..map_len {
                    // Key: Credential array(2) [type, hash]
                    let _ = decoder.array();
                    // Credential type: 0 = key hash, 1 = script hash
                    let cred_type = decoder.u8().unwrap_or(0);
                    let hash_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                    let hex_hash = hex::encode(&hash_bytes);

                    // Encode as CIP-0129 bech32 identifier (drep1 / drep_script1)
                    let cip0129_id =
                        dugite_primitives::encode_drep_from_cbor(cred_type, &hash_bytes)
                            .unwrap_or_else(|_| hex_hash.clone());

                    // Value: DRepState array(4)
                    let _ = decoder.array();
                    let expiry_epoch = decoder.u64().unwrap_or(0);
                    // maybe_anchor: array(0)=None, array(1)=[anchor]
                    let anchor_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut anchor = String::new();
                    if anchor_len == 1 {
                        let _ = decoder.array(); // Anchor [url, hash]
                        anchor = decoder.str().map(|s| s.to_string()).unwrap_or_default();
                        decoder.skip().ok(); // skip hash
                    }
                    let deposit = decoder.u64().unwrap_or(0);
                    decoder.skip().ok(); // skip delegators set

                    dreps.push((cip0129_id, hex_hash, deposit, anchor, expiry_epoch));
                }

                // Filter by key hash if provided — match against either the hex hash or the bech32 id
                let filtered: Vec<_> = if let Some(ref hash) = drep_key_hash {
                    dreps
                        .iter()
                        .filter(|(id, hex, _, _, _)| hex.contains(hash) || id.contains(hash))
                        .collect()
                } else {
                    dreps.iter().collect()
                };

                println!("DRep State (Conway)");
                println!("===================");
                println!("Total DReps: {}", dreps.len());

                if !filtered.is_empty() {
                    // CIP-0129 identifiers are at most ~63 chars; use 66-char column.
                    println!(
                        "\n{:<66} {:>16} {:>8}",
                        "DRep ID (CIP-0129)", "Deposit (ADA)", "Epoch"
                    );
                    println!("{}", "-".repeat(92));
                    for (cip0129_id, _hex, deposit, anchor, epoch) in &filtered {
                        let deposit_ada = *deposit / 1_000_000;
                        println!("{cip0129_id:<66} {deposit_ada:>16} {epoch:>8}");
                        if !anchor.is_empty() {
                            println!("  Anchor: {anchor}");
                        }
                    }
                }

                Ok(())
            }
            QuerySubcommand::CommitteeState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_committee_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query committee state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(3) [map_members, maybe_threshold, epoch]]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // CommitteeMembersState array(3)
                let _ = decoder.array();

                // [0] Map<ColdCredential, CommitteeMemberState>
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                // Each entry: (cold_cip0129, hot_cip0129, status_tag)
                let mut members: Vec<(String, String, u32)> = Vec::new();
                let mut resigned_count = 0u64;

                for _ in 0..map_len {
                    // Key: ColdCredential array(2) [type, hash]
                    let _ = decoder.array();
                    let cold_type = decoder.u8().unwrap_or(0);
                    let cold_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                    // Encode with CIP-0129 cc_cold1 / cc_cold_script1
                    let cold = dugite_primitives::encode_cc_cold_from_cbor(cold_type, &cold_bytes)
                        .unwrap_or_else(|_| hex::encode(&cold_bytes));

                    // Value: CommitteeMemberState array(4)
                    let _ = decoder.array();
                    // [0] HotCredAuthStatus
                    let status_arr = decoder.array().unwrap_or(Some(1)).unwrap_or(1);
                    let status_tag = decoder.u32().unwrap_or(1);
                    let mut hot = String::new();
                    if status_tag == 0 && status_arr >= 2 {
                        // MemberAuthorized: [0, hot_credential]
                        let _ = decoder.array(); // [type, hash]
                        let hot_type = decoder.u8().unwrap_or(0);
                        let hot_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                        // Encode with CIP-0129 cc_hot1 / cc_hot_script1
                        hot = dugite_primitives::encode_cc_hot_from_cbor(hot_type, &hot_bytes)
                            .unwrap_or_else(|_| hex::encode(&hot_bytes));
                    } else if status_tag == 2 {
                        resigned_count += 1;
                        // MemberResigned: skip maybe_anchor
                        if status_arr >= 2 {
                            decoder.skip().ok();
                        }
                    }
                    // [1] MemberStatus
                    decoder.skip().ok();
                    // [2] Maybe EpochNo
                    decoder.skip().ok();
                    // [3] NextEpochChange
                    decoder.skip().ok();

                    members.push((cold, hot, status_tag));
                }

                // [1] Maybe threshold
                decoder.skip().ok();
                // [2] Current epoch
                let epoch = decoder.u64().unwrap_or(0);

                println!("Constitutional Committee State (Conway)");
                println!("=======================================");
                println!("Epoch: {epoch}");
                println!(
                    "Active Members: {}",
                    members.iter().filter(|(_, _, s)| *s == 0).count()
                );
                println!("Resigned Members: {resigned_count}");

                let authorized: Vec<_> = members.iter().filter(|(_, _, s)| *s == 0).collect();
                if !authorized.is_empty() {
                    // CIP-0129 identifiers: cc_cold1/cc_cold_script1 and cc_hot1/cc_hot_script1
                    println!(
                        "\n{:<65} {:<65}",
                        "Cold Credential (CIP-0129)", "Hot Credential (CIP-0129)"
                    );
                    println!("{}", "-".repeat(132));
                    for (cold, hot, _) in &authorized {
                        println!("{cold:<65} {hot:<65}");
                    }
                }

                Ok(())
            }
            QuerySubcommand::TxMempool {
                socket_path,
                subcmd,
                tx_id,
                testnet_magic,
            } => {
                let mut client = connect_and_handshake(&socket_path, testnet_magic).await?;

                match subcmd.as_str() {
                    "info" => {
                        let slot = client
                            .monitor_acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor acquire failed: {e}"))?;
                        let (capacity, size, num_txs) = client
                            .monitor_get_sizes()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor get sizes failed: {e}"))?;

                        println!("Mempool snapshot at slot {slot}:");
                        println!("  Capacity:     {capacity} bytes");
                        println!("  Size:         {size} bytes");
                        println!("  Transactions: {num_txs}");

                        let _ = client.monitor_done().await;
                    }
                    "has-tx" => {
                        let id = tx_id.ok_or_else(|| {
                            anyhow::anyhow!("--tx-id is required for has-tx subcommand")
                        })?;
                        let hash_bytes = hex::decode(&id)
                            .map_err(|e| anyhow::anyhow!("Invalid tx ID hex: {e}"))?;
                        if hash_bytes.len() != 32 {
                            return Err(anyhow::anyhow!(
                                "Transaction ID must be 32 bytes (64 hex chars)"
                            ));
                        }
                        let mut tx_hash = [0u8; 32];
                        tx_hash.copy_from_slice(&hash_bytes);

                        let _slot = client
                            .monitor_acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor acquire failed: {e}"))?;
                        let has_tx = client
                            .monitor_has_tx(&tx_hash)
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor has-tx failed: {e}"))?;

                        if has_tx {
                            println!("Transaction {id} is in the mempool");
                        } else {
                            println!("Transaction {id} is NOT in the mempool");
                        }

                        let _ = client.monitor_done().await;
                    }
                    "next-tx" => {
                        let _slot = client
                            .monitor_acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("Monitor acquire failed: {e}"))?;

                        loop {
                            match client.monitor_next_tx().await {
                                Ok(Some((tx_hash, tx_cbor))) => {
                                    println!("TxHash: {}", hex::encode(&tx_hash));
                                    println!("CBOR size: {} bytes", tx_cbor.len());
                                    println!("{}", hex::encode(&tx_cbor));
                                    println!();
                                }
                                Ok(None) => {
                                    println!("No more transactions in mempool");
                                    break;
                                }
                                Err(e) => {
                                    return Err(anyhow::anyhow!("next-tx failed: {e}"));
                                }
                            }
                        }

                        let _ = client.monitor_done().await;
                    }
                    _ => {
                        println!("Available subcommands: info, has-tx, next-tx");
                    }
                }
                Ok(())
            }
            QuerySubcommand::StakePools {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_stake_distribution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake pools: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [Map<pool_hash, [ratio, vrf_hash]>]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }
                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                struct PoolInfo {
                    pool_id: String,
                    stake_num: u64,
                    stake_den: u64,
                }
                let mut pools = Vec::new();
                // Map<pool_hash(28), [tag(30)[num,den], vrf_hash(32)]>
                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                for _ in 0..map_len {
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = decoder.array(); // array(2)
                                             // Stake ratio: tag(30)[num, den]
                    let _ = decoder.tag(); // tag(30)
                    let _ = decoder.array(); // [num, den]
                    let stake_num = decoder.u64().unwrap_or(0);
                    let stake_den = decoder.u64().unwrap_or(1);
                    // VRF hash
                    let _vrf = decoder.bytes().unwrap_or(&[]);
                    pools.push(PoolInfo {
                        pool_id,
                        stake_num,
                        stake_den,
                    });
                }

                println!("{:<58} {:>12}", "PoolId", "Stake");
                println!("{}", "-".repeat(72));
                for p in &pools {
                    let stake_frac = if p.stake_den > 0 {
                        p.stake_num as f64 / p.stake_den as f64
                    } else {
                        0.0
                    };
                    println!("{:<58} {:>11.6}%", p.pool_id, stake_frac * 100.0);
                }
                println!("\nTotal pools: {}", pools.len());
                Ok(())
            }
            QuerySubcommand::StakeSnapshot {
                socket_path,
                testnet_magic,
                stake_pool_id,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let result = client
                    .query_stake_snapshot()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake snapshot: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(4) [pool_map, mark_total, set_total, go_total]]]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }
                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                // array(4) [pool_map, mark_total, set_total, go_total]
                let _ = decoder.array(); // array(4)

                // pool_map: Map<pool_hash(28), array(3) [mark, set, go]>
                let pool_count = decoder.map().unwrap_or(Some(0)).unwrap_or(0);
                let mut pools: Vec<(String, u64, u64, u64)> = Vec::new();
                for _ in 0..pool_count {
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = decoder.array(); // array(3)
                    let mark = decoder.u64().unwrap_or(0);
                    let set = decoder.u64().unwrap_or(0);
                    let go = decoder.u64().unwrap_or(0);
                    pools.push((pool_id, mark, set, go));
                }

                let total_mark = decoder.u64().unwrap_or(0);
                let total_set = decoder.u64().unwrap_or(0);
                let total_go = decoder.u64().unwrap_or(0);

                println!("Stake Snapshot");
                println!("==============");
                println!("Total Mark Stake: {} ADA", total_mark / 1_000_000);
                println!("Total Set Stake:  {} ADA", total_set / 1_000_000);
                println!("Total Go Stake:   {} ADA", total_go / 1_000_000);
                println!("Pools: {}", pools.len());

                // Filter by pool ID if provided
                let filtered: Vec<_> = if let Some(ref pool_id) = stake_pool_id {
                    pools
                        .iter()
                        .filter(|(id, _, _, _)| id.contains(pool_id))
                        .collect()
                } else {
                    pools.iter().collect()
                };

                if !filtered.is_empty() {
                    println!(
                        "\n{:<58} {:>16} {:>16} {:>16}",
                        "Pool ID", "Mark (ADA)", "Set (ADA)", "Go (ADA)"
                    );
                    println!("{}", "-".repeat(108));
                    for (id, mark, set, go) in &filtered {
                        println!(
                            "{id:<58} {:>16} {:>16} {:>16}",
                            mark / 1_000_000,
                            set / 1_000_000,
                            go / 1_000_000
                        );
                    }
                }
                Ok(())
            }
            QuerySubcommand::PoolParams {
                socket_path,
                testnet_magic,
                stake_pool_id,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let result = client
                    .query_pool_params()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query pool params: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [Map<pool_hash(28), PoolParams_array(9)>]]
                let mut decoder = minicbor::Decoder::new(&result);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Unexpected response tag: {tag}");
                }
                // Strip HFC success wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // Consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                let map_len = decoder.map().unwrap_or(Some(0)).unwrap_or(0);

                struct PoolInfo {
                    pool_id: String,
                    vrf_keyhash: String,
                    pledge: u64,
                    cost: u64,
                    margin_num: u64,
                    margin_den: u64,
                    relays: Vec<String>,
                    reward_account: String,
                    owners: Vec<String>,
                    metadata_url: Option<String>,
                    metadata_hash: Option<String>,
                }

                let mut pools: Vec<PoolInfo> = Vec::new();

                for _ in 0..map_len {
                    // Key: pool hash
                    let pool_id = hex::encode(decoder.bytes().unwrap_or(&[]));
                    // Value: array(9) PoolParams
                    let _ = decoder.array(); // consume array(9)
                    let operator = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let _ = operator; // same as pool_id
                    let vrf_keyhash = hex::encode(decoder.bytes().unwrap_or(&[]));
                    let pledge = decoder.u64().unwrap_or(0);
                    let cost = decoder.u64().unwrap_or(0);
                    // margin: tagged rational tag(30)[num, den]
                    let (margin_num, margin_den) = {
                        let _ = decoder.tag(); // tag(30)
                        let _ = decoder.array(); // [num, den]
                        let n = decoder.u64().unwrap_or(0);
                        let d = decoder.u64().unwrap_or(1);
                        (n, d)
                    };
                    let reward_account = hex::encode(decoder.bytes().unwrap_or(&[]));
                    // owners: tag(258) set
                    let _ = decoder.tag(); // tag(258)
                    let owner_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut owners = Vec::new();
                    for _ in 0..owner_len {
                        owners.push(hex::encode(decoder.bytes().unwrap_or(&[])));
                    }
                    // relays: array of relay structures
                    let relay_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let mut relays = Vec::new();
                    for _ in 0..relay_len {
                        let _ = decoder.array(); // relay array
                        let relay_tag = decoder.u32().unwrap_or(99);
                        match relay_tag {
                            0 => {
                                // SingleHostAddr: port, ipv4, ipv6
                                let port = decoder.u16().ok();
                                if port.is_none() {
                                    decoder.skip().ok();
                                } // skip null
                                let ipv4 = decoder.bytes().ok().map(|b| {
                                    if b.len() == 4 {
                                        format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
                                    } else {
                                        hex::encode(b)
                                    }
                                });
                                if ipv4.is_none() {
                                    decoder.skip().ok();
                                } // skip null
                                decoder.skip().ok(); // skip ipv6
                                let addr = ipv4.unwrap_or_default();
                                relays.push(format!("{}:{}", addr, port.unwrap_or(0)));
                            }
                            1 => {
                                // SingleHostName: port, dns_name
                                let port = decoder.u16().ok();
                                if port.is_none() {
                                    decoder.skip().ok();
                                }
                                let dns = decoder.str().unwrap_or("").to_string();
                                relays.push(format!("{}:{}", dns, port.unwrap_or(0)));
                            }
                            2 => {
                                // MultiHostName: dns_name
                                relays.push(decoder.str().unwrap_or("").to_string());
                            }
                            _ => {
                                // Skip unknown relay type fields
                                decoder.skip().ok();
                            }
                        }
                    }
                    // metadata: nullable [url, hash]
                    let (metadata_url, metadata_hash) = {
                        let pos = decoder.position();
                        if let Ok(Some(2)) = decoder.array() {
                            let url = Some(decoder.str().unwrap_or("").to_string());
                            let hash = decoder.bytes().ok().map(hex::encode);
                            (url, hash)
                        } else {
                            decoder.set_position(pos);
                            decoder.skip().ok(); // skip null
                            (None, None)
                        }
                    };

                    pools.push(PoolInfo {
                        pool_id,
                        vrf_keyhash,
                        pledge,
                        cost,
                        margin_num,
                        margin_den,
                        relays,
                        reward_account,
                        owners,
                        metadata_url,
                        metadata_hash,
                    });
                }

                // Filter by pool ID if provided
                let filtered: Vec<_> = if let Some(ref pid) = stake_pool_id {
                    pools.iter().filter(|p| p.pool_id.contains(pid)).collect()
                } else {
                    pools.iter().collect()
                };

                println!("Pool Parameters");
                println!("===============");
                println!("Total Pools: {}", pools.len());

                for pool in &filtered {
                    println!("\nPool ID:       {}", pool.pool_id);
                    println!("VRF Key:       {}", pool.vrf_keyhash);
                    println!("Pledge:        {} ADA", pool.pledge / 1_000_000);
                    println!("Cost:          {} ADA", pool.cost / 1_000_000);
                    if pool.margin_den > 0 {
                        let margin_pct = (pool.margin_num as f64 / pool.margin_den as f64) * 100.0;
                        println!("Margin:        {margin_pct:.2}%");
                    }
                    if !pool.reward_account.is_empty() {
                        println!("Reward Acct:   {}", pool.reward_account);
                    }
                    if !pool.owners.is_empty() {
                        println!("Owners:        {}", pool.owners.join(", "));
                    }
                    if !pool.relays.is_empty() {
                        println!("Relays:        {}", pool.relays.join(", "));
                    }
                    if let Some(url) = &pool.metadata_url {
                        println!("Metadata URL:  {url}");
                    }
                    if let Some(hash) = &pool.metadata_hash {
                        println!("Metadata Hash: {hash}");
                    }
                }
                Ok(())
            }
            QuerySubcommand::Treasury {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_account_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query account state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [treasury, reserves]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // consumed wrapper
                } else {
                    decoder.set_position(pos);
                }

                let _ = decoder.array();
                let treasury = decoder.u64().unwrap_or(0);
                let reserves = decoder.u64().unwrap_or(0);

                println!("Account State");
                println!("=============");
                println!("Treasury: {} ADA", treasury / 1_000_000);
                println!("Reserves: {} ADA", reserves / 1_000_000);

                Ok(())
            }
            QuerySubcommand::Constitution {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_constitution()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query constitution: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, [array(2) [Anchor, MaybeScriptHash]]]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC wrapper
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                // Constitution = array(2) [Anchor, MaybeScriptHash]
                let mut url = String::new();
                let mut data_hash = String::new();
                let mut script_hash = String::from("none");

                if let Ok(Some(2)) = decoder.array() {
                    // Anchor = array(2) [url, hash]
                    if let Ok(Some(2)) = decoder.array() {
                        url = decoder.str().unwrap_or("").to_string();
                        data_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                    }
                    // StrictMaybe ScriptHash
                    if let Ok(bytes) = decoder.bytes() {
                        script_hash = hex::encode(bytes);
                    }
                }

                println!("Constitution");
                println!("============");
                println!("URL:         {url}");
                println!("Data Hash:   {data_hash}");
                println!("Script Hash: {script_hash}");

                Ok(())
            }
            QuerySubcommand::RatifyState {
                socket_path,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_ratify_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query ratify state: {e}"))?;

                release_and_done(&mut client).await;

                // Parse MsgResult [4, ratify_state]
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array();
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // Strip HFC wrapper array(1)
                let pos = decoder.position();
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u64(); // consume HFC success tag (1)
                } else if let Ok(Some(1)) = {
                    decoder.set_position(pos);
                    decoder.array()
                } {
                    // HFC wrapper stripped
                } else {
                    decoder.set_position(pos);
                }

                // RatifyState = array(4) [enacted_seq, expired_seq, delayed_bool, future_pparam_update]
                let mut enacted_count = 0u64;
                let mut expired_count = 0u64;
                let mut delayed = false;
                let mut enacted_ids = Vec::new();
                let mut expired_ids = Vec::new();

                if let Ok(Some(4)) = decoder.array() {
                    // enacted: array(n) of (GovActionState, GovActionId)
                    if let Ok(Some(n)) = decoder.array() {
                        enacted_count = n;
                        for _ in 0..n {
                            // array(2) [GovActionState, GovActionId]
                            if let Ok(Some(2)) = decoder.array() {
                                // Skip GovActionState (complex structure)
                                decoder.skip().ok();
                                // GovActionId = array(2) [tx_hash, index]
                                if let Ok(Some(2)) = decoder.array() {
                                    let tx_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                                    let index = decoder.u32().unwrap_or(0);
                                    enacted_ids.push(format!("{}#{}", tx_hash, index));
                                }
                            }
                        }
                    }

                    // expired: array(n) of GovActionId
                    if let Ok(Some(n)) = decoder.array() {
                        expired_count = n;
                        for _ in 0..n {
                            // GovActionId = array(2) [tx_hash, index]
                            if let Ok(Some(2)) = decoder.array() {
                                let tx_hash = hex::encode(decoder.bytes().unwrap_or(&[]));
                                let index = decoder.u32().unwrap_or(0);
                                expired_ids.push(format!("{}#{}", tx_hash, index));
                            }
                        }
                    }

                    // delayed flag
                    delayed = decoder.bool().unwrap_or(false);

                    // future_pparam_update: skip
                    decoder.skip().ok();
                }

                println!("Ratification State");
                println!("==================");
                println!("Enacted proposals: {enacted_count}");
                for id in &enacted_ids {
                    println!("  {id}");
                }
                println!("Expired proposals: {expired_count}");
                for id in &expired_ids {
                    println!("  {id}");
                }
                println!("Delayed:           {delayed}");

                Ok(())
            }
            QuerySubcommand::LeadershipSchedule {
                socket_path,
                mainnet,
                testnet_magic,
                genesis,
                stake_pool_id,
                cold_verification_key_file,
                vrf_signing_key_file,
                current,
                next,
                output_json: _,
                output_text,
                out_file,
            } => {
                // Resolve network magic: --mainnet wins, otherwise use --testnet-magic.
                let testnet_magic = if mainnet {
                    Some(764_824_073)
                } else {
                    testnet_magic
                };

                // Allow CARDANO_NODE_SOCKET_PATH to override the default socket path.
                let socket_path = if socket_path == std::path::Path::new("node.sock") {
                    if let Ok(env_sock) = std::env::var("CARDANO_NODE_SOCKET_PATH") {
                        PathBuf::from(env_sock)
                    } else {
                        socket_path
                    }
                } else {
                    socket_path
                };

                // When neither flag is supplied, default to --current behaviour.
                let use_next = next && !current;

                // Resolve pool ID from explicit hex or from the cold verification key.
                let pool_id_hex = if let Some(ref id) = stake_pool_id {
                    id.to_lowercase()
                } else if let Some(ref path) = cold_verification_key_file {
                    pool_id_from_cold_vkey(path)?
                } else {
                    anyhow::bail!(
                        "Either --stake-pool-id or --cold-verification-key-file is required"
                    );
                };

                // Load and decode the VRF signing key (Cardano text-envelope format).
                // The `cborHex` field contains a CBOR-wrapped 32- or 64-byte secret key.
                // We strip the CBOR wrapper and use the first 32 bytes as the VRF seed.
                let vrf_content = std::fs::read_to_string(&vrf_signing_key_file)?;
                let vrf_env: serde_json::Value = serde_json::from_str(&vrf_content)?;
                let vrf_cbor_hex = vrf_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in VRF skey file"))?;
                let vrf_cbor = hex::decode(vrf_cbor_hex)?;
                // Strip the one- or two-byte CBOR bytestring header.
                let vrf_key_bytes = if vrf_cbor.len() > 2 && vrf_cbor[0] == 0x58 {
                    &vrf_cbor[2..]
                } else if vrf_cbor.len() > 1 && (vrf_cbor[0] & 0xe0) == 0x40 {
                    &vrf_cbor[1..]
                } else {
                    &vrf_cbor
                };
                let vrf_seed = match vrf_key_bytes.len() {
                    32 => vrf_key_bytes,
                    64 => &vrf_key_bytes[..32],
                    n => anyhow::bail!("VRF secret key must be 32 or 64 bytes, got {n}"),
                };
                let mut vrf_skey = [0u8; 32];
                vrf_skey.copy_from_slice(vrf_seed);

                // Read timing parameters from the Shelley genesis file.
                let gp = read_shelley_genesis(&genesis)?;

                // Connect to the node socket and acquire a ledger state snapshot.
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                // Query the current chain tip to determine the current epoch boundaries.
                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;

                // Slot within the current epoch and the absolute slot at epoch start.
                let slot_in_epoch = tip.slot % gp.epoch_length;
                let current_epoch_start = tip.slot - slot_in_epoch;
                let epoch_start_slot = if use_next {
                    current_epoch_start + gp.epoch_length
                } else {
                    current_epoch_start
                };

                // Query the chain-dependent state (tag 13 = PraosState) to extract nonces.
                // This is much smaller than query_protocol_state (tag 8 = full ledger dump).
                let proto_raw = client
                    .query_chain_dep_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query chain dep state: {e}"))?;
                let nonces = parse_protocol_state_nonces(&proto_raw)?;

                // For --current use the epoch nonce; for --next use the candidate nonce
                // (which becomes the epoch nonce at the next epoch boundary).
                let epoch_nonce_bytes = if use_next {
                    nonces.candidate_nonce
                } else {
                    nonces.epoch_nonce
                };
                let epoch_nonce = dugite_primitives::Hash(epoch_nonce_bytes);

                // Query the stake snapshot for pool/total active stake figures.
                let stake_raw = client
                    .query_stake_snapshot()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query stake snapshot: {e}"))?;

                release_and_done(&mut client).await;

                // use_next=true → use the "mark" snapshot; false → "set" snapshot.
                let stake = parse_stake_for_pool(&stake_raw, &pool_id_hex, use_next)?;

                // Convert the floating-point active-slots coefficient to an exact rational.
                let (f_num, f_den) = f64_to_rational_approx(gp.active_slots_coeff);

                // Compute the full leadership schedule for the epoch.
                let schedule = dugite_consensus::compute_leader_schedule(
                    &vrf_skey,
                    &epoch_nonce,
                    epoch_start_slot,
                    gp.epoch_length,
                    stake.pool_stake,
                    stake.total_active_stake,
                    f_num,
                    f_den,
                );

                // Build JSON entries matching cardano-cli's output format.
                let json_entries: Vec<serde_json::Value> = schedule
                    .iter()
                    .map(|s| {
                        let time = slot_to_utc(s.slot.0, gp.system_start_unix, gp.slot_length);
                        serde_json::json!({
                            "slotNumber": s.slot.0,
                            "slotTime": time,
                        })
                    })
                    .collect();

                // Render as human-readable text table or JSON (default).
                let output = if output_text {
                    let mut lines = Vec::new();
                    lines.push(format!("{:<15} {}", "SlotNo", "UTC Time"));
                    lines.push("-".repeat(50));
                    for entry in &json_entries {
                        lines.push(format!(
                            "{:<15} {}",
                            entry["slotNumber"],
                            entry["slotTime"].as_str().unwrap_or("")
                        ));
                    }
                    lines.join("\n")
                } else {
                    serde_json::to_string_pretty(&json_entries)?
                };

                if let Some(ref path) = out_file {
                    std::fs::write(path, &output)?;
                    eprintln!("Leadership schedule written to: {}", path.display());
                } else {
                    println!("{output}");
                }

                Ok(())
            }
            QuerySubcommand::SlotNumber {
                utc_time,
                socket_path,
                testnet_magic,
            } => {
                // Parse the caller-supplied UTC timestamp to a Unix timestamp
                // (seconds since 1970-01-01T00:00:00Z).
                let target_unix = parse_iso8601_to_unix(&utc_time).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Cannot parse timestamp '{}'. Expected ISO-8601 UTC, e.g. 2024-01-15T00:00:00Z",
                        utc_time
                    )
                })?;

                // Connect to the node (no state acquire needed: GetEraHistory is
                // a top-level HardFork query that does not require acquired state).
                let mut client = connect_and_handshake(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_era_history()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query era history: {e}"))?;

                // GetEraHistory response: MsgResult [4, <indefinite-array of EraSummary>]
                // Each EraSummary = array(3)[EraParams, EraStart, SafeZone]
                // EraStart      = array(2)[slot_offset: u64, time_offset_ms: u64]
                //                  (time_offset is milliseconds relative to the system start)
                // EraParams     = array(3)[epoch_length, slot_length_ms, safe_zone_k]
                //
                // We only need the *last* era summary since the current chain tip is
                // guaranteed to be within it.  We iterate all summaries to land on the
                // final one whose zero_time_ms ≤ target timestamp.
                let mut decoder = minicbor::Decoder::new(&raw);
                let _ = decoder.array(); // MsgResult outer array(2)
                let tag = decoder.u32().unwrap_or(999);
                if tag != 6 {
                    anyhow::bail!("Expected MsgResult(6), got {tag}");
                }

                // EraHistory is NOT wrapped in an HFC success array(1).  Decode
                // the indefinite-length array of summaries directly.
                let mut best_zero_slot: u64 = 0;
                let mut best_zero_time_ms: u64 = 0;
                let mut best_slot_length_ms: u64 = 1_000; // 1 second default

                // Consume the outer indefinite array (or definite array) of summaries.
                // minicbor represents indefinite arrays as None from .array().
                let _ = decoder.array(); // may be Some(n) or None (indefinite)
                loop {
                    // Each entry is array(3): [EraParams, EraStart, SafeZone]
                    // Attempt to read the next EraStart; break on any parse failure
                    // (which indicates end-of-array or break code for indefinite).
                    let arr = decoder.array();
                    if arr.is_err() {
                        break;
                    }

                    // EraParams = array(3)[epoch_length, slot_length_ms, safe_zone]
                    let params_len = decoder.array().unwrap_or(Some(0)).unwrap_or(0);
                    let _epoch_length = decoder.u64().unwrap_or(0);
                    let slot_length_ms = decoder.u64().unwrap_or(1_000);
                    // Consume any remaining EraParams fields (safe_zone etc.)
                    for _ in 2..params_len {
                        decoder.skip().ok();
                    }

                    // EraStart = array(2)[slot_offset, time_offset_ms]
                    if decoder.array().is_err() {
                        break;
                    }
                    let zero_slot = decoder.u64().unwrap_or(0);
                    let zero_time_ms = decoder.u64().unwrap_or(0);

                    // SafeZone: skip (we only need slot/time offsets)
                    decoder.skip().ok();

                    best_zero_slot = zero_slot;
                    best_zero_time_ms = zero_time_ms;
                    best_slot_length_ms = slot_length_ms;
                }

                // Obtain the system start time via GetSystemStart so we can
                // convert zero_time_ms (relative offset) to absolute Unix seconds.
                // Re-use the same client connection (still in Idle state after
                // the first query).
                let system_start_str = client
                    .query_system_start()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query system start: {e}"))?;

                let system_start_unix =
                    parse_iso8601_to_unix(&system_start_str).ok_or_else(|| {
                        anyhow::anyhow!("Cannot parse system start: {}", system_start_str)
                    })?;

                client.done().await.ok();

                // Convert zero_time_ms (ms since system start) to an absolute
                // Unix timestamp in whole seconds.
                let era_start_unix = system_start_unix + best_zero_time_ms / 1_000;

                if target_unix < era_start_unix {
                    anyhow::bail!(
                        "Timestamp {} is before the era start at {} ({})",
                        utc_time,
                        system_start_str,
                        era_start_unix
                    );
                }

                // slot = era_zero_slot + floor((target - era_start) * 1000 / slot_length_ms)
                let elapsed_secs = target_unix - era_start_unix;
                let elapsed_ms = elapsed_secs * 1_000;
                let slot_offset = elapsed_ms / best_slot_length_ms;
                let slot = best_zero_slot + slot_offset;

                // cardano-cli outputs just the slot number as a plain integer.
                println!("{slot}");
                Ok(())
            }
            QuerySubcommand::KesPeriodInfo {
                op_cert_file,
                socket_path,
                testnet_magic,
            } => {
                // Read and decode the operational certificate text envelope.
                // OpCert envelope type is "NodeOperationalCertificate".
                // CBOR payload = array(2)[
                //   array(4)[hot_vkey(32), counter(u64), kes_period(u64), sigma(64)],
                //   cold_vkey(32)
                // ]
                let cert_content = std::fs::read_to_string(&op_cert_file)?;
                let cert_env: serde_json::Value = serde_json::from_str(&cert_content)?;
                let cbor_hex = cert_env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in opcert file"))?;
                let cbor_bytes = hex::decode(cbor_hex)?;

                // Decode the operational certificate CBOR.
                //
                // Two formats exist:
                //   (a) Flat: array(4)[hot_vkey, counter, kes_period, sigma]
                //       — legacy format (missing cold_vkey)
                //   (b) Wrapped: array(2)[array(4)[hot_vkey, counter, kes_period, sigma], cold_vkey]
                //       — standard format (cardano-cli / dugite-cli)
                //
                // We detect the format by checking the first element's type:
                // if it's bytes → flat (a); if it's an array → wrapped (b).
                let mut dec = minicbor::Decoder::new(&cbor_bytes);
                let arr_len = dec
                    .array()
                    .map_err(|e| anyhow::anyhow!("Invalid opcert CBOR: {e}"))?;

                // Peek at the first element type to detect format
                let first_type = dec
                    .datatype()
                    .map_err(|e| anyhow::anyhow!("Invalid opcert CBOR element: {e}"))?;

                if first_type == minicbor::data::Type::Array {
                    // Wrapped format (b): skip into the inner array(4)
                    dec.array()
                        .map_err(|e| anyhow::anyhow!("Invalid inner cert array: {e}"))?;
                }
                // Now positioned at: hot_vkey, counter, kes_period, sigma
                let _hot_vkey = dec.bytes().unwrap_or(&[]).to_vec();
                let opcert_counter = dec.u64().unwrap_or(0);
                let opcert_kes_period = dec.u64().unwrap_or(0);
                let _sigma = dec.bytes().ok();
                let _ = arr_len; // suppress unused warning

                // Compute the current KES period from the tip slot and genesis params.
                // KES period = current_slot / slots_per_kes_period
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;
                let tip = client
                    .query_tip()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query tip: {e}"))?;
                release_and_done(&mut client).await;

                let current_slot = tip.slot;

                // slotsPerKESPeriod: 129600 on mainnet/preview/preprod
                // This is a genesis parameter — universal across all Cardano networks.
                let slots_per_kes_period: u64 = 129600;
                let current_kes_period = current_slot / slots_per_kes_period;

                // The KES evolution window is the number of periods a KES key
                // covers before it must be rotated.  max_kes_evolutions = 62
                // on all networks.  The start period and end
                // period of the cert are the key facts the operator needs.
                const KES_EVOLUTIONS: u64 = 129; // slotsPerKESPeriod typical value
                let cert_start = opcert_kes_period;
                let cert_end = opcert_kes_period + KES_EVOLUTIONS;

                // Determine validity status
                let status = if current_kes_period < cert_start {
                    "NOT YET VALID"
                } else if current_kes_period > cert_end {
                    "EXPIRED"
                } else {
                    "VALID"
                };

                let periods_remaining = cert_end.saturating_sub(current_kes_period);

                // Output format matches cardano-cli query kes-period-info.
                println!("KES Period Info");
                println!("===============");
                println!("Status:                  {status}");
                println!("Operational certificate: {}", op_cert_file.display());
                println!("KES counter:             {opcert_counter}");
                println!("Opcert start KES period: {cert_start}");
                println!("Opcert end KES period:   {cert_end}");
                println!("Current KES period:      {current_kes_period}");
                println!("KES periods remaining:   {periods_remaining}");

                if status == "EXPIRED" {
                    eprintln!(
                        "Warning: operational certificate has expired. Rotate KES key immediately."
                    );
                } else if status == "NOT YET VALID" {
                    eprintln!(
                        "Warning: operational certificate is not yet valid (starts at period {cert_start}, currently {current_kes_period})."
                    );
                }

                Ok(())
            }
            QuerySubcommand::LedgerState {
                socket_path,
                out_file,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_ledger_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query ledger state: {e}"))?;

                release_and_done(&mut client).await;

                if let Some(out) = out_file {
                    std::fs::write(&out, &raw)?;
                    println!("Ledger state written to: {}", out.display());
                } else {
                    // Print as hex if no out_file
                    println!("{}", hex::encode(&raw));
                }
                Ok(())
            }
            QuerySubcommand::ProtocolState {
                socket_path,
                out_file,
                testnet_magic,
            } => {
                let mut client = connect_and_acquire(&socket_path, testnet_magic).await?;

                let raw = client
                    .query_protocol_state()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to query protocol state: {e}"))?;

                release_and_done(&mut client).await;

                if let Some(out) = out_file {
                    std::fs::write(&out, &raw)?;
                    println!("Protocol state written to: {}", out.display());
                } else {
                    println!("{}", hex::encode(&raw));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso8601_to_unix_epoch() {
        // 1970-01-01T00:00:00Z = Unix epoch
        assert_eq!(parse_iso8601_to_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn test_parse_iso8601_to_unix_known_date() {
        // 2022-11-24T00:00:00Z = Preview testnet genesis (approximately)
        let ts = parse_iso8601_to_unix("2022-11-24T00:00:00Z").unwrap();
        // 2022-11-24 is day 328 of year 2022
        // Expected: 1669248000
        assert_eq!(ts, 1669248000);
    }

    #[test]
    fn test_parse_iso8601_to_unix_with_time() {
        let ts = parse_iso8601_to_unix("2020-06-01T21:44:51Z").unwrap();
        // Manually computed
        assert!(ts > 1590000000 && ts < 1600000000);
        assert_eq!(ts, 1591047891);
    }

    #[test]
    fn test_parse_iso8601_leap_year() {
        // 2024 is a leap year — Feb 29 should be valid
        let feb28 = parse_iso8601_to_unix("2024-02-28T00:00:00Z").unwrap();
        let feb29 = parse_iso8601_to_unix("2024-02-29T00:00:00Z").unwrap();
        assert_eq!(
            feb29 - feb28,
            86400,
            "Feb 29 should be exactly 1 day after Feb 28"
        );
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_to_unix("not a date").is_none());
        assert!(parse_iso8601_to_unix("2022-01-01").is_none()); // no time
        assert!(parse_iso8601_to_unix("").is_none());
    }

    #[test]
    fn test_era_name_all() {
        assert_eq!(era_name(0), "Byron");
        assert_eq!(era_name(1), "Shelley");
        assert_eq!(era_name(2), "Allegra");
        assert_eq!(era_name(3), "Mary");
        assert_eq!(era_name(4), "Alonzo");
        assert_eq!(era_name(5), "Babbage");
        assert_eq!(era_name(6), "Conway");
        assert_eq!(era_name(7), "Unknown");
        assert_eq!(era_name(99), "Unknown");
    }

    // ── slot-number: inline slot arithmetic ──────────────────────────────────

    /// Helper that replicates the slot-number formula from the SlotNumber handler.
    /// Extracted here so we can test it independently of the node connection.
    ///
    /// Parameters match the handler's local variables:
    ///   system_start_unix  – Unix seconds of the system start (GetSystemStart)
    ///   zero_slot          – Slot number at the start of the current era
    ///   zero_time_ms       – Milliseconds since system start at era start
    ///   slot_length_ms     – Duration of a slot in milliseconds
    ///   target_unix        – The UTC Unix timestamp we are converting
    fn compute_slot(
        system_start_unix: u64,
        zero_slot: u64,
        zero_time_ms: u64,
        slot_length_ms: u64,
        target_unix: u64,
    ) -> u64 {
        let era_start_unix = system_start_unix + zero_time_ms / 1_000;
        let elapsed_secs = target_unix - era_start_unix;
        let elapsed_ms = elapsed_secs * 1_000;
        let slot_offset = elapsed_ms / slot_length_ms;
        zero_slot + slot_offset
    }

    #[test]
    fn test_slot_number_preview_genesis() {
        // Preview testnet parameters:
        //   system start  = 2022-10-25T00:00:00Z = 1666656000 Unix
        //   Conway era start: zero_slot=4_924_800, zero_time_ms (approximate)
        //
        // For a simpler unit test we use a synthetic single-era chain where the
        // only era starts at slot 0 and the system start is the Unix epoch.
        let system_start: u64 = 0; // 1970-01-01T00:00:00Z
        let zero_slot: u64 = 0;
        let zero_time_ms: u64 = 0;
        let slot_length_ms: u64 = 1_000; // 1 second per slot

        // One hour after genesis → 3600 slots.
        let target = 3600u64;
        let slot = compute_slot(
            system_start,
            zero_slot,
            zero_time_ms,
            slot_length_ms,
            target,
        );
        assert_eq!(slot, 3600);
    }

    #[test]
    fn test_slot_number_with_era_offset() {
        // Chain with a Shelley hard-fork at slot 4_492_800 (≈ mainnet Shelley).
        // zero_time_ms is the total milliseconds elapsed from genesis to Shelley.
        let system_start: u64 = 1_506_203_091; // approximate Cardano mainnet genesis
        let zero_slot: u64 = 4_492_800;
        // Shelley started ~89 days after Byron at 1s/slot: 4_492_800_000 ms
        let zero_time_ms: u64 = 4_492_800_000;
        let slot_length_ms: u64 = 1_000; // 1s/slot in Shelley

        // Query a timestamp that is exactly 10 slots after Shelley start.
        let era_start_unix = system_start + zero_time_ms / 1_000;
        let target = era_start_unix + 10;
        let slot = compute_slot(
            system_start,
            zero_slot,
            zero_time_ms,
            slot_length_ms,
            target,
        );
        assert_eq!(
            slot, 4_492_810,
            "ten seconds past era start = ten slots past zero_slot"
        );
    }

    #[test]
    fn test_slot_number_slot_boundary() {
        // Verify that the floor division truncates correctly: a timestamp that
        // is 1.5 slot-lengths past zero should yield slot 1 (not 2).
        let slot = compute_slot(0, 0, 0, 2_000, 3); // target = 3s, slot_length = 2s
        assert_eq!(slot, 1, "floor(3000ms / 2000ms) = 1");
    }

    // ── Task 6: slot_to_utc ─────────────────────────────────────────────────

    #[test]
    fn test_slot_to_utc() {
        // Preview systemStart = 2022-10-25T00:00:00Z = Unix 1666656000, 1s slots.
        assert_eq!(slot_to_utc(0, 1666656000, 1), "2022-10-25T00:00:00Z");
        assert_eq!(slot_to_utc(86400, 1666656000, 1), "2022-10-26T00:00:00Z");
        assert_eq!(slot_to_utc(3600, 1666656000, 1), "2022-10-25T01:00:00Z");
    }

    // ── Task 2: Protocol State CBOR Parser ──────────────────────────────────

    #[test]
    fn test_parse_protocol_state_nonces() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).ok(); // MsgResult [4, payload]
        enc.u32(4).ok(); // tag 4 = MsgResult (not 6 which is MsgReAcquire)
        enc.array(1).ok(); // HFC EitherMismatch success: array(1)[result]
        enc.array(2).ok(); // Versioned
        enc.u8(0).ok();
        enc.array(7).ok(); // PraosState
                           // [0] lastSlot = Origin
        enc.array(1).ok();
        enc.u8(0).ok();
        // [1] ocertCounters = empty map
        enc.map(0).ok();
        // [2] evolvingNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();
        // [3] candidateNonce = Nonce(0xBB * 32)
        enc.array(2).ok();
        enc.u8(1).ok();
        enc.bytes(&[0xBB; 32]).ok();
        // [4] epochNonce = Nonce(0xAA * 32)
        enc.array(2).ok();
        enc.u8(1).ok();
        enc.bytes(&[0xAA; 32]).ok();
        // [5] labNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();
        // [6] lastEpochBlockNonce = NeutralNonce
        enc.array(1).ok();
        enc.u8(0).ok();

        let nonces = parse_protocol_state_nonces(&buf).unwrap();
        assert_eq!(nonces.epoch_nonce, [0xAA; 32]);
        assert_eq!(nonces.candidate_nonce, [0xBB; 32]);
    }

    // ── Task 3: Stake Snapshot Parser ───────────────────────────────────────

    #[test]
    fn test_parse_stake_for_pool() {
        let pool_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef01";
        let pool_bytes = hex::decode(pool_id).unwrap();

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).ok(); // MsgResult [4, payload]
        enc.u32(4).ok(); // tag 4 = MsgResult
        enc.array(1).ok(); // HFC EitherMismatch success: array(1)[result]
        enc.array(4).ok(); // stake snapshot

        // pool map with 1 entry
        enc.map(1).ok();
        enc.bytes(&pool_bytes).ok();
        enc.array(3).ok();
        enc.u64(5_000_000_000).ok(); // mark
        enc.u64(4_000_000_000).ok(); // set
        enc.u64(3_000_000_000).ok(); // go

        // totals
        enc.u64(100_000_000_000).ok(); // mark_total
        enc.u64(90_000_000_000).ok(); // set_total
        enc.u64(80_000_000_000).ok(); // go_total

        // --current (use set)
        let info = parse_stake_for_pool(&buf, pool_id, false).unwrap();
        assert_eq!(info.pool_stake, 4_000_000_000);
        assert_eq!(info.total_active_stake, 90_000_000_000);

        // --next (use mark)
        let info = parse_stake_for_pool(&buf, pool_id, true).unwrap();
        assert_eq!(info.pool_stake, 5_000_000_000);
        assert_eq!(info.total_active_stake, 100_000_000_000);
    }
}
