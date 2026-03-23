use torsten_primitives::block::{BlockHeader, Point, Tip};
use torsten_primitives::era::Era;
use torsten_primitives::hash::{blake2b_224, BlockHeaderHash};

/// Chain selection rule implementing Ouroboros chain preference.
///
/// Supports two distinct modes matching the Haskell cardano-node:
///
/// - **Byron (OBFT)**: Chains are compared by *density* — the ratio of blocks
///   to slots. A denser chain is preferred because Byron used a round-robin
///   (OBFT) protocol where honest chains should fill most slots.
///
/// - **Praos (Shelley+)**: Chains are compared by *length* (block number).
///   Longer chains are always preferred because Praos is a longest-chain
///   protocol.
///
/// ## Tiebreaker (Praos only)
///
/// When chains are equal length, the Cardano Blueprint specifies a structured
/// tiebreaker to prevent geographic centralization incentives:
///
/// 1. **Same stake pool** (same issuer VRF key hash): the block with the
///    **higher opcert sequence number** wins. Since valid opcerts can only
///    increment by 1 per block, this is a deterministic and fair rule.
///
/// 2. **Different stake pools**: the block with the **lower VRF output value**
///    wins. In Conway (protocol ≥ 9), this comparison is only applied when the
///    two tip blocks are within `slot_window` slots of each other — this
///    prevents very late blocks from winning against already-adopted chains.
///
/// For Byron, when density is equal, we fall back to comparing block header
/// hashes (lower hash wins), since Byron has no VRF or opcert concept.
pub struct ChainSelection {
    pub current_tip: Tip,
}

impl ChainSelection {
    pub fn new() -> Self {
        ChainSelection {
            current_tip: Tip::origin(),
        }
    }

    /// Compare two chain candidates by block number only (legacy Praos rule).
    ///
    /// Does NOT perform deterministic tie-breaking. Retained for backward
    /// compatibility — callers that don't have header context can still use
    /// this, but should prefer [`prefer_chain_with_headers`] for spec
    /// compliance.
    pub fn prefer(&self, candidate: &Tip) -> ChainPreference {
        match (&self.current_tip.point, &candidate.point) {
            (Point::Origin, Point::Origin) => ChainPreference::Equal,
            (Point::Origin, _) => ChainPreference::PreferCandidate,
            (_, Point::Origin) => ChainPreference::PreferCurrent,
            _ => {
                if candidate.block_number > self.current_tip.block_number {
                    ChainPreference::PreferCandidate
                } else if candidate.block_number < self.current_tip.block_number {
                    ChainPreference::PreferCurrent
                } else {
                    ChainPreference::Equal
                }
            }
        }
    }

    /// Full chain preference with era-aware comparison and deterministic
    /// tie-breaking using block header metadata.
    ///
    /// This is the authoritative comparison implementing the Cardano Blueprint
    /// tiebreaker rules:
    ///
    /// - **Byron**: density comparison; equal-density falls back to lower
    ///   block header hash.
    /// - **Shelley–Babbage**: length comparison; equal-length tiebreaker is
    ///   same-pool (higher opcert wins) or different-pool (lower VRF wins),
    ///   with no slot-distance restriction.
    /// - **Conway+**: same as Shelley–Babbage but the cross-pool VRF comparison
    ///   is only applied when the two tip slots are within `slot_window` of
    ///   each other. When the slot difference exceeds `slot_window`, the
    ///   existing (already-selected) chain is preferred, preventing very late
    ///   blocks from displacing the current selection.
    ///
    /// `slot_window` should be set to `3k/f` (the stability window). Pass `u64::MAX`
    /// to disable the Conway slot-distance constraint (matches pre-Conway behavior).
    pub fn prefer_chain_with_headers(
        &self,
        candidate: &Tip,
        current_header: &BlockHeader,
        candidate_header: &BlockHeader,
        era: Era,
        slot_window: u64,
    ) -> ChainPreference {
        match (&self.current_tip.point, &candidate.point) {
            (Point::Origin, Point::Origin) => ChainPreference::Equal,
            (Point::Origin, _) => ChainPreference::PreferCandidate,
            (_, Point::Origin) => ChainPreference::PreferCurrent,
            _ => {
                let primary = if era == Era::Byron {
                    self.compare_density(candidate)
                } else {
                    self.compare_length(candidate)
                };

                match primary {
                    ChainPreference::Equal => {
                        if era == Era::Byron {
                            // Byron has no VRF/opcert — use header hash as a
                            // deterministic tiebreaker.
                            hash_tiebreak(
                                &current_header.header_hash,
                                &candidate_header.header_hash,
                            )
                        } else {
                            praos_tiebreak(current_header, candidate_header, era, slot_window)
                        }
                    }
                    other => other,
                }
            }
        }
    }

    /// Full chain preference with era-aware comparison and deterministic
    /// tie-breaking.
    ///
    /// - In **Byron** era: compares chain density (blocks / slots). A chain
    ///   covering fewer slots with the same number of blocks is denser.
    /// - In **Praos** eras (Shelley+): compares chain length (block number).
    /// - **Tiebreaker** (both eras): lower block header hash wins.
    ///
    /// This method uses only header hashes for tiebreaking, which is a
    /// simplified rule. For full spec compliance, use
    /// [`prefer_chain_with_headers`] which applies the proper opcert/VRF rules.
    ///
    /// `current_hash` and `candidate_hash` are the header hashes of the tip
    /// blocks of the current and candidate chains respectively.
    pub fn prefer_chain(
        &self,
        candidate: &Tip,
        era: Era,
        current_hash: &BlockHeaderHash,
        candidate_hash: &BlockHeaderHash,
    ) -> ChainPreference {
        match (&self.current_tip.point, &candidate.point) {
            (Point::Origin, Point::Origin) => ChainPreference::Equal,
            (Point::Origin, _) => ChainPreference::PreferCandidate,
            (_, Point::Origin) => ChainPreference::PreferCurrent,
            _ => {
                let primary = if era == Era::Byron {
                    self.compare_density(candidate)
                } else {
                    self.compare_length(candidate)
                };

                match primary {
                    ChainPreference::Equal => hash_tiebreak(current_hash, candidate_hash),
                    other => other,
                }
            }
        }
    }

    /// Check if a candidate chain would trigger a switch (legacy API).
    pub fn should_switch(&self, candidate: &Tip) -> bool {
        matches!(self.prefer(candidate), ChainPreference::PreferCandidate)
    }

    /// Check if a candidate chain would trigger a switch using full
    /// era-aware comparison with deterministic tiebreaking.
    pub fn should_switch_chain(
        &self,
        candidate: &Tip,
        era: Era,
        current_hash: &BlockHeaderHash,
        candidate_hash: &BlockHeaderHash,
    ) -> bool {
        matches!(
            self.prefer_chain(candidate, era, current_hash, candidate_hash),
            ChainPreference::PreferCandidate
        )
    }

    /// Check if a candidate chain would trigger a switch using the full
    /// spec-compliant tiebreaker with opcert/VRF comparison.
    pub fn should_switch_chain_with_headers(
        &self,
        candidate: &Tip,
        current_header: &BlockHeader,
        candidate_header: &BlockHeader,
        era: Era,
        slot_window: u64,
    ) -> bool {
        matches!(
            self.prefer_chain_with_headers(
                candidate,
                current_header,
                candidate_header,
                era,
                slot_window
            ),
            ChainPreference::PreferCandidate
        )
    }

    /// Update the current tip.
    pub fn set_tip(&mut self, tip: Tip) {
        self.current_tip = tip;
    }

    /// Compare chains by block number (Praos longest-chain rule).
    fn compare_length(&self, candidate: &Tip) -> ChainPreference {
        if candidate.block_number > self.current_tip.block_number {
            ChainPreference::PreferCandidate
        } else if candidate.block_number < self.current_tip.block_number {
            ChainPreference::PreferCurrent
        } else {
            ChainPreference::Equal
        }
    }

    /// Compare chains by density (Byron OBFT rule).
    ///
    /// Density = block_count / slot_span. We compare using cross-multiplication
    /// to avoid floating-point: chain A is denser than B when
    ///   A.blocks * B.slots > B.blocks * A.slots
    ///
    /// If both chains span 0 slots (single genesis blocks), fall back to
    /// block number comparison.
    fn compare_density(&self, candidate: &Tip) -> ChainPreference {
        let current_slot = self.current_tip.point.slot().map(|s| s.0).unwrap_or(0);
        let candidate_slot = candidate.point.slot().map(|s| s.0).unwrap_or(0);

        let current_blocks = self.current_tip.block_number.0;
        let candidate_blocks = candidate.block_number.0;

        // If either chain has zero slot span, fall back to block count
        if current_slot == 0 && candidate_slot == 0 {
            return self.compare_length(candidate);
        }

        // Density comparison via cross-multiplication to avoid floating point:
        // candidate_density > current_density
        // ⟺ candidate_blocks / candidate_slot > current_blocks / current_slot
        // ⟺ candidate_blocks * current_slot > current_blocks * candidate_slot
        //
        // Using u128 to prevent overflow for large slot numbers
        let lhs = (candidate_blocks as u128) * (current_slot as u128);
        let rhs = (current_blocks as u128) * (candidate_slot as u128);

        if lhs > rhs {
            ChainPreference::PreferCandidate
        } else if lhs < rhs {
            ChainPreference::PreferCurrent
        } else {
            ChainPreference::Equal
        }
    }
}

