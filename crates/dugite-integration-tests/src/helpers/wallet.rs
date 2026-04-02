use crate::helpers::cli::run_cli_ok;

/// A parsed UTxO entry from the CLI output.
#[derive(Debug, Clone)]
pub struct Utxo {
    pub tx_hash: String,
    pub tx_index: u32,
    pub lovelace: u64,
}

/// Query UTxOs for an address via the CLI.
pub fn get_utxos_cli(socket: &str, address: &str) -> Vec<Utxo> {
    let output = run_cli_ok(&[
        "query",
        "utxo",
        "--address",
        address,
        "--socket-path",
        socket,
    ]);
    parse_utxo_output(&output)
}

/// Parse the tabular UTxO output from `query utxo`.
fn parse_utxo_output(output: &str) -> Vec<Utxo> {
    let mut utxos = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        // Skip header lines and separators
        if line.is_empty()
            || line.starts_with("TxHash")
            || line.starts_with('-')
            || line.starts_with("=")
        {
            continue;
        }
        // Expected format: "txhash     index    lovelace + ..."
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            if let (Ok(index), Ok(lovelace)) = (parts[1].parse::<u32>(), parts[2].parse::<u64>()) {
                utxos.push(Utxo {
                    tx_hash: parts[0].to_string(),
                    tx_index: index,
                    lovelace,
                });
            }
        }
    }
    utxos
}

/// Get total balance from CLI UTxO query.
pub fn get_balance_cli(socket: &str, address: &str) -> u64 {
    get_utxos_cli(socket, address)
        .iter()
        .map(|u| u.lovelace)
        .sum()
}

/// Skip the test if the wallet doesn't have enough lovelace.
pub fn skip_if_underfunded(socket: &str, address: &str, min_lovelace: u64) -> bool {
    let balance = get_balance_cli(socket, address);
    if balance < min_lovelace {
        eprintln!(
            "SKIP: wallet {} has {} lovelace, need {}",
            address, balance, min_lovelace
        );
        return true;
    }
    false
}
