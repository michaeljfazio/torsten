//! N2C LocalStateQuery — ledger state query protocol.
//!
//! The most complex N2C protocol. Supports 39 Shelley BlockQuery tags (0-38),
//! plus QueryAnytime and QueryHardFork queries.
//!
//! ## State machine
//! ```text
//! StIdle ──MsgAcquire(target)──► StAcquiring
//! StAcquiring ──MsgAcquired──► StAcquired
//! StAcquiring ──MsgFailure(reason)──► StIdle
//! StAcquired ──MsgQuery(query)──► StQuerying
//! StQuerying ──MsgResult(result)──► StAcquired
//! StAcquired ──MsgReAcquire(target)──► StAcquiring
//! StAcquired ──MsgRelease──► StIdle
//! StIdle ──MsgDone──► StDone
//! ```
//!
//! ## Acquire targets
//! - `[0, point]` → SpecificPoint
//! - `[8]` → VolatileTip (always succeeds)
//! - `[10]` → ImmutableTip (V16+, always succeeds)
//!
//! ## Query wrapping
//! BlockQuery results are wrapped in HFC `QueryIfCurrent` success envelope: `[1, result]`.
//! QueryAnytime and QueryHardFork results are sent unwrapped.

pub mod encoding;
pub mod server;

/// Acquire target for LocalStateQuery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireTarget {
    /// Acquire a specific point on the chain.
    SpecificPoint(crate::codec::Point),
    /// Acquire the current volatile (chain) tip.
    VolatileTip,
    /// Acquire the immutable tip (V16+).
    ImmutableTip,
}

/// Failure reason for MsgFailure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireFailure {
    /// The requested point is too old (before the immutable tip).
    PointTooOld,
    /// The requested point is not on the current chain.
    PointNotOnChain,
}

/// LocalStateQuery protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateQueryState {
    /// Client has agency — can acquire, release, or terminate.
    StIdle,
    /// Server processing acquire — will respond with acquired or failure.
    StAcquiring,
    /// State acquired — client can query or release.
    StAcquired,
    /// Server processing query — will respond with result.
    StQuerying,
    /// Terminal state.
    StDone,
}

// ─── Wire format message tags ───

/// MsgAcquire = [8, target] (V16+: specific, volatile, immutable)
pub const TAG_ACQUIRE: u64 = 8;
/// MsgAcquired = [4]
pub const TAG_ACQUIRED: u64 = 4;
/// MsgFailure = [5, reason]
pub const TAG_FAILURE: u64 = 5;
/// MsgQuery = [3, query]
pub const TAG_QUERY: u64 = 3;
/// MsgResult = [6, result]
pub const TAG_RESULT: u64 = 6;
/// MsgRelease = [7]
pub const TAG_RELEASE: u64 = 7;
/// MsgReAcquire = [9, target] / [11, target]
pub const TAG_REACQUIRE: u64 = 9;
/// MsgDone = [0]
pub const TAG_DONE: u64 = 0;
