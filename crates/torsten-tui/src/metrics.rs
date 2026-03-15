//! Prometheus metrics HTTP polling and parsing.
//!
//! Fetches the Torsten node's Prometheus endpoint and parses the text exposition
//! format into a structured `MetricsSnapshot`. Only simple gauge/counter lines are
//! parsed — histograms, labels, and comments are skipped.

use std::collections::HashMap;

/// A point-in-time snapshot of all metrics scraped from the Prometheus endpoint.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    /// Raw metric name -> value mapping for all simple (non-histogram) metrics.
    pub values: HashMap<String, f64>,
    /// Labeled metric values. Key format: "metric_name:label_value" -> count.
    ///
    /// Parses Prometheus lines like `metric_name{key="value"} 123` into
    /// entries keyed as `"metric_name:value"`.
    #[allow(dead_code)]
    pub labeled: HashMap<String, f64>,
    /// Whether the last scrape succeeded.
    pub connected: bool,
    /// Error message from the last failed scrape, if any.
    #[allow(dead_code)]
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

/// Result of parsing Prometheus exposition text.
struct ParsedMetrics {
    /// Simple (unlabeled) metric values.
    values: HashMap<String, f64>,
    /// Labeled metric values with key format "metric_name:label_value".
    labeled: HashMap<String, f64>,
}

/// Parse Prometheus text exposition format into both simple and labeled metric maps.
///
/// Simple lines (`metric_name value`) go into `values`.
/// Labeled lines (`metric_name{key="value"} 123`) go into `labeled`
/// with key format `"metric_name:value"` (using the first label's value).
/// Comment lines (starting with #) are skipped.
fn parse_prometheus(text: &str) -> ParsedMetrics {
    let mut values = HashMap::new();
    let mut labeled = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        // Skip comments and empty lines.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(brace_start) = line.find('{') {
            // Labeled metric: extract name, first label value, and metric value.
            let metric_name = &line[..brace_start];
            if let Some(brace_end) = line.find('}') {
                let labels_str = &line[brace_start + 1..brace_end];
                let value_str = line[brace_end + 1..].trim();

                // Parse the numeric value after the closing brace.
                if let Ok(value) = value_str.parse::<f64>() {
                    // Extract first label value: key="value" or key='value'.
                    if let Some(eq_pos) = labels_str.find('=') {
                        let label_val = labels_str[eq_pos + 1..]
                            .trim_matches('"')
                            .split('"')
                            .next()
                            .unwrap_or("");
                        // Also handle comma-separated labels by taking only up to first comma.
                        let label_val = label_val.split(',').next().unwrap_or(label_val);
                        let key = format!("{}:{}", metric_name, label_val);
                        labeled.insert(key, value);
                    }
                }
            }
        } else {
            // Simple unlabeled metric.
            let mut parts = line.split_whitespace();
            if let (Some(name), Some(value_str)) = (parts.next(), parts.next()) {
                if let Ok(value) = value_str.parse::<f64>() {
                    values.insert(name.to_string(), value);
                }
            }
        }
    }

    ParsedMetrics { values, labeled }
}

/// Fetch metrics from the Prometheus endpoint and return a parsed snapshot.
pub async fn fetch_metrics(url: &str) -> MetricsSnapshot {
    match reqwest::get(url).await {
        Ok(resp) => match resp.text().await {
            Ok(body) => {
                let parsed = parse_prometheus(&body);
                MetricsSnapshot {
                    values: parsed.values,
                    labeled: parsed.labeled,
                    connected: true,
                    error: None,
                }
            }
            Err(e) => MetricsSnapshot {
                values: HashMap::new(),
                labeled: HashMap::new(),
                connected: false,
                error: Some(format!("Failed to read response: {e}")),
            },
        },
        Err(e) => MetricsSnapshot {
            values: HashMap::new(),
            labeled: HashMap::new(),
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
# HELP torsten_slot_number Current slot number
# TYPE torsten_slot_number gauge
torsten_slot_number 106919624
# HELP torsten_block_number Current block number
# TYPE torsten_block_number gauge
torsten_block_number 4109330
# HELP torsten_epoch_number Current epoch number
# TYPE torsten_epoch_number gauge
torsten_epoch_number 1237
torsten_sync_progress_percent 9982
torsten_peers_connected 5
torsten_utxo_count 2939027
torsten_mempool_tx_count 3
torsten_mempool_bytes 1200
torsten_treasury_lovelace 14070000000000
torsten_drep_count 8791
torsten_proposal_count 2
torsten_pool_count 656
torsten_mem_resident_bytes 6076211200
torsten_uptime_seconds 3600
torsten_tip_age_seconds 12
torsten_peers_hot 5
torsten_peers_warm 3
torsten_peers_cold 8
"#;
        let parsed = parse_prometheus(input);
        assert_eq!(parsed.values["torsten_slot_number"], 106919624.0);
        assert_eq!(parsed.values["torsten_block_number"], 4109330.0);
        assert_eq!(parsed.values["torsten_epoch_number"], 1237.0);
        assert_eq!(parsed.values["torsten_sync_progress_percent"], 9982.0);
        assert_eq!(parsed.values["torsten_peers_connected"], 5.0);
    }

    #[test]
    fn test_parse_prometheus_labeled_metrics() {
        let input = r#"
# TYPE torsten_peer_handshake_rtt_ms histogram
torsten_peer_handshake_rtt_ms_bucket{le="1"} 0
torsten_peer_handshake_rtt_ms_bucket{le="5"} 2
torsten_peer_handshake_rtt_ms_bucket{le="+Inf"} 10
torsten_peer_handshake_rtt_ms_sum 555
torsten_peer_handshake_rtt_ms_count 10
torsten_slot_number 42
"#;
        let parsed = parse_prometheus(input);
        // Labeled lines go into `labeled` map.
        assert_eq!(
            parsed.labeled["torsten_peer_handshake_rtt_ms_bucket:1"],
            0.0
        );
        assert_eq!(
            parsed.labeled["torsten_peer_handshake_rtt_ms_bucket:5"],
            2.0
        );
        assert_eq!(
            parsed.labeled["torsten_peer_handshake_rtt_ms_bucket:+Inf"],
            10.0
        );
        // Sum and count (no labels) are in `values`.
        assert_eq!(parsed.values["torsten_peer_handshake_rtt_ms_sum"], 555.0);
        assert_eq!(parsed.values["torsten_peer_handshake_rtt_ms_count"], 10.0);
        assert_eq!(parsed.values["torsten_slot_number"], 42.0);
    }

    #[test]
    fn test_parse_prometheus_multi_label() {
        let input = r#"
http_requests_total{method="GET",endpoint="/api"} 42
"#;
        let parsed = parse_prometheus(input);
        // Uses first label value.
        assert_eq!(parsed.labeled["http_requests_total:GET"], 42.0);
    }

    #[test]
    fn test_metrics_snapshot_get() {
        let mut snap = MetricsSnapshot::default();
        snap.values
            .insert("torsten_block_number".to_string(), 100.0);
        assert_eq!(snap.get("torsten_block_number"), 100.0);
        assert_eq!(snap.get("nonexistent"), 0.0);
        assert_eq!(snap.get_u64("torsten_block_number"), 100);
    }
}