/// Praos tiebreaker for equal-length chains.
///
/// Implements the Cardano Blueprint tiebreaker spec:
///
/// 1. If the tip is issued by the **same stake pool** (same blake2b-224 hash
///    of the cold verification key):
///    - The block with the **higher opcert sequence number** wins.
///    - (Valid opcerts can only increment by 1, so this is deterministic.)
///
/// 2. If issued by **different stake pools**:
///    - The block with the **lower VRF output value** (lexicographic byte
///      comparison) wins.
///    - In Conway era (protocol version ≥ 9): this comparison is only applied
///      when the two tip slots are within `slot_window` of each other. When
///      the slot gap exceeds `slot_window`, the current selection is preferred
///      (we do not switch to a block that arrived much later).
///    - In pre-Conway eras: the VRF comparison is applied unconditionally.
///
/// The slot-distance restriction in Conway prevents stake pools from gaming
/// the selection rule by ignoring peers' blocks when they know they can win
/// the VRF comparison — an attack that would incentivize geographic
/// centralization.
fn praos_tiebreak(
    current: &BlockHeader,
    candidate: &BlockHeader,
    era: Era,
    slot_window: u64,
) -> ChainPreference {
    // Compute pool IDs as blake2b-224 of the cold verification key (issuer_vkey).
    let current_pool = blake2b_224(&current.issuer_vkey);
    let candidate_pool = blake2b_224(&candidate.issuer_vkey);

    if current_pool == candidate_pool {
        // Same pool: higher opcert sequence number wins.
        //
        // Per the Haskell cardano-node implementation (Ouroboros.Consensus.Protocol.Praos),
        // the block with the higher `ocertN` is preferred. Since valid opcerts can only
        // increment by exactly 1, the difference will always be 0 or 1 in practice.
        let current_seq = current.operational_cert.sequence_number;
        let candidate_seq = candidate.operational_cert.sequence_number;

        if candidate_seq > current_seq {
            ChainPreference::PreferCandidate
        } else if candidate_seq < current_seq {
            ChainPreference::PreferCurrent
        } else {
            // Identical pools and identical opcert counters (same block seen twice
            // from different paths) — treat as equal.
            ChainPreference::Equal
        }
    } else {
        // Different pools: lower VRF output value wins.
        //
        // In Conway (protocol ≥ 9) only apply the VRF comparison when the two
        // tip slots are within `slot_window` of each other. This ensures that a
        // block forged much later cannot displace an already-adopted chain by
        // winning the VRF lottery — doing so would incentivize pools to ignore
        // other pools' blocks, harming geographic decentralization.
        let is_conway = era == Era::Conway || {
            // Also treat any era that would map to protocol ≥ 9 as Conway-style.
            // In practice we check the era enum directly.
            false
        };

        let apply_vrf_comparison = if is_conway {
            // Only compare VRF if slots are within the window.
            let current_slot = current.slot.0;
            let candidate_slot = candidate.slot.0;
            let slot_diff = current_slot.abs_diff(candidate_slot);
            slot_diff <= slot_window
        } else {
            // Pre-Conway: VRF comparison is unconditional.
            true
        };

        if apply_vrf_comparison {
            // Compare VRF output values lexicographically.
            // Lower value = block had "luckier" VRF draw = preferred.
            vrf_tiebreak(&current.vrf_result.output, &candidate.vrf_result.output)
        } else {
            // Slot distance exceeds window in Conway: keep current selection.
            ChainPreference::PreferCurrent
        }
    }
}

/// Compare VRF output values as byte sequences (lower = preferred).
///
/// The VRF output is a 64-byte (or 32-byte for Praos) value. We compare
/// lexicographically: the chain whose tip block has the smaller VRF output
/// is preferred. This is a deterministic rule that all nodes compute
/// identically from the block headers.
fn vrf_tiebreak(current_vrf: &[u8], candidate_vrf: &[u8]) -> ChainPreference {
    // Lexicographic byte comparison: lower VRF value wins.
    match candidate_vrf.cmp(current_vrf) {
        std::cmp::Ordering::Less => ChainPreference::PreferCandidate,
        std::cmp::Ordering::Greater => ChainPreference::PreferCurrent,
        std::cmp::Ordering::Equal => ChainPreference::Equal,
    }
}

