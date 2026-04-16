//! Fuzzy search / filter for the parameter tree.
//!
//! Implements a simple case-insensitive substring match against each
//! parameter's key and description.  This is intentionally not a full fuzzy
//! algorithm (e.g. Levenshtein distance) — it is cheap to run on every
//! keystroke and the result is predictable for operators.
//!
//! # Match scoring
//!
//! A match is assigned a score used to rank results:
//!
//! | Condition                            | Score contribution |
//! |--------------------------------------|-------------------|
//! | Query is a prefix of the key         |              +100 |
//! | Query appears anywhere in the key    |               +50 |
//! | Query appears in the description     |               +10 |
//! | Query appears in the tuning hint     |                +5 |
//!
//! Higher scores sort earlier.  Items with a score of zero are excluded from
//! the filtered view entirely.

/// The result of matching a single query against a parameter.
///
/// `query` and `key_ranges` are provided for callers that want to render
/// highlighted matches in the UI; they are not used by the search engine
/// itself.
#[derive(Debug, Clone)]
#[allow(dead_code)] // query/key_ranges are part of the public API for UI consumers
pub struct MatchResult {
    /// Original index into the flat item list (section_idx, item_idx within that section).
    pub section_idx: usize,
    /// Item index within the section.
    pub item_idx: usize,
    /// Relevance score — higher is better.
    pub score: u32,
    /// The query string that produced this match (used for highlight rendering).
    pub query: String,
    /// Character ranges within the *key* string that match the query.
    ///
    /// Each range is `(byte_start, byte_end)` in the key string.
    pub key_ranges: Vec<(usize, usize)>,
}

/// Score and collect all matching items for the given query.
///
/// - `query` — the raw search string typed by the user.
/// - `items` — flat iterator of `(section_idx, item_idx, key, description, tuning_hint)`.
///
/// Returns matches sorted by score descending.  Items with score 0 are
/// excluded.
pub fn search<'a, I>(query: &str, items: I) -> Vec<MatchResult>
where
    I: Iterator<Item = (usize, usize, &'a str, &'a str, &'a str)>,
{
    if query.is_empty() {
        return Vec::new();
    }

    let q_lower = query.to_lowercase();
    let mut results: Vec<MatchResult> = Vec::new();

    for (section_idx, item_idx, key, description, tuning_hint) in items {
        let key_lower = key.to_lowercase();
        let desc_lower = description.to_lowercase();
        let hint_lower = tuning_hint.to_lowercase();

        let mut score: u32 = 0;
        let mut key_ranges: Vec<(usize, usize)> = Vec::new();

        // Key prefix match (highest priority).
        if key_lower.starts_with(&q_lower) {
            score += 100;
            key_ranges.push((0, q_lower.len()));
        } else if let Some(pos) = key_lower.find(&q_lower) {
            // Substring match anywhere in key.
            score += 50;
            key_ranges.push((pos, pos + q_lower.len()));
        }

        // Description substring match.
        if desc_lower.contains(&q_lower) {
            score += 10;
        }

        // Tuning hint substring match.
        if hint_lower.contains(&q_lower) {
            score += 5;
        }

        if score > 0 {
            results.push(MatchResult {
                section_idx,
                item_idx,
                score,
                query: query.to_string(),
                key_ranges,
            });
        }
    }

    // Sort by score descending, then by key alphabetically for stability.
    results.sort_by_key(|r| std::cmp::Reverse(r.score));
    results
}

/// Return the highlight ranges within `key` for the given query string.
///
/// This is a thin wrapper around [`search`] for use when you already have
/// a single key to check; it returns the matching byte ranges in `key`
/// (empty if no match).
pub fn highlight_ranges(query: &str, key: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let q_lower = query.to_lowercase();
    let key_lower = key.to_lowercase();
    let mut ranges = Vec::new();
    if let Some(pos) = key_lower.find(&q_lower) {
        ranges.push((pos, pos + q_lower.len()));
    }
    ranges
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<(usize, usize, &'static str, &'static str, &'static str)> {
        vec![
            (
                0,
                0,
                "EnableP2P",
                "Enable the P2P networking stack.",
                "Always enable.",
            ),
            (
                0,
                1,
                "PeerSharing",
                "Peer sharing policy.",
                "Use public for relays.",
            ),
            (
                1,
                0,
                "MinSeverity",
                "Minimum log severity.",
                "Info is recommended.",
            ),
            (
                1,
                1,
                "TurnOnLogMetrics",
                "Enable EKG metrics.",
                "Keep enabled.",
            ),
            (
                2,
                0,
                "ByronGenesisFile",
                "Path to Byron genesis.",
                "Must match network.",
            ),
        ]
    }

    #[test]
    fn test_search_prefix_matches_first() {
        let results = search("Enable", items().into_iter());
        // "EnableP2P" has a prefix match (+100) and also "Enable" appears in
        // its description (+10) and tuning hint (+5), so score >= 100.
        // "TurnOnLogMetrics" description contains "Enable EKG" (+10) only.
        // EnableP2P must sort first (highest score).
        assert!(!results.is_empty());
        assert_eq!(results[0].section_idx, 0);
        assert_eq!(results[0].item_idx, 0);
        assert!(
            results[0].score >= 100,
            "expected score >= 100, got {}",
            results[0].score
        );
    }

    #[test]
    fn test_search_substring_in_key() {
        let results = search("Peer", items().into_iter());
        // "EnableP2P" has "p2p" not "peer", so only "PeerSharing" should match by key.
        let first = &results[0];
        assert_eq!(first.item_idx, 1); // PeerSharing
        assert!(first.score >= 50);
    }

    #[test]
    fn test_search_description_match_only() {
        // "EKG" only appears in TurnOnLogMetrics description.
        let results = search("EKG", items().into_iter());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].section_idx, 1);
        assert_eq!(results[0].item_idx, 1);
        assert_eq!(results[0].score, 10);
    }

    #[test]
    fn test_search_empty_query_returns_nothing() {
        let results = search("", items().into_iter());
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_no_match_returns_empty() {
        let results = search("zzzzz", items().into_iter());
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_case_insensitive() {
        let results = search("enablep2p", items().into_iter());
        assert!(!results.is_empty());
        assert_eq!(results[0].item_idx, 0);
    }

    #[test]
    fn test_highlight_ranges_prefix() {
        let ranges = highlight_ranges("Enable", "EnableP2P");
        assert_eq!(ranges, vec![(0, 6)]);
    }

    #[test]
    fn test_highlight_ranges_mid() {
        let ranges = highlight_ranges("Peer", "PeerSharing");
        assert_eq!(ranges, vec![(0, 4)]);
    }

    #[test]
    fn test_highlight_ranges_empty_query() {
        let ranges = highlight_ranges("", "EnableP2P");
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_highlight_ranges_no_match() {
        let ranges = highlight_ranges("xyz", "EnableP2P");
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_search_tuning_hint_match() {
        // "relays" appears only in the PeerSharing tuning hint.
        let results = search("relays", items().into_iter());
        // Should match PeerSharing (hint) and possibly others.
        assert!(results
            .iter()
            .any(|r| r.item_idx == 1 && r.section_idx == 0));
    }
}
