use crate::helpers::koios;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Poll Koios until a transaction is confirmed on-chain or timeout expires.
/// Returns the tx_info response on success.
pub async fn wait_for_confirmation(tx_hash: &str, timeout_secs: u64) -> Result<serde_json::Value> {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Transaction {} not confirmed within {}s",
                tx_hash,
                timeout_secs
            );
        }

        if let Ok(info) = koios::tx_info(tx_hash).await {
            if let Some(arr) = info.as_array() {
                if !arr.is_empty() {
                    return Ok(info);
                }
            }
        }

        sleep(Duration::from_secs(5)).await;
    }
}
