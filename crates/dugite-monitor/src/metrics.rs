//! Prometheus metrics HTTP polling and parsing.
//!
//! Fetches the Dugite node's Prometheus endpoint and parses the text exposition
//! format into a structured `MetricsSnapshot`.  Both simple gauge/counter lines
//! and histogram bucket lines (with `{le="…"}` labels) are parsed.

use std::collections::HashMap;

/// A point-in-time snapshot of all metrics scraped from the Prometheus endpoint.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    /// Raw metric name -> value mapping for simple (non-histogram) metrics.
    pub values: HashMap<String, f64>,
    /// Histogram bucket cumulative counts.
    ///
    /// Key: metric base name (e.g. `"dugite_peer_handshake_rtt_ms"`).
    /// Value: map of le-label string (e.g. `"50"`, `"100"`, `"+Inf"`) -> cumulative count.
    pub histogram_buckets: HashMap<String, HashMap<String, f64>>,
    /// String-valued labels from Prometheus info metrics.
    ///
    /// Populated for metrics that carry a single text label (e.g. pool_id).
    /// Key: synthetic name `<metric_name>.<label_key>` (e.g. `"dugite_pool_id_info.pool_id"`).
    /// Value: the label value string (e.g. `"6954ec…7856"`).
    pub string_labels: HashMap<String, String>,
    /// Whether the last scrape succeeded.
    pub connected: bool,
    /// Error message from the last failed scrape, if any.
    ///
    /// Shown in the Node panel when the node is offline so the operator can
    /// see why the connection failed (e.g. "Connection refused").
    pub error: Option<String>,
}

impl MetricsSnapshot {
    /// Retrieve a metric value by name, returning 0.0 if not found.
    pub fn get(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    /// Retrieve a metric value as u64.
    pub fn get_u64(&self, name: &str) -> u64 {
        self.get(name) as u64
    }
}

/// Parse Prometheus text exposition format into a `MetricsSnapshot`.
///
/// Handles:
/// - Simple gauge/counter lines: `metric_name value`
/// - Histogram bucket lines with `{le="…"}` label: stored in `histogram_buckets`
/// - `_sum` and `_count` suffix lines stored as plain values
/// - Comment lines (starting with `#`) skipped
fn parse_prometheus(text: &str) -> MetricsSnapshot {
    let mut values: HashMap<String, f64> = HashMap::new();
    let mut histogram_buckets: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut string_labels: HashMap<String, String> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        // Skip comments and empty lines.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Labeled metric line: `metric_name{label="value"} numeric_value [timestamp]`
        if let Some(bracket_pos) = line.find('{') {
            // Extract metric base name.
            let base_name = &line[..bracket_pos];

            if let Some(hist_name) = base_name.strip_suffix("_bucket") {
                // Histogram bucket: base name has `_bucket` suffix stripped, store
                // cumulative count under the histogram base name.
                if let Some(le_val) = extract_le_label(line) {
                    if let Some(value) = parse_value_after_labels(line) {
                        histogram_buckets
                            .entry(hist_name.to_string())
                            .or_default()
                            .insert(le_val, value);
                    }
                }
            } else {
                // Generic labeled metric — extract all key="value" label pairs and
                // store them as `<metric_name>.<label_key>` string entries so the
                // TUI can access string-valued metadata (e.g. pool_id from
                // `dugite_pool_id_info{pool_id="<hex>"}`).
                if let Some(bracket_end) = line.find('}') {
                    let labels_str = &line[bracket_pos + 1..bracket_end];
                    for kv in labels_str.split(',') {
                        let kv = kv.trim();
                        if let Some(eq_pos) = kv.find('=') {
                            let label_key = kv[..eq_pos].trim();
                            let label_val_raw = kv[eq_pos + 1..].trim();
                            // Strip surrounding quotes from the value.
                            let label_val = label_val_raw
                                .strip_prefix('"')
                                .and_then(|s| s.strip_suffix('"'))
                                .unwrap_or(label_val_raw);
                            let synthetic_key = format!("{}.{}", base_name, label_key);
                            string_labels.insert(synthetic_key, label_val.to_string());
                        }
                    }
                }
            }
            continue;
        }

        // Plain `metric_name value` line.
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(value_str)) = (parts.next(), parts.next()) {
            if let Ok(value) = value_str.parse::<f64>() {
                values.insert(name.to_string(), value);
            }
        }
    }

    MetricsSnapshot {
        values,
        histogram_buckets,
        string_labels,
        connected: true,
        error: None,
    }
}

/// Extract the `le` label value from a histogram bucket line.
///
/// Input:  `dugite_peer_handshake_rtt_ms_bucket{le="50"} 3`
/// Output: `Some("50")`
fn extract_le_label(line: &str) -> Option<String> {
    // Find `le="` inside the braces.
    let le_start = line.find("le=\"")?;
    let val_start = le_start + 4;
    let val_end = line[val_start..].find('"')?;
    Some(line[val_start..val_start + val_end].to_string())
}

/// Extract the metric value that appears after the closing `}` brace.
///
/// Input:  `dugite_peer_handshake_rtt_ms_bucket{le="50"} 3`
/// Output: `Some(3.0)`
fn parse_value_after_labels(line: &str) -> Option<f64> {
    let close_brace = line.find('}')?;
    let rest = line[close_brace + 1..].trim();
    // The value is the first whitespace-delimited token.
    rest.split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
}

