use anyhow::Result;
use serde_json::Value;

const KOIOS_PREVIEW_BASE: &str = "https://preview.koios.rest/api/v1";

/// GET request to Koios preview API.
pub async fn koios_get(endpoint: &str) -> Result<Value> {
    let url = format!("{}/{}", KOIOS_PREVIEW_BASE, endpoint);
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?;
    let body = resp.json::<Value>().await?;
    Ok(body)
}

/// POST request to Koios preview API.
pub async fn koios_post(endpoint: &str, body: &Value) -> Result<Value> {
    let url = format!("{}/{}", KOIOS_PREVIEW_BASE, endpoint);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await?;
    let result = resp.json::<Value>().await?;
    Ok(result)
}

/// Get the current tip from Koios.
pub async fn tip() -> Result<Value> {
    let result = koios_get("tip").await?;
    Ok(result)
}

/// Get UTxOs for an address from Koios.
pub async fn address_utxos(addr: &str) -> Result<Value> {
    let body = serde_json::json!({
        "_addresses": [addr]
    });
    koios_post("address_utxos", &body).await
}

/// Get transaction info from Koios.
pub async fn tx_info(tx_hash: &str) -> Result<Value> {
    let body = serde_json::json!({
        "_tx_hashes": [tx_hash]
    });
    koios_post("tx_info", &body).await
}

/// Get transaction status from Koios.
pub async fn tx_status(tx_hash: &str) -> Result<Value> {
    let body = serde_json::json!({
        "_tx_hashes": [tx_hash]
    });
    koios_post("tx_status", &body).await
}

/// Get the total balance for an address (sum of all UTxO values) from Koios.
pub async fn address_balance(addr: &str) -> Result<u64> {
    let utxos = address_utxos(addr).await?;
    let mut total: u64 = 0;
    if let Some(arr) = utxos.as_array() {
        for utxo in arr {
            if let Some(val) = utxo.get("value").and_then(|v| v.as_str()) {
                total += val.parse::<u64>().unwrap_or(0);
            }
        }
    }
    Ok(total)
}