/// Deterministic fork tiebreaker: the chain with the **lower** block header
/// hash is preferred. This matches the Haskell cardano-node behavior where
/// `compare` on `HeaderHash` is used as the ultimate tiebreaker.
///
/// This is used for Byron chains (no VRF/opcert) and as a fallback in the
/// hash-based `prefer_chain` API.
fn hash_tiebreak(
    current_hash: &BlockHeaderHash,
    candidate_hash: &BlockHeaderHash,
) -> ChainPreference {
    if candidate_hash < current_hash {
        ChainPreference::PreferCandidate
    } else if candidate_hash > current_hash {
        ChainPreference::PreferCurrent
    } else {
        // Identical hashes — truly equal (same block)
        ChainPreference::Equal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainPreference {
    PreferCurrent,
    PreferCandidate,
    Equal,
}

impl Default for ChainSelection {
    fn default() -> Self {
        Self::new()
    }
}

// ── Ouroboros Genesis density-based chain selection ──────────────────────────

/// A sliding window of block arrival slots used to compute chain density.
///
/// The Ouroboros Genesis paper defines the "genesis window" as a contiguous
/// span of `s = 3k/f` slots anchored at the intersection point of two
/// candidate chains. Within this window, the chain with more blocks (higher
/// density) wins, regardless of which chain is longer beyond the window.
///
/// This struct tracks the slots of blocks observed within one such window.
/// Slots are stored in sorted order so that stale entries (before the window
/// start) can be efficiently trimmed.
///
/// ## Invariant
///
/// Every slot in `slots` satisfies `intersection_slot < slot <= window_end`.
/// `window_end = intersection_slot + window_size`.
#[derive(Debug, Clone)]
pub struct DensityWindow {
    /// Absolute slot of the intersection point (fork origin).
    pub intersection_slot: u64,
    /// Window size in slots: `3k/f` where `k` is the security parameter
    /// and `f` is the active slot coefficient.
    pub window_size: u64,
    /// Sorted list of block slots observed within the window.
    slots: Vec<u64>,
}

impl DensityWindow {
    /// Create a new window anchored at `intersection_slot`.
    pub fn new(intersection_slot: u64, window_size: u64) -> Self {
        DensityWindow {
            intersection_slot,
            window_size,
            slots: Vec::new(),
        }
    }

    /// Return the slot at which the window closes.
    #[inline]
    pub fn end_slot(&self) -> u64 {
        self.intersection_slot.saturating_add(self.window_size)
    }

    /// Record a block at `slot`, if it falls within the open window.
    ///
    /// Blocks at or before `intersection_slot` are the shared history and
    /// are not counted — density comparison starts strictly after the fork.
    /// Blocks beyond `window_end` are ignored because the comparison only
    /// covers the genesis window.
    ///
    /// Returns `true` if the block was recorded.
    pub fn record_block(&mut self, slot: u64) -> bool {
        if slot > self.intersection_slot && slot <= self.end_slot() {
            // Maintain sorted order via insertion sort — windows are typically
            // small (a few hundred slots) and blocks arrive roughly in order.
            let pos = self.slots.partition_point(|&s| s < slot);
            self.slots.insert(pos, slot);
            true
        } else {
            false
        }
    }

    /// Number of blocks seen within the genesis window.
    #[inline]
    pub fn block_count(&self) -> u64 {
        self.slots.len() as u64
    }

    /// Returns `true` once the leading slot has passed the window end.
    ///
    /// After the window closes, density comparison is complete and normal
    /// Praos chain selection (longest-chain) resumes.
    pub fn is_closed(&self, current_tip_slot: u64) -> bool {
        current_tip_slot > self.end_slot()
    }

    /// Reset the window to a new intersection (e.g., after a rollback).
    pub fn reset(&mut self, intersection_slot: u64) {
        self.intersection_slot = intersection_slot;
        self.slots.clear();
    }
}

/// Density-based chain selection for Ouroboros Genesis.
///
/// During initial sync the node may receive two competing chains that diverge
/// at a common intersection point. Within the `s = 3k/f` slot genesis window
/// immediately following that intersection, a denser chain is preferred over a
/// sparser one — even if the sparse chain is longer beyond the window. This
/// prevents an adversary from presenting a long-but-empty chain to stall the
/// honest node.
///
/// Once the window closes (the current tip slot has passed `intersection +
/// window_size`) or the GSM transitions to `CaughtUp`, normal Praos
/// longest-chain selection is used instead.
///
/// ## Algorithm (matching Haskell `ChainSel` in `ouroboros-consensus`)
///
/// Given chains A and B diverging at slot `I`:
///
/// 1. Count blocks in `(I, I + s]` for each chain → `density_A`, `density_B`
/// 2. If `density_A > density_B` → prefer A
/// 3. If `density_A < density_B` → prefer B
/// 4. If equal density → fall back to standard Praos tiebreaker (longest chain)
///
/// The `GenesisDensityComparator` operates on a snapshot of two density
/// windows at the moment of comparison. It is deliberately stateless so it
/// can be called from multiple contexts without lock contention.
pub struct GenesisDensityComparator {
    /// Genesis window size in slots (`3k/f`).
    pub window_size: u64,
}

impl GenesisDensityComparator {
    /// Construct a comparator for the given genesis window size.
    ///
    /// `window_size` should be `3 * k / f` rounded to the nearest integer,
    /// where `k` is the security parameter and `f` is the active slot
    /// coefficient from the Shelley genesis.
    pub fn new(window_size: u64) -> Self {
        GenesisDensityComparator { window_size }
    }

    /// Compare two chains by density within the genesis window.
    ///
    /// `intersection_slot` — the last slot shared by both chains.
    /// `current_blocks_in_window` — blocks in `(I, I+s]` on the current chain.
    /// `candidate_blocks_in_window` — blocks in `(I, I+s]` on the candidate chain.
    /// `candidate_tip` — used to fall back to Praos if the window is closed.
    /// `current_praos_tip` — used for Praos fallback.
    ///
    /// Returns a `ChainPreference` according to Genesis density rules.
    pub fn compare(
        &self,
        intersection_slot: u64,
        current_blocks_in_window: u64,
        candidate_blocks_in_window: u64,
        candidate_tip: &Tip,
        current_praos_tip: &Tip,
    ) -> ChainPreference {
        // Determine whether the window is still open.
        // We use the candidate's tip slot as the "furthest point" — if the
        // candidate is still inside the window, the comparison is live.
        let window_end = intersection_slot.saturating_add(self.window_size);
        let candidate_slot = candidate_tip.point.slot().map(|s| s.0).unwrap_or(0);
        let current_slot = current_praos_tip.point.slot().map(|s| s.0).unwrap_or(0);

        // If neither chain has reached the window yet, we cannot make a density
        // judgement — fall back to Praos (no block beats genesis).
        if candidate_slot <= intersection_slot && current_slot <= intersection_slot {
            return ChainPreference::Equal;
        }

        // If both chains have fully passed the window end AND both have the
        // same block count within the window, fall back to Praos length.
        // This prevents Genesis from permanently overriding Praos selection
        // once the window is behind us.
        let window_closed_current = current_slot > window_end;
        let window_closed_candidate = candidate_slot > window_end;

        if window_closed_current && window_closed_candidate {
            // Window is behind both chains — compare by density then length.
            return self.density_then_length(
                current_blocks_in_window,
                candidate_blocks_in_window,
                current_praos_tip,
                candidate_tip,
            );
        }

        // Window is still open for at least one chain — compare partial density.
        // Normalize to the same observed slot range so that a chain that has
        // simply seen more slots does not win unfairly.
        //
        // The window spans `(intersection_slot, window_end]` — at most
        // `window_size` slots. A chain that has only produced `n` blocks in
        // `m` of those slots cannot be compared fairly with one that has
        // produced `n'` blocks in `m'` slots when `m ≠ m'`.  We therefore
        // compare using cross-multiplication on the observed slot spans.
        let current_observed = current_slot
            .saturating_sub(intersection_slot)
            .min(self.window_size);
        let candidate_observed = candidate_slot
            .saturating_sub(intersection_slot)
            .min(self.window_size);

        // Guard: if neither chain has produced any observed slots past the
        // intersection, they are equal.
        if current_observed == 0 && candidate_observed == 0 {
            return ChainPreference::Equal;
        }

        // Cross-multiply to compare rates without floating point:
        //   candidate_density > current_density
        //   ⟺ candidate_blocks / candidate_observed > current_blocks / current_observed
        //   ⟺ candidate_blocks * current_observed > current_blocks * candidate_observed
        //
        // Use u128 to prevent overflow (block counts and slot counts are u64).
        let lhs = (candidate_blocks_in_window as u128) * (current_observed as u128);
        let rhs = (current_blocks_in_window as u128) * (candidate_observed as u128);

        match lhs.cmp(&rhs) {
            std::cmp::Ordering::Greater => ChainPreference::PreferCandidate,
            std::cmp::Ordering::Less => ChainPreference::PreferCurrent,
            std::cmp::Ordering::Equal => {
                // Equal density — fall back to Praos length comparison.
                self.length_comparison(current_praos_tip, candidate_tip)
            }
        }
    }

    /// Compare density once the window is fully behind both chains, then fall
    /// back to Praos length if densities are equal.
    fn density_then_length(
        &self,
        current_in_window: u64,
        candidate_in_window: u64,
        current_tip: &Tip,
        candidate_tip: &Tip,
    ) -> ChainPreference {
        match candidate_in_window.cmp(&current_in_window) {
            std::cmp::Ordering::Greater => ChainPreference::PreferCandidate,
            std::cmp::Ordering::Less => ChainPreference::PreferCurrent,
            std::cmp::Ordering::Equal => self.length_comparison(current_tip, candidate_tip),
        }
    }

    /// Praos longest-chain fallback (block number comparison).
    fn length_comparison(&self, current: &Tip, candidate: &Tip) -> ChainPreference {
        if candidate.block_number > current.block_number {
            ChainPreference::PreferCandidate
        } else if candidate.block_number < current.block_number {
            ChainPreference::PreferCurrent
        } else {
            ChainPreference::Equal
        }
    }

    /// Compute the genesis window size from protocol parameters.
    ///
    /// `s = 3k/f` where `k` is the security parameter (typically 2160 for
    /// mainnet) and `f` is the active slot coefficient (typically 0.05).
    ///
    /// For mainnet: `3 * 2160 / 0.05 = 129_600` slots (36 hours).
    /// For preview: `3 * 2160 / 0.05 = 129_600` slots (36 hours).
    ///
    /// The result is truncated to `u64`. Callers should pass the values
    /// directly from the Shelley genesis.
    pub fn window_size_from_params(security_param_k: u64, active_slot_coeff_f: f64) -> u64 {
        if active_slot_coeff_f <= 0.0 {
            return 0;
        }
        ((3.0 * security_param_k as f64) / active_slot_coeff_f) as u64
    }
}

// ── Subsystem 3: Fragment-level chain preference and maximal candidates ───────
//
// These free functions implement the Ouroboros Praos chain selection rule at
// the level of `ChainFragment` objects rather than the legacy `Tip` / header
// pair API above.  They match the Haskell source:
//
//   • `Praos/Common.hs:comparePraos` — the four-case tiebreaker
//   • `ChainSel.hs:chainSelectionForBlock` — pick the better chain
//   • `Paths.hs:maximalCandidates` — enumerate all maximal fork paths
//
// Design note: `torsten-consensus` does NOT depend on `torsten-storage` (that
// would create a dependency cycle through `torsten-node`).  The
// `SuccessorProvider` trait is therefore the only coupling point — callers
// (node, storage layer) implement it for `VolatileDB` and inject it here.

use crate::chain_fragment::ChainFragment;

/// Compare two anchored chain fragments using the Ouroboros Praos chain
/// selection rule.
///
/// This is the authoritative entry point for Subsystem 3 chain selection,
/// matching Haskell's `comparePraos` in `Ouroboros.Consensus.Protocol.Praos.Common`.
///
/// # Rules (applied in order)
///
/// 1. **Longer chain wins** — the fragment whose tip has the higher block
///    number is preferred.  This is the primary Praos invariant: the
///    longest valid chain is always the winner.
///
/// 2. **Equal length, same pool** — if both tip headers were issued by the
///    same stake pool (identified by the blake2b-224 hash of `issuer_vkey`),
///    the block with the **higher opcert sequence number** wins.  Valid
///    opcerts increment by at most 1 per block so this is deterministic.
///
/// 3. **Equal length, different pools** — the block with the **lower VRF
///    output value** (lexicographic byte comparison) wins.  In Conway
///    (protocol version ≥ 9) this VRF comparison is only applied when the
///    two tip slots are within `slot_window` of each other; when the gap
///    exceeds the window the current chain is preferred (prevents very late
///    blocks from displacing an already-adopted chain).
///
/// Pass `u64::MAX` for `slot_window` to disable the Conway distance limit
/// (matches pre-Conway behaviour).
///
/// # Empty fragments
///
/// - If both fragments are empty (tip == anchor) the result is `Equal`.
/// - An empty fragment always loses to a non-empty one because its effective
///   block number is 0 (the anchor, which has no block number embedded).
///
/// # Haskell Reference
///
/// ```text
/// comparePraos :: PraosChainSelectConfig -> SelectView Praos b
///              -> SelectView Praos b -> Ordering
/// ```
/// in `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Protocol/Praos/Common.hs`
pub fn chain_preference(
    current: &ChainFragment,
    candidate: &ChainFragment,
    slot_window: u64,
) -> ChainPreference {
    let current_no = current.tip_block_no();
    let candidate_no = candidate.tip_block_no();

    // Step 1: compare by length (block number at tip).
    use std::cmp::Ordering;
    match candidate_no.0.cmp(&current_no.0) {
        Ordering::Greater => return ChainPreference::PreferCandidate,
        Ordering::Less => return ChainPreference::PreferCurrent,
        Ordering::Equal => {}
    }

    // Step 2: equal length — apply the Praos tiebreaker on the tip headers.
    // If either fragment is empty (no headers), use block-number only (Equal).
    let current_tip_header = current.headers().back();
    let candidate_tip_header = candidate.headers().back();

    match (current_tip_header, candidate_tip_header) {
        (Some(cur), Some(cand)) => {
            // Determine the protocol era from the current tip's protocol version.
            // Conway is major ≥ 9; we derive the Era here rather than importing
            // torsten_primitives::era directly (it's already available via the
            // block module which we import above).
            let era = cur.protocol_version.era();
            praos_tiebreak(cur, cand, era, slot_window)
        }
        // Both empty or one empty: no tiebreak possible.
        _ => ChainPreference::Equal,
    }
}

// ── SuccessorProvider trait ───────────────────────────────────────────────────

/// Abstraction over the VolatileDB successor index.
///
/// This trait allows `maximal_candidates` to be independent of the
/// `torsten-storage` crate, avoiding a circular dependency.  The concrete
/// implementation for `VolatileDB` lives in `torsten-storage` (or `torsten-node`)
/// and is injected at the call site.
///
/// # Contract
///
/// - `successors_of(hash)` returns the hashes of all blocks whose `prev_hash`
///   equals `hash`.  An empty slice means `hash` is a leaf in the DAG.
/// - `header_of(hash)` returns the `BlockHeader` for the block with that hash,
///   or `None` if the block is not in the volatile window.
///
/// Implementors must return slices/values only for blocks that are currently
/// in VolatileDB.  The ImmutableDB portion of the chain is represented by
/// the `anchor` parameter passed to `maximal_candidates`.
pub trait SuccessorProvider {
    /// Return the hashes of all volatile blocks whose parent is `parent_hash`.
    fn successors_of(
        &self,
        parent_hash: &torsten_primitives::hash::Hash32,
    ) -> Vec<torsten_primitives::hash::Hash32>;

    /// Return the `BlockHeader` for a volatile block, or `None` if unknown.
    fn header_of(
        &self,
        hash: &torsten_primitives::hash::Hash32,
    ) -> Option<torsten_primitives::block::BlockHeader>;
}

// ── maximal_candidates ───────────────────────────────────────────────────────

/// Compute all maximal chain paths through the VolatileDB that include
/// `new_block_hash`.
///
/// A *maximal* path is one that cannot be extended further within VolatileDB —
/// i.e. it ends at a leaf (a block with no successors).  This matches Haskell's
/// `maximalCandidates` in `Ouroboros.Consensus.Storage.ChainDB.Impl.Paths`.
///
/// # Algorithm
///
/// 1. Walk *backward* from `new_block_hash` toward `immutable_tip` to collect
///    the prefix of the chain that connects the new block to the anchor.
/// 2. Walk *forward* from `new_block_hash` through all successor paths (DFS),
///    collecting every leaf.
/// 3. For each leaf, build a `ChainFragment` anchored at `immutable_tip`
///    containing the full path: prefix + suffix ending at the leaf.
///
/// # Why include all forward paths?
///
/// A single new block can unlock multiple maximal paths if earlier fork blocks
/// are already in VolatileDB and the new block is their shared parent.  We must
/// evaluate all of them to find the globally preferred chain.
///
/// # Empty result
///
/// Returns an empty `Vec` if `new_block_hash` is not reachable from
/// `immutable_tip` within VolatileDB (e.g. a disconnected block).
///
/// # Haskell Reference
///
/// ```text
/// maximalCandidates :: VolatileDB m blk -> Point blk -> [BlockNo] -> m [AnchoredFragment (Header blk)]
/// ```
/// in `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/Paths.hs`
pub fn maximal_candidates(
    immutable_tip: &torsten_primitives::block::Point,
    provider: &dyn SuccessorProvider,
    new_block_hash: &torsten_primitives::hash::Hash32,
) -> Vec<ChainFragment> {
    use torsten_primitives::block::Point;
    use torsten_primitives::hash::Hash32;

    // Step 1: Walk backward from new_block_hash to immutable_tip.
    // Collect the ordered prefix of headers (oldest first) that connects
    // the immutable anchor to the new block.
    let prefix: Vec<torsten_primitives::block::BlockHeader> = {
        let mut headers = Vec::new();
        let mut current = *new_block_hash;
        loop {
            match provider.header_of(&current) {
                None => {
                    // Block not in VolatileDB — disconnected chain, abort.
                    return Vec::new();
                }
                Some(hdr) => {
                    let is_anchored = match immutable_tip {
                        Point::Origin => {
                            // Anchored at genesis: the chain is rooted at a
                            // block whose prev_hash is the zero hash.
                            hdr.prev_hash == Hash32::ZERO
                        }
                        Point::Specific(_, anchor_hash) => hdr.prev_hash == *anchor_hash,
                    };
                    headers.push(hdr.clone());
                    if is_anchored || hdr.prev_hash == current {
                        // Reached the anchor or detected a cycle (safety guard).
                        break;
                    }
                    current = hdr.prev_hash;
                }
            }
        }
        headers.reverse(); // oldest first
        headers
    };

    // If we built an empty prefix the walk didn't reach the anchor.
    // Return nothing.
    if prefix.is_empty() {
        return Vec::new();
    }

    // Step 2: Walk forward from new_block_hash through all successor paths.
    // We enumerate every maximal path (leaf) reachable from new_block_hash.
    //
    // We use an iterative DFS.  Each stack entry is a partial suffix
    // (most-recent header last).  When a node has no successors we record the
    // completed suffix as a leaf.
    let mut leaf_suffixes: Vec<Vec<torsten_primitives::block::BlockHeader>> = Vec::new();

    // Stack: (current_hash, headers_accumulated_so_far_in_this_path)
    let mut stack: Vec<(Hash32, Vec<torsten_primitives::block::BlockHeader>)> = Vec::new();
    stack.push((*new_block_hash, Vec::new()));

    while let Some((hash, mut path)) = stack.pop() {
        if let Some(hdr) = provider.header_of(&hash) {
            let succs = provider.successors_of(&hash);
            if succs.is_empty() {
                // Leaf node — this is a maximal tip.
                path.push(hdr);
                leaf_suffixes.push(path);
            } else {
                path.push(hdr);
                for succ_hash in succs {
                    stack.push((succ_hash, path.clone()));
                }
            }
        }
        // If header_of returns None for a successor, that path is orphaned;
        // we simply don't emit it.
    }

    // Step 3: Build one ChainFragment per leaf.
    leaf_suffixes
        .into_iter()
        .map(|suffix| {
            // Full ordered path: prefix (up to new_block) + suffix starting at new_block.
            // The prefix already ends with the new block's header.  The suffix
            // starts with the new block (DFS includes it), so we skip it to avoid
            // duplication.
            let mut all_headers: Vec<torsten_primitives::block::BlockHeader> = prefix.to_vec();
            // Skip the first element of suffix (it duplicates new_block_hash).
            all_headers.extend(suffix.into_iter().skip(1));
            ChainFragment::from_headers(immutable_tip.clone(), all_headers)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::block::{OperationalCert, ProtocolVersion, VrfOutput};
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    fn make_tip(block_no: u64, slot: u64) -> Tip {
        Tip {
            point: Point::Specific(SlotNo(slot), Hash32::from_bytes([block_no as u8; 32])),
            block_number: BlockNo(block_no),
        }
    }

    fn make_tip_with_hash(block_no: u64, slot: u64, hash: Hash32) -> Tip {
        Tip {
            point: Point::Specific(SlotNo(slot), hash),
            block_number: BlockNo(block_no),
        }
    }

    /// Build a minimal BlockHeader for tiebreaker testing.
    ///
    /// `issuer_vkey` determines the pool ID (blake2b-224 of these bytes).
    /// `opcert_seq` is the operational certificate sequence number.
    /// `vrf_output` is the VRF output bytes used for cross-pool comparison.
    fn make_header(
        block_no: u64,
        slot: u64,
        hash_bytes: [u8; 32],
        issuer_vkey: Vec<u8>,
        opcert_seq: u64,
        vrf_output: Vec<u8>,
    ) -> BlockHeader {
        BlockHeader {
            header_hash: Hash32::from_bytes(hash_bytes),
            prev_hash: Hash32::ZERO,
            issuer_vkey,
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: vrf_output,
                proof: vec![],
            },
            block_number: BlockNo(block_no),
            slot: SlotNo(slot),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: opcert_seq,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![],
            nonce_vrf_output: vec![],
        }
    }

    // -----------------------------------------------------------------------
    // Legacy API tests (backward compatibility)
    // -----------------------------------------------------------------------

    #[test]
    fn test_origin_vs_block() {
        let cs = ChainSelection::new();
        let candidate = make_tip(1, 100);
        assert_eq!(cs.prefer(&candidate), ChainPreference::PreferCandidate);
    }

    #[test]
    fn test_longer_chain_preferred() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let longer = make_tip(11, 210);
        assert_eq!(cs.prefer(&longer), ChainPreference::PreferCandidate);
        assert!(cs.should_switch(&longer));
    }

    #[test]
    fn test_shorter_chain_not_preferred() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let shorter = make_tip(9, 180);
        assert_eq!(cs.prefer(&shorter), ChainPreference::PreferCurrent);
        assert!(!cs.should_switch(&shorter));
    }

    #[test]
    fn test_equal_chains() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let equal = make_tip(10, 200);
        assert_eq!(cs.prefer(&equal), ChainPreference::Equal);
        assert!(!cs.should_switch(&equal));
    }

    #[test]
    fn test_both_origin() {
        let cs = ChainSelection::new();
        assert_eq!(cs.prefer(&Tip::origin()), ChainPreference::Equal);
    }

    // -----------------------------------------------------------------------
    // Praos era: longer chain preferred
    // -----------------------------------------------------------------------

    #[test]
    fn test_praos_longer_chain_preferred() {
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, current_hash));

        let candidate = make_tip_with_hash(12, 240, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_praos_shorter_chain_not_preferred() {
        let current_hash = Hash32::from_bytes([0xBB; 32]);
        let candidate_hash = Hash32::from_bytes([0xAA; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(15, 300, current_hash));

        let candidate = make_tip_with_hash(12, 240, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Babbage, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_praos_all_post_shelley_eras() {
        // Verify all Shelley+ eras use length comparison, not density
        for era in [
            Era::Shelley,
            Era::Allegra,
            Era::Mary,
            Era::Alonzo,
            Era::Babbage,
            Era::Conway,
        ] {
            let current_hash = Hash32::from_bytes([0xCC; 32]);
            let candidate_hash = Hash32::from_bytes([0xDD; 32]);

            let mut cs = ChainSelection::new();
            cs.set_tip(make_tip_with_hash(10, 200, current_hash));

            let candidate = make_tip_with_hash(11, 220, candidate_hash);
            assert_eq!(
                cs.prefer_chain(&candidate, era, &current_hash, &candidate_hash),
                ChainPreference::PreferCandidate,
                "Era {:?} should prefer longer chain",
                era
            );
        }
    }

    // -----------------------------------------------------------------------
    // Byron era: density-based chain selection
    // -----------------------------------------------------------------------

    #[test]
    fn test_byron_higher_density_preferred() {
        // Chain A (current): 8 blocks in 100 slots → density 0.08
        // Chain B (candidate): 8 blocks in 80 slots → density 0.10
        // Candidate is denser, should be preferred
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(8, 100, current_hash));

        let candidate = make_tip_with_hash(8, 80, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_lower_density_not_preferred() {
        // Chain A (current): 10 blocks in 100 slots → density 0.10
        // Chain B (candidate): 8 blocks in 100 slots → density 0.08
        // Current is denser
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(8, 100, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_byron_density_more_blocks_fewer_slots_wins() {
        // Chain A (current): 5 blocks in 50 slots → density 0.10
        // Chain B (candidate): 7 blocks in 50 slots → density 0.14
        // Candidate has more blocks in same slot range
        let current_hash = Hash32::from_bytes([0x11; 32]);
        let candidate_hash = Hash32::from_bytes([0x22; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 50, current_hash));

        let candidate = make_tip_with_hash(7, 50, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_density_cross_multiplication_large_values() {
        // Test with large slot/block numbers to verify u128 overflow protection
        // Chain A: 1,000,000 blocks in 10,000,000 slots → density 0.1
        // Chain B: 1,000,001 blocks in 10,000,000 slots → density ~0.1000001
        let current_hash = Hash32::from_bytes([0x01; 32]);
        let candidate_hash = Hash32::from_bytes([0x02; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(1_000_000, 10_000_000, current_hash));

        let candidate = make_tip_with_hash(1_000_001, 10_000_000, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_same_density_uses_tiebreak() {
        // Chain A: 10 blocks in 100 slots → density 0.10
        // Chain B: 20 blocks in 200 slots → density 0.10
        // Same density → tiebreaker by hash
        let current_hash = Hash32::from_bytes([0xFF; 32]); // higher hash
        let candidate_hash = Hash32::from_bytes([0x01; 32]); // lower hash

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(20, 200, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate, // lower hash wins
        );
    }

    #[test]
    fn test_byron_vs_praos_different_preference() {
        // Demonstrate that Byron and Praos can give different results for the
        // same pair of chains:
        // Chain A (current): 10 blocks in 100 slots (density 0.10, length 10)
        // Chain B (candidate): 9 blocks in 80 slots (density 0.1125, length 9)
        //
        // Byron prefers B (denser), Praos prefers A (longer).
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(9, 80, candidate_hash);

        // Byron: candidate is denser (9/80 > 10/100 → 900 > 800)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );

        // Praos: current is longer (10 > 9)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    // -----------------------------------------------------------------------
    // Deterministic fork tie-breaking (hash-based legacy API)
    // -----------------------------------------------------------------------

    #[test]
    fn test_tiebreak_lower_hash_wins() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, high_hash));

        let candidate = make_tip_with_hash(10, 200, low_hash);

        // Same length, lower hash candidate wins
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &high_hash, &low_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_tiebreak_higher_hash_loses() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, low_hash));

        let candidate = make_tip_with_hash(10, 200, high_hash);

        // Same length, higher hash candidate loses
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &low_hash, &high_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_tiebreak_identical_hashes() {
        let same_hash = Hash32::from_bytes([0x42; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, same_hash));

        let candidate = make_tip_with_hash(10, 200, same_hash);

        // Identical tips → Equal
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &same_hash, &same_hash),
            ChainPreference::Equal
        );
    }

    #[test]
    fn test_tiebreak_only_first_byte_differs() {
        let mut hash_a_bytes = [0x00; 32];
        let mut hash_b_bytes = [0x00; 32];
        hash_a_bytes[0] = 0x01;
        hash_b_bytes[0] = 0x02;
        let hash_a = Hash32::from_bytes(hash_a_bytes);
        let hash_b = Hash32::from_bytes(hash_b_bytes);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, hash_b));

        let candidate = make_tip_with_hash(5, 100, hash_a);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Babbage, &hash_b, &hash_a),
            ChainPreference::PreferCandidate, // hash_a < hash_b
        );
    }

    #[test]
    fn test_tiebreak_only_last_byte_differs() {
        let mut hash_a_bytes = [0x00; 32];
        let mut hash_b_bytes = [0x00; 32];
        hash_a_bytes[31] = 0x01;
        hash_b_bytes[31] = 0x02;
        let hash_a = Hash32::from_bytes(hash_a_bytes);
        let hash_b = Hash32::from_bytes(hash_b_bytes);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, hash_b));

        let candidate = make_tip_with_hash(5, 100, hash_a);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &hash_b, &hash_a),
            ChainPreference::PreferCandidate, // hash_a < hash_b
        );
    }

    #[test]
    fn test_tiebreak_does_not_override_length() {
        // Length difference should take priority over hash comparison
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(11, 220, low_hash)); // longer chain, lower hash

        let candidate = make_tip_with_hash(10, 200, high_hash); // shorter, higher hash

        // Current is longer → PreferCurrent, even though candidate hash is irrelevant
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &low_hash, &high_hash),
            ChainPreference::PreferCurrent
        );
    }

    // -----------------------------------------------------------------------
    // Edge cases (hash-based API)
    // -----------------------------------------------------------------------

    #[test]
    fn test_origin_vs_candidate_with_era() {
        let candidate_hash = Hash32::from_bytes([0xAB; 32]);
        let current_hash = Hash32::ZERO;

        let cs = ChainSelection::new(); // origin
        let candidate = make_tip_with_hash(1, 10, candidate_hash);

        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_candidate_origin_with_era() {
        let current_hash = Hash32::from_bytes([0xAB; 32]);
        let candidate_hash = Hash32::ZERO;

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, current_hash));

        assert_eq!(
            cs.prefer_chain(&Tip::origin(), Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_both_origin_with_era() {
        let hash = Hash32::ZERO;
        let cs = ChainSelection::new();
        assert_eq!(
            cs.prefer_chain(&Tip::origin(), Era::Byron, &hash, &hash),
            ChainPreference::Equal
        );
    }

    #[test]
    fn test_single_block_chain_vs_origin() {
        let hash = Hash32::from_bytes([0x42; 32]);
        let cs = ChainSelection::new();
        let single_block = make_tip_with_hash(1, 1, hash);

        assert_eq!(
            cs.prefer_chain(&single_block, Era::Byron, &Hash32::ZERO, &hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_should_switch_chain_with_tiebreak() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, high_hash));

        let candidate = make_tip_with_hash(10, 200, low_hash);
        assert!(cs.should_switch_chain(&candidate, Era::Conway, &high_hash, &low_hash));

        // Reverse: candidate has higher hash, should NOT switch
        let mut cs2 = ChainSelection::new();
        cs2.set_tip(make_tip_with_hash(10, 200, low_hash));
        let candidate2 = make_tip_with_hash(10, 200, high_hash);
        assert!(!cs2.should_switch_chain(&candidate2, Era::Conway, &low_hash, &high_hash));
    }

    #[test]
    fn test_byron_density_slot_1_block_1() {
        // Single-block chains at slot 1
        // Chain A: 1 block at slot 1 → density 1.0
        // Chain B: 1 block at slot 2 → density 0.5
        let hash_a = Hash32::from_bytes([0xAA; 32]);
        let hash_b = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(1, 2, hash_a));

        let candidate = make_tip_with_hash(1, 1, hash_b);
        // candidate density (1/1 = 1.0) > current density (1/2 = 0.5)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &hash_a, &hash_b),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_hash_tiebreak_function() {
        let low = Hash32::from_bytes([0x00; 32]);
        let high = Hash32::from_bytes([0xFF; 32]);
        let mid = Hash32::from_bytes([0x80; 32]);

        assert_eq!(hash_tiebreak(&high, &low), ChainPreference::PreferCandidate);
        assert_eq!(hash_tiebreak(&low, &high), ChainPreference::PreferCurrent);
        assert_eq!(hash_tiebreak(&low, &low), ChainPreference::Equal);
        assert_eq!(hash_tiebreak(&high, &mid), ChainPreference::PreferCandidate);
        assert_eq!(hash_tiebreak(&mid, &high), ChainPreference::PreferCurrent);
    }

    // -----------------------------------------------------------------------
    // Chain Selection Edge Cases (hash-based API)
    // -----------------------------------------------------------------------

    #[test]
    fn test_equal_length_different_hashes_tiebreaker() {
        // Two chains of equal length but different tip hashes
        let low_hash = Hash32::from_bytes([0x10; 32]);
        let high_hash = Hash32::from_bytes([0x90; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(100, 2000, high_hash));

        let candidate = make_tip_with_hash(100, 2000, low_hash);

        // Lower hash should win the tiebreak
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &high_hash, &low_hash),
            ChainPreference::PreferCandidate,
            "Lower hash candidate should win on equal-length tiebreak"
        );

        // Reverse: higher hash candidate should lose
        let mut cs2 = ChainSelection::new();
        cs2.set_tip(make_tip_with_hash(100, 2000, low_hash));
        let candidate2 = make_tip_with_hash(100, 2000, high_hash);
        assert_eq!(
            cs2.prefer_chain(&candidate2, Era::Conway, &low_hash, &high_hash),
            ChainPreference::PreferCurrent,
            "Higher hash candidate should lose on equal-length tiebreak"
        );
    }

    #[test]
    fn test_higher_block_number_lower_slot_preferred_in_praos() {
        // Praos uses block number (length), not slot
        // Chain A: 50 blocks at slot 1000
        // Chain B: 51 blocks at slot 900 (higher block count, lower slot)
        let hash_a = Hash32::from_bytes([0xAA; 32]);
        let hash_b = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(50, 1000, hash_a));

        let candidate = make_tip_with_hash(51, 900, hash_b);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &hash_a, &hash_b),
            ChainPreference::PreferCandidate,
            "Praos should prefer higher block number regardless of slot"
        );
    }

    #[test]
    fn test_byron_density_max_slot_values_no_overflow() {
        // Test with near-maximum u64 slot values to check u128 overflow protection
        let hash_a = Hash32::from_bytes([0x11; 32]);
        let hash_b = Hash32::from_bytes([0x22; 32]);

        let max_slot = u64::MAX / 2;
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(1_000_000, max_slot, hash_a));

        // Candidate has same blocks in fewer slots (higher density)
        let candidate = make_tip_with_hash(1_000_000, max_slot - 1000, hash_b);
        let result = cs.prefer_chain(&candidate, Era::Byron, &hash_a, &hash_b);
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Should handle large slot values without overflow"
        );
    }

    #[test]
    fn test_praos_block_number_zero_chains() {
        // Two chains both at block 0 but different slots
        let hash_a = Hash32::from_bytes([0x55; 32]);
        let hash_b = Hash32::from_bytes([0x33; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(0, 0, hash_a));

        // Block 0, slot 0 vs block 0, slot 5 — equal block number
        let candidate = make_tip_with_hash(0, 5, hash_b);
        let result = cs.prefer_chain(&candidate, Era::Shelley, &hash_a, &hash_b);
        // Equal block numbers → tiebreak by hash: 0x33 < 0x55
        assert_eq!(result, ChainPreference::PreferCandidate);
    }

    // -----------------------------------------------------------------------
    // Spec-compliant tiebreaker: prefer_chain_with_headers
    // -----------------------------------------------------------------------

    #[test]
    fn test_same_pool_higher_opcert_wins() {
        // Two equal-length chains from the SAME pool.
        // Candidate has a higher opcert sequence number → candidate preferred.
        let pool_vkey = vec![0xAB; 32]; // same issuer for both
        let current_header = make_header(
            10,
            200,
            [0xCC; 32],
            pool_vkey.clone(),
            5,              // opcert sequence = 5
            vec![0x80; 32], // VRF output
        );
        let candidate_header = make_header(
            10,
            205,
            [0xDD; 32],
            pool_vkey.clone(),
            6,              // opcert sequence = 6 (higher)
            vec![0x90; 32], // higher VRF — but irrelevant since same pool
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            u64::MAX, // no slot-distance constraint
        );
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Same pool, higher opcert seq should prefer candidate"
        );
    }

    #[test]
    fn test_same_pool_lower_opcert_keeps_current() {
        // Two equal-length chains from the SAME pool.
        // Candidate has a lower opcert sequence number → current preferred.
        let pool_vkey = vec![0xAB; 32];
        let current_header = make_header(
            10,
            200,
            [0xCC; 32],
            pool_vkey.clone(),
            6,              // opcert sequence = 6 (higher)
            vec![0x40; 32], // lower VRF — irrelevant, same pool
        );
        let candidate_header = make_header(
            10,
            205,
            [0xDD; 32],
            pool_vkey.clone(),
            5,              // opcert sequence = 5 (lower)
            vec![0x10; 32], // even lower VRF — still irrelevant
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Same pool, lower opcert seq should keep current"
        );
    }

    #[test]
    fn test_different_pools_lower_vrf_wins() {
        // Two equal-length chains from DIFFERENT pools.
        // Candidate has a lower VRF output → candidate preferred.
        let current_pool_vkey = vec![0x01; 32]; // pool A
        let candidate_pool_vkey = vec![0x02; 32]; // pool B (different)

        let current_header = make_header(
            10,
            200,
            [0xCC; 32],
            current_pool_vkey,
            5,
            vec![0x80; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            205,
            [0xDD; 32],
            candidate_pool_vkey,
            5,
            vec![0x10; 32], // lower VRF → wins
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Babbage, // pre-Conway: no slot window restriction
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Different pools, lower VRF should prefer candidate (Babbage)"
        );
    }

    #[test]
    fn test_different_pools_higher_vrf_keeps_current() {
        // Two equal-length chains from DIFFERENT pools.
        // Candidate has a higher VRF output → current preferred.
        let current_pool_vkey = vec![0x01; 32];
        let candidate_pool_vkey = vec![0x02; 32];

        let current_header = make_header(
            10,
            200,
            [0xCC; 32],
            current_pool_vkey,
            5,
            vec![0x10; 32], // lower VRF → current wins
        );
        let candidate_header = make_header(
            10,
            205,
            [0xDD; 32],
            candidate_pool_vkey,
            5,
            vec![0x80; 32], // higher VRF → loses
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Shelley,
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Different pools, higher VRF should keep current"
        );
    }

    #[test]
    fn test_conway_vrf_within_slot_window_applies_comparison() {
        // Conway: VRF comparison applied when slots are within the window.
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        let current_header = make_header(
            10,
            1000,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xAA; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            1050,
            [0xDD; 32], // 50 slots later — within window of 100
            pool_b,
            5,
            vec![0x11; 32], // lower VRF → should win
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 1000, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 1050, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            100, // slot window = 100
        );
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Within window: lower VRF candidate should win in Conway"
        );
    }

    #[test]
    fn test_conway_vrf_outside_slot_window_keeps_current() {
        // Conway: VRF comparison NOT applied when slots exceed the window.
        // Even if candidate has lower VRF, current is preferred (arrived first).
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        let current_header = make_header(
            10,
            1000,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xAA; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            1200,
            [0xDD; 32], // 200 slots later — OUTSIDE window of 100
            pool_b,
            5,
            vec![0x01; 32], // lowest possible VRF — but slot distance is too large
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 1000, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 1200, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            100, // slot window = 100; difference is 200 > 100
        );
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Outside window: late block should NOT win in Conway even with lower VRF"
        );
    }

    #[test]
    fn test_babbage_vrf_no_slot_window_restriction() {
        // Pre-Conway (Babbage): VRF comparison applies regardless of slot distance.
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        let current_header = make_header(
            10,
            1000,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xAA; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            5000,
            [0xDD; 32], // 4000 slots later — would be outside any window
            pool_b,
            5,
            vec![0x01; 32], // lower VRF → wins in Babbage (no slot restriction)
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 1000, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 5000, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Babbage,
            129_600, // standard 3k/f window — but Babbage ignores it
        );
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Babbage: lower VRF should always win regardless of slot distance"
        );
    }

    #[test]
    fn test_longer_chain_wins_regardless_of_tiebreaker() {
        // Length always takes priority over opcert/VRF tiebreaker.
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        // Current chain is longer (block 11 vs 10)
        let current_header = make_header(
            11,
            200,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xFF; 32], // worst possible VRF — but length wins
        );
        let candidate_header = make_header(
            10,
            200,
            [0xDD; 32],
            pool_b,
            99,             // much higher opcert — irrelevant, candidate is shorter
            vec![0x00; 32], // best possible VRF — but chain is shorter
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(11, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 200, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Longer chain always wins even if candidate has better VRF and opcert"
        );
    }

    #[test]
    fn test_should_switch_chain_with_headers_uses_opcert() {
        // Verify should_switch_chain_with_headers correctly delegates to praos_tiebreak.
        let pool_vkey = vec![0xAB; 32]; // same pool
        let current_header = make_header(
            10,
            200,
            [0x00; 32],
            pool_vkey.clone(),
            3, // lower opcert
            vec![0x80; 32],
        );
        let candidate_header = make_header(
            10,
            210,
            [0x11; 32],
            pool_vkey.clone(),
            4, // higher opcert → should switch
            vec![0x80; 32],
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0x00; 32])));
        let candidate = make_tip_with_hash(10, 210, Hash32::from_bytes([0x11; 32]));

        assert!(
            cs.should_switch_chain_with_headers(
                &candidate,
                &current_header,
                &candidate_header,
                Era::Conway,
                u64::MAX,
            ),
            "Should switch when same pool and candidate has higher opcert"
        );
    }

    #[test]
    fn test_vrf_tiebreak_equal_values() {
        // Identical VRF values → equal (no preference)
        assert_eq!(
            vrf_tiebreak(&[0xAA; 32], &[0xAA; 32]),
            ChainPreference::Equal
        );
    }

    #[test]
    fn test_vrf_tiebreak_first_byte_differs() {
        // Candidate VRF first byte is lower → preferred
        assert_eq!(
            vrf_tiebreak(&[0x80; 32], &[0x01; 32]),
            ChainPreference::PreferCandidate
        );
        // Candidate VRF first byte is higher → current preferred
        assert_eq!(
            vrf_tiebreak(&[0x01; 32], &[0x80; 32]),
            ChainPreference::PreferCurrent
        );
    }

    // ===================================================================
    //  Coverage Sprint: Chain selection tiebreaker tests
    // ===================================================================

    /// Conway slot window boundary: slots differ by EXACTLY the window size.
    /// The comparison should include the boundary (slot_diff <= slot_window).
    #[test]
    fn test_conway_vrf_exactly_at_slot_window_boundary() {
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        let current_header = make_header(
            10,
            1000,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xAA; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            1100, // exactly 100 slots apart = exactly at window boundary
            [0xDD; 32],
            pool_b,
            5,
            vec![0x11; 32], // lower VRF → should win
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 1000, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 1100, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            100, // slot_window = 100; difference = 100 = exactly at boundary
        );
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "At exact window boundary (diff == window), VRF comparison should apply"
        );
    }

    /// Conway slot window: 1 slot past the boundary rejects VRF comparison.
    #[test]
    fn test_conway_vrf_one_past_slot_window() {
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        let current_header = make_header(
            10,
            1000,
            [0xCC; 32],
            pool_a,
            5,
            vec![0xAA; 32], // higher VRF
        );
        let candidate_header = make_header(
            10,
            1101, // 101 slots apart = 1 past window
            [0xDD; 32],
            pool_b,
            5,
            vec![0x01; 32], // lower VRF — but outside window
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 1000, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 1101, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            100, // window = 100; diff = 101 > 100
        );
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "One past window: VRF comparison must NOT apply"
        );
    }

    /// Same pool, identical opcert counters → Equal (no preference).
    #[test]
    fn test_same_pool_equal_opcert_is_equal() {
        let pool_vkey = vec![0x42; 32];
        let current_header = make_header(
            10,
            200,
            [0xCC; 32],
            pool_vkey.clone(),
            5,              // same opcert
            vec![0x10; 32], // different VRF — but irrelevant for same pool
        );
        let candidate_header = make_header(
            10,
            205,
            [0xDD; 32],
            pool_vkey,
            5,              // same opcert
            vec![0x80; 32], // different VRF — irrelevant
        );

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Conway,
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::Equal,
            "Same pool + same opcert = Equal"
        );
    }

    /// Different pools, equal VRF → Equal.
    #[test]
    fn test_different_pools_equal_vrf_is_equal() {
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];
        let same_vrf = vec![0x50; 32];

        let current_header = make_header(10, 200, [0xCC; 32], pool_a, 5, same_vrf.clone());
        let candidate_header = make_header(10, 205, [0xDD; 32], pool_b, 5, same_vrf);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 205, Hash32::from_bytes([0xDD; 32]));

        let result = cs.prefer_chain_with_headers(
            &candidate,
            &current_header,
            &candidate_header,
            Era::Babbage,
            u64::MAX,
        );
        assert_eq!(
            result,
            ChainPreference::Equal,
            "Different pools + equal VRF = Equal"
        );
    }

    /// Verify VRF comparison uses lexicographic byte order (not numeric).
    #[test]
    fn test_vrf_tiebreak_lexicographic_last_byte() {
        // Same prefix, differ only in last byte
        let mut vrf_a = vec![0x50; 32];
        let mut vrf_b = vec![0x50; 32];
        vrf_a[31] = 0x01;
        vrf_b[31] = 0x02;

        // vrf_a < vrf_b lexicographically
        assert_eq!(
            vrf_tiebreak(&vrf_b, &vrf_a),
            ChainPreference::PreferCandidate,
            "Lower last-byte VRF candidate should win"
        );
    }

    /// Conway + slot_window = 0: only identical slots get VRF comparison.
    #[test]
    fn test_conway_zero_slot_window() {
        let pool_a = vec![0x01; 32];
        let pool_b = vec![0x02; 32];

        // Same slot: VRF should apply
        let h1 = make_header(10, 500, [0xCC; 32], pool_a.clone(), 5, vec![0x80; 32]);
        let h2 = make_header(10, 500, [0xDD; 32], pool_b.clone(), 5, vec![0x10; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 500, Hash32::from_bytes([0xCC; 32])));
        let candidate = make_tip_with_hash(10, 500, Hash32::from_bytes([0xDD; 32]));

        assert_eq!(
            cs.prefer_chain_with_headers(&candidate, &h1, &h2, Era::Conway, 0),
            ChainPreference::PreferCandidate,
            "slot_window=0, same slot: VRF comparison should apply"
        );

        // 1 slot apart with window=0: VRF should NOT apply
        let h3 = make_header(10, 500, [0xCC; 32], pool_a, 5, vec![0x80; 32]);
        let h4 = make_header(10, 501, [0xDD; 32], pool_b, 5, vec![0x10; 32]);

        let mut cs2 = ChainSelection::new();
        cs2.set_tip(make_tip_with_hash(10, 500, Hash32::from_bytes([0xCC; 32])));
        let candidate2 = make_tip_with_hash(10, 501, Hash32::from_bytes([0xDD; 32]));

        assert_eq!(
            cs2.prefer_chain_with_headers(&candidate2, &h3, &h4, Era::Conway, 0),
            ChainPreference::PreferCurrent,
            "slot_window=0, 1 slot apart: VRF must NOT apply in Conway"
        );
    }

    /// Pre-Conway eras (Shelley through Babbage) all use VRF unconditionally.
    #[test]
    fn test_all_pre_conway_eras_unconditional_vrf() {
        for era in [
            Era::Shelley,
            Era::Allegra,
            Era::Mary,
            Era::Alonzo,
            Era::Babbage,
        ] {
            let pool_a = vec![0x01; 32];
            let pool_b = vec![0x02; 32];

            let current_header = make_header(10, 100, [0xCC; 32], pool_a, 5, vec![0xFF; 32]);
            let candidate_header = make_header(10, 99999, [0xDD; 32], pool_b, 5, vec![0x01; 32]);

            let mut cs = ChainSelection::new();
            cs.set_tip(make_tip_with_hash(10, 100, Hash32::from_bytes([0xCC; 32])));
            let candidate = make_tip_with_hash(10, 99999, Hash32::from_bytes([0xDD; 32]));

            let result = cs.prefer_chain_with_headers(
                &candidate,
                &current_header,
                &candidate_header,
                era,
                1, // tiny window — but pre-Conway ignores it
            );
            assert_eq!(
                result,
                ChainPreference::PreferCandidate,
                "Era {:?}: pre-Conway must apply VRF unconditionally regardless of slot distance",
                era
            );
        }
    }

    // -----------------------------------------------------------------------
    // Property-based tests: chain selection transitivity
    // -----------------------------------------------------------------------

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a Praos-style tip with arbitrary block number and slot.
        /// Slot is always >= block_no (at least one slot per block).
        fn arb_praos_tip() -> impl Strategy<Value = (Tip, Hash32)> {
            (1u64..10_000, 1u64..100_000).prop_flat_map(|(block_no, extra_slot)| {
                let slot = block_no.saturating_add(extra_slot);
                prop::array::uniform32(any::<u8>()).prop_map(move |hash_bytes| {
                    let hash = Hash32::from_bytes(hash_bytes);
                    let tip = Tip {
                        point: Point::Specific(SlotNo(slot), hash),
                        block_number: BlockNo(block_no),
                    };
                    (tip, hash)
                })
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(500))]

            /// Transitivity: if A > B and B > C then A > C.
            ///
            /// For the Praos longest-chain rule, "prefer" is a total order on
            /// block numbers with hash-based tiebreaking. This property verifies
            /// that the `prefer_chain` comparison is transitive for Shelley+ eras.
            #[test]
            fn prop_chain_selection_transitivity(
                (tip_a, hash_a) in arb_praos_tip(),
                (tip_b, hash_b) in arb_praos_tip(),
                (tip_c, hash_c) in arb_praos_tip(),
            ) {
                // Compare A vs B (with B as current)
                let mut cs_b = ChainSelection::new();
                cs_b.set_tip(tip_b.clone());
                let a_vs_b = cs_b.prefer_chain(&tip_a, Era::Babbage, &hash_b, &hash_a);

                // Compare B vs C (with C as current)
                let mut cs_c = ChainSelection::new();
                cs_c.set_tip(tip_c.clone());
                let b_vs_c = cs_c.prefer_chain(&tip_b, Era::Babbage, &hash_c, &hash_b);

                // If A preferred over B, and B preferred over C, then A must be preferred over C.
                if a_vs_b == ChainPreference::PreferCandidate
                    && b_vs_c == ChainPreference::PreferCandidate
                {
                    let mut cs_c2 = ChainSelection::new();
                    cs_c2.set_tip(tip_c.clone());
                    let a_vs_c = cs_c2.prefer_chain(&tip_a, Era::Babbage, &hash_c, &hash_a);
                    prop_assert_eq!(
                        a_vs_c,
                        ChainPreference::PreferCandidate,
                        "Transitivity violated: A > B and B > C but A !> C"
                    );
                }
            }

            /// Antisymmetry: if A > B then B is NOT > A.
            ///
            /// Chain preference must be antisymmetric — if we prefer A over B
            /// then we must not also prefer B over A.
            #[test]
            fn prop_chain_selection_antisymmetry(
                (tip_a, hash_a) in arb_praos_tip(),
                (tip_b, hash_b) in arb_praos_tip(),
            ) {
                let mut cs_b = ChainSelection::new();
                cs_b.set_tip(tip_b.clone());
                let a_vs_b = cs_b.prefer_chain(&tip_a, Era::Babbage, &hash_b, &hash_a);

                let mut cs_a = ChainSelection::new();
                cs_a.set_tip(tip_a.clone());
                let b_vs_a = cs_a.prefer_chain(&tip_b, Era::Babbage, &hash_a, &hash_b);

                match a_vs_b {
                    ChainPreference::PreferCandidate => {
                        // A preferred over B => B must NOT be preferred over A
                        prop_assert_ne!(
                            b_vs_a,
                            ChainPreference::PreferCandidate,
                            "Antisymmetry violated: A > B and B > A"
                        );
                    }
                    ChainPreference::PreferCurrent => {
                        // B preferred over A => A must NOT be preferred over B
                        prop_assert_ne!(
                            b_vs_a,
                            ChainPreference::PreferCurrent,
                            "Antisymmetry violated: B > A but also B < A"
                        );
                    }
                    ChainPreference::Equal => {
                        // Equal must be symmetric
                        prop_assert_eq!(
                            b_vs_a,
                            ChainPreference::Equal,
                            "Symmetry of Equal violated"
                        );
                    }
                }
            }

            /// Longer chain always wins in Praos (Shelley+).
            ///
            /// When block numbers differ, the chain with the higher block number
            /// must always be preferred, regardless of slot numbers or hashes.
            #[test]
            fn prop_longer_chain_always_preferred(
                (tip_long, hash_long) in arb_praos_tip(),
                delta in 1u64..1000,
                slot_short in 1u64..100_000,
                hash_bytes_short in prop::array::uniform32(any::<u8>()),
            ) {
                // Ensure the "short" chain is strictly shorter
                let short_block_no = tip_long.block_number.0.saturating_sub(delta);
                let hash_short = Hash32::from_bytes(hash_bytes_short);
                let tip_short = Tip {
                    point: Point::Specific(SlotNo(slot_short), hash_short),
                    block_number: BlockNo(short_block_no),
                };

                // With the shorter chain as current, the longer must be preferred
                let mut cs = ChainSelection::new();
                cs.set_tip(tip_short);
                let result = cs.prefer_chain(&tip_long, Era::Babbage, &hash_short, &hash_long);
                prop_assert_eq!(
                    result,
                    ChainPreference::PreferCandidate,
                    "Longer chain (block_no={}) not preferred over shorter (block_no={})",
                    tip_long.block_number.0,
                    short_block_no,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Genesis density: DensityWindow tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_density_window_records_within_range() {
        // Window: (100, 600]
        let mut w = DensityWindow::new(100, 500);
        assert!(!w.record_block(100)); // at intersection — not counted
        assert!(w.record_block(101)); // just inside
        assert!(w.record_block(600)); // right at end
        assert!(!w.record_block(601)); // just outside
        assert_eq!(w.block_count(), 2);
    }

    #[test]
    fn test_density_window_sorted_insertion() {
        let mut w = DensityWindow::new(0, 1000);
        w.record_block(300);
        w.record_block(100);
        w.record_block(200);
        // Slots should be sorted
        assert_eq!(w.slots, vec![100, 200, 300]);
    }

    #[test]
    fn test_density_window_is_closed() {
        let w = DensityWindow::new(1000, 500); // window ends at 1500
        assert!(!w.is_closed(1500)); // at end — window still includes this slot
        assert!(w.is_closed(1501)); // just past end
        assert!(!w.is_closed(999)); // well before end
    }

    #[test]
    fn test_density_window_reset() {
        let mut w = DensityWindow::new(100, 500);
        w.record_block(200);
        w.record_block(300);
        assert_eq!(w.block_count(), 2);

        w.reset(500);
        assert_eq!(w.intersection_slot, 500);
        assert_eq!(w.block_count(), 0);
        assert!(!w.record_block(200)); // old slot — below new intersection
        assert!(w.record_block(600)); // new window valid
    }

    #[test]
    fn test_density_window_end_slot_overflow_safe() {
        // intersection near u64::MAX should not panic
        let w = DensityWindow::new(u64::MAX - 10, 100);
        assert_eq!(w.end_slot(), u64::MAX); // saturating_add caps at MAX
    }

    // -----------------------------------------------------------------------
    // Genesis density: GenesisDensityComparator tests
    // -----------------------------------------------------------------------

    fn make_tip_at_slot(block_no: u64, slot: u64) -> Tip {
        make_tip(block_no, slot)
    }

    #[test]
    fn test_genesis_dense_candidate_wins() {
        // Intersection at slot 1000, window = 5000 slots (closes at 6000).
        // Current chain: 10 blocks in 5000 observed slots.
        // Candidate chain: 50 blocks in 5000 observed slots (much denser).
        let cmp = GenesisDensityComparator::new(5000);
        let current_tip = make_tip_at_slot(10, 6000);
        let candidate_tip = make_tip_at_slot(50, 6000);

        let result = cmp.compare(1000, 10, 50, &candidate_tip, &current_tip);
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Denser candidate should win"
        );
    }

    #[test]
    fn test_genesis_sparse_candidate_loses() {
        // Intersection at slot 1000, window = 5000 slots.
        // Current chain: 50 blocks (dense), candidate: 10 blocks (sparse).
        let cmp = GenesisDensityComparator::new(5000);
        let current_tip = make_tip_at_slot(50, 6000);
        let candidate_tip = make_tip_at_slot(10, 6000);

        let result = cmp.compare(1000, 50, 10, &candidate_tip, &current_tip);
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Sparser candidate should lose"
        );
    }

    #[test]
    fn test_genesis_equal_density_falls_back_to_length() {
        // Both chains have identical density. Candidate is longer — should win.
        let cmp = GenesisDensityComparator::new(5000);
        let current_tip = make_tip_at_slot(50, 6100);
        let candidate_tip = make_tip_at_slot(55, 6200); // longer

        // 50 blocks in 5000 slots vs 50 blocks in 5000 slots (equal density)
        let result = cmp.compare(1000, 50, 50, &candidate_tip, &current_tip);
        assert_eq!(
            result,
            ChainPreference::PreferCandidate,
            "Equal density: longer chain (55 blocks) should win"
        );
    }

    #[test]
    fn test_genesis_equal_density_equal_length() {
        // Identical density and length → Equal.
        let cmp = GenesisDensityComparator::new(5000);
        let current_tip = make_tip_at_slot(50, 6000);
        let candidate_tip = make_tip_at_slot(50, 6000);

        let result = cmp.compare(1000, 50, 50, &candidate_tip, &current_tip);
        assert_eq!(result, ChainPreference::Equal);
    }

    #[test]
    fn test_genesis_neither_chain_in_window_yet() {
        // Both chains are still at the intersection — no data to compare.
        let cmp = GenesisDensityComparator::new(5000);
        let current_tip = make_tip_at_slot(0, 1000);
        let candidate_tip = make_tip_at_slot(0, 1000);

        let result = cmp.compare(1000, 0, 0, &candidate_tip, &current_tip);
        assert_eq!(result, ChainPreference::Equal);
    }

    #[test]
    fn test_genesis_partial_window_cross_multiply() {
        // Window = 1000 slots, intersection = 0.
        // Current: 5 blocks, observed 500 slots (rate = 1/100).
        // Candidate: 6 blocks, observed 1000 slots (rate = 6/1000 = 3/500).
        // Rate C: 5/500 = 1/100 = 10/1000
        // Rate K: 6/1000
        // 10/1000 > 6/1000, so current is denser → PreferCurrent
        let cmp = GenesisDensityComparator::new(1000);
        let current_tip = make_tip_at_slot(5, 500); // 500 observed slots
        let candidate_tip = make_tip_at_slot(6, 1000); // 1000 observed slots

        // cross-mult: candidate(6) * current_obs(500) = 3000
        //             current(5) * candidate_obs(1000) = 5000
        // 3000 < 5000 → PreferCurrent
        let result = cmp.compare(0, 5, 6, &candidate_tip, &current_tip);
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Current chain is denser per observed slot"
        );
    }

    #[test]
    fn test_genesis_window_size_from_params_mainnet() {
        // Mainnet: k=2160, f=0.05 → 3*2160/0.05 = 129_600
        let s = GenesisDensityComparator::window_size_from_params(2160, 0.05);
        assert_eq!(s, 129_600, "Mainnet genesis window should be 129_600 slots");
    }

    #[test]
    fn test_genesis_window_size_zero_f() {
        // Zero active slot coefficient should not panic
        let s = GenesisDensityComparator::window_size_from_params(2160, 0.0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_genesis_longer_sparse_loses_to_denser_shorter() {
        // This is the core security property of Ouroboros Genesis:
        // An adversary presents a longer but sparse chain. The honest chain
        // is shorter but denser. The honest chain must win.
        //
        // Honest: 100 blocks in window of 5000 slots
        // Adversary: 20 blocks in window of 5000 slots, but chain is longer
        //            beyond the window (block_no = 200 vs 100).
        let cmp = GenesisDensityComparator::new(5000);
        // After the window both chains have advanced past it.
        let honest_tip = make_tip_at_slot(100, 7000); // block 100
        let adversary_tip = make_tip_at_slot(200, 8000); // block 200 — longer!

        // Honest is current, adversary is candidate.
        // Even though adversary has block_no=200 > honest block_no=100,
        // density 100 > 20 means current (honest) should be preferred.
        let result = cmp.compare(1000, 100, 20, &adversary_tip, &honest_tip);
        assert_eq!(
            result,
            ChainPreference::PreferCurrent,
            "Honest dense chain must beat longer sparse adversary chain"
        );
    }
}