/// Fetch metrics from the Prometheus endpoint and return a parsed snapshot.
pub async fn fetch_metrics(url: &str) -> MetricsSnapshot {
    match reqwest::get(url).await {
        Ok(resp) => match resp.text().await {
            Ok(body) => parse_prometheus(&body),
            Err(e) => MetricsSnapshot {
                values: HashMap::new(),
                histogram_buckets: HashMap::new(),
                string_labels: HashMap::new(),
                connected: false,
                error: Some(format!("Failed to read response: {e}")),
            },
        },
        Err(e) => MetricsSnapshot {
            values: HashMap::new(),
            histogram_buckets: HashMap::new(),
            string_labels: HashMap::new(),
            connected: false,
            error: Some(format!("Connection failed: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prometheus_basic() {
        let input = r#"
# HELP dugite_slot_number Current slot number
# TYPE dugite_slot_number gauge
dugite_slot_number 106919624
# HELP dugite_block_number Current block number
# TYPE dugite_block_number gauge
dugite_block_number 4109330
dugite_epoch_number 1237
dugite_sync_progress_percent 9982
dugite_peers_connected 5
dugite_mempool_tx_count 3
dugite_tip_age_seconds 12
dugite_peers_hot 5
dugite_peers_warm 3
dugite_peers_cold 8
"#;
        let snap = parse_prometheus(input);
        assert!(snap.connected);
        assert_eq!(snap.values["dugite_slot_number"], 106919624.0);
        assert_eq!(snap.values["dugite_block_number"], 4109330.0);
        assert_eq!(snap.values["dugite_epoch_number"], 1237.0);
        assert_eq!(snap.values["dugite_sync_progress_percent"], 9982.0);
        assert_eq!(snap.values["dugite_peers_connected"], 5.0);
    }

    #[test]
    fn test_parse_histogram_buckets() {
        let input = r#"
# TYPE dugite_peer_handshake_rtt_ms histogram
dugite_peer_handshake_rtt_ms_bucket{le="50"} 3
dugite_peer_handshake_rtt_ms_bucket{le="100"} 7
dugite_peer_handshake_rtt_ms_bucket{le="200"} 9
dugite_peer_handshake_rtt_ms_bucket{le="+Inf"} 10
dugite_peer_handshake_rtt_ms_sum 555
dugite_peer_handshake_rtt_ms_count 10
dugite_slot_number 42
"#;
        let snap = parse_prometheus(input);
        assert_eq!(snap.values["dugite_slot_number"], 42.0);
        assert_eq!(snap.values["dugite_peer_handshake_rtt_ms_sum"], 555.0);
        assert_eq!(snap.values["dugite_peer_handshake_rtt_ms_count"], 10.0);

        let buckets = snap
            .histogram_buckets
            .get("dugite_peer_handshake_rtt_ms")
            .expect("histogram should be parsed");
        assert_eq!(buckets["50"], 3.0);
        assert_eq!(buckets["100"], 7.0);
        assert_eq!(buckets["200"], 9.0);
        assert_eq!(buckets["+Inf"], 10.0);
    }

    #[test]
    fn test_parse_extract_le_label() {
        assert_eq!(
            extract_le_label(r#"some_bucket{le="50"} 3"#),
            Some("50".to_string())
        );
        assert_eq!(
            extract_le_label(r#"some_bucket{le="+Inf"} 10"#),
            Some("+Inf".to_string())
        );
        assert_eq!(extract_le_label("no_labels 5"), None);
    }

    #[test]
    fn test_parse_value_after_labels() {
        assert_eq!(
            parse_value_after_labels(r#"some_bucket{le="50"} 3.5"#),
            Some(3.5)
        );
        assert_eq!(
            parse_value_after_labels(r#"some_bucket{le="+Inf"} 10"#),
            Some(10.0)
        );
    }

    #[test]
    fn test_metrics_snapshot_get() {
        let mut snap = MetricsSnapshot::default();
        snap.values.insert("dugite_block_number".to_string(), 100.0);
        assert_eq!(snap.get("dugite_block_number"), 100.0);
        assert_eq!(snap.get("nonexistent"), 0.0);
        assert_eq!(snap.get_u64("dugite_block_number"), 100);
    }

    /// Verify that `dugite_pool_id_info{pool_id="<hex>"}` is parsed into
    /// the `string_labels` map under the key `dugite_pool_id_info.pool_id`.
    #[test]
    fn test_parse_pool_id_info_label() {
        let input = r#"
# HELP dugite_is_block_producer 1 when running as a block producer
# TYPE dugite_is_block_producer gauge
dugite_is_block_producer 1
# HELP dugite_pool_id_info Block producer pool identity
# TYPE dugite_pool_id_info gauge
dugite_pool_id_info{pool_id="6954ec7856abcdef1234567890abcdef12345678901234567856"} 1
dugite_slot_number 42
"#;
        let snap = parse_prometheus(input);
        assert_eq!(snap.values["dugite_is_block_producer"], 1.0);
        assert_eq!(snap.values["dugite_slot_number"], 42.0);
        assert_eq!(
            snap.string_labels.get("dugite_pool_id_info.pool_id"),
            Some(&"6954ec7856abcdef1234567890abcdef12345678901234567856".to_string())
        );
    }

    /// Verify that info metrics with multiple labels are all stored.
    #[test]
    fn test_parse_multi_label_info_metric() {
        let input = r#"dugite_pool_id_info{pool_id="aabbcc",version="1"} 1"#;
        let snap = parse_prometheus(input);
        assert_eq!(
            snap.string_labels.get("dugite_pool_id_info.pool_id"),
            Some(&"aabbcc".to_string())
        );
        assert_eq!(
            snap.string_labels.get("dugite_pool_id_info.version"),
            Some(&"1".to_string())
        );
    }
}
