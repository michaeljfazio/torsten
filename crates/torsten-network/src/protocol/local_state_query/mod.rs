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

// ─── Wire format message tags (matching Haskell ouroboros-network) ───
//
// Tags 0-7 are the base LocalStateQuery protocol messages.
// Tags 8-11 are V16+ extensions for VolatileTip/ImmutableTip acquire targets.
//
// Base protocol:
//   0: MsgAcquire (SpecificPoint)  [0, point]
//   1: MsgAcquired                 [1]
//   2: MsgFailure                  [2, failure_code]
//   3: MsgQuery                    [3, query]
//   4: MsgResult                   [4, result]
//   5: MsgRelease                  [5]
//   6: MsgReAcquire (SpecificPoint) [6, point]
//   7: MsgDone                     [7]
//
// V16+ extensions:
//   8:  MsgAcquire (VolatileTip)    [8]
//   9:  MsgReAcquire (VolatileTip)  [9]
//   10: MsgAcquire (ImmutableTip)   [10]
//   11: MsgReAcquire (ImmutableTip) [11]

/// MsgAcquire (SpecificPoint) = [0, point]
pub const TAG_ACQUIRE_SPECIFIC: u64 = 0;
/// MsgAcquired = [1]
pub const TAG_ACQUIRED: u64 = 1;
/// MsgFailure = [2, reason]
pub const TAG_FAILURE: u64 = 2;
/// MsgQuery = [3, query]
pub const TAG_QUERY: u64 = 3;
/// MsgResult = [4, result]
pub const TAG_RESULT: u64 = 4;
/// MsgRelease = [5]
pub const TAG_RELEASE: u64 = 5;
/// MsgReAcquire (SpecificPoint) = [6, point]
pub const TAG_REACQUIRE_SPECIFIC: u64 = 6;
/// MsgDone = [7]
pub const TAG_DONE: u64 = 7;
/// MsgAcquire (VolatileTip) = [8]
pub const TAG_ACQUIRE_VOLATILE: u64 = 8;
/// MsgReAcquire (VolatileTip) = [9]
pub const TAG_REACQUIRE_VOLATILE: u64 = 9;
/// MsgAcquire (ImmutableTip) = [10]
pub const TAG_ACQUIRE_IMMUTABLE: u64 = 10;
/// MsgReAcquire (ImmutableTip) = [11]
pub const TAG_REACQUIRE_IMMUTABLE: u64 = 11;
