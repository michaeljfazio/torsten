use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::Rational;
use torsten_primitives::value::Lovelace;

/// Shelley genesis configuration (compatible with cardano-node shelley-genesis.json)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ShelleyGenesis {
    pub network_magic: u64,
    pub network_id: String,
    pub system_start: String,
    pub active_slots_coeff: f64,
    pub security_param: u64,
    pub epoch_length: u64,
    pub slot_length: u64,
    pub max_lovelace_supply: u64,
    pub max_k_e_s_evolutions: u64,
    pub slots_per_k_e_s_period: u64,
    pub update_quorum: u64,
    pub protocol_params: ShelleyGenesisProtocolParams,
}

/// Protocol parameters as specified in Shelley genesis
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ShelleyGenesisProtocolParams {
    pub min_fee_a: u64,
    pub min_fee_b: u64,
    pub max_block_body_size: u64,
    pub max_tx_size: u64,
    pub max_block_header_size: u64,
    pub key_deposit: u64,
    pub pool_deposit: u64,
    pub e_max: u64,
    #[serde(alias = "nOpt")]
    pub n_opt: u64,
    pub a0: f64,
    pub rho: f64,
    pub tau: f64,
    pub min_pool_cost: u64,
    #[serde(default)]
    pub min_u_tx_o_value: u64,
    pub protocol_version: ProtocolVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolVersion {
    pub major: u64,
    pub minor: u64,
}

impl ShelleyGenesis {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Shelley genesis: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Shelley genesis: {}", path.display()))
    }

    /// Apply genesis parameters to protocol parameters, keeping Conway-era
    /// defaults for fields not present in Shelley genesis.
    pub fn apply_to_protocol_params(&self, params: &mut ProtocolParameters) {
        let gp = &self.protocol_params;
        params.min_fee_a = gp.min_fee_a;
        params.min_fee_b = gp.min_fee_b;
        params.max_block_body_size = gp.max_block_body_size;
        params.max_tx_size = gp.max_tx_size;
        params.max_block_header_size = gp.max_block_header_size;
        params.key_deposit = Lovelace(gp.key_deposit);
        params.pool_deposit = Lovelace(gp.pool_deposit);
        params.e_max = gp.e_max;
        params.n_opt = gp.n_opt;
        params.a0 = float_to_rational(gp.a0);
        params.rho = float_to_rational(gp.rho);
        params.tau = float_to_rational(gp.tau);
        params.min_pool_cost = Lovelace(gp.min_pool_cost);
        params.protocol_version_major = gp.protocol_version.major;
        params.protocol_version_minor = gp.protocol_version.minor;
        params.active_slots_coeff = self.active_slots_coeff;
    }
}

/// Convert a float to a rational approximation
fn float_to_rational(f: f64) -> Rational {
    if f == 0.0 {
        return Rational {
            numerator: 0,
            denominator: 1,
        };
    }
    // Use 1_000_000 as denominator for good precision
    let den = 1_000_000u64;
    let num = (f * den as f64).round() as u64;
    // Simplify with GCD
    let g = gcd(num, den);
    Rational {
        numerator: num / g,
        denominator: den / g,
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float_to_rational() {
        let r = float_to_rational(0.3);
        assert_eq!(r.numerator, 3);
        assert_eq!(r.denominator, 10);

        let r = float_to_rational(0.05);
        assert_eq!(r.numerator, 1);
        assert_eq!(r.denominator, 20);

        let r = float_to_rational(0.003);
        assert_eq!(r.numerator, 3);
        assert_eq!(r.denominator, 1000);
    }

    #[test]
    fn test_parse_shelley_genesis() {
        let json = r#"{
            "networkMagic": 2,
            "networkId": "Testnet",
            "systemStart": "2022-10-25T00:00:00Z",
            "activeSlotsCoeff": 0.05,
            "securityParam": 432,
            "epochLength": 86400,
            "slotLength": 1,
            "maxLovelaceSupply": 45000000000000000,
            "maxKESEvolutions": 62,
            "slotsPerKESPeriod": 129600,
            "updateQuorum": 5,
            "protocolParams": {
                "minFeeA": 44,
                "minFeeB": 155381,
                "maxBlockBodySize": 65536,
                "maxTxSize": 16384,
                "maxBlockHeaderSize": 1100,
                "keyDeposit": 2000000,
                "poolDeposit": 500000000,
                "eMax": 18,
                "nOpt": 150,
                "a0": 0.3,
                "rho": 0.003,
                "tau": 0.2,
                "minPoolCost": 340000000,
                "minUTxOValue": 1000000,
                "protocolVersion": { "major": 6, "minor": 0 }
            }
        }"#;

        let genesis: ShelleyGenesis = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.network_magic, 2);
        assert_eq!(genesis.system_start, "2022-10-25T00:00:00Z");
        assert_eq!(genesis.active_slots_coeff, 0.05);
        assert_eq!(genesis.epoch_length, 86400);
        assert_eq!(genesis.protocol_params.n_opt, 150);
        assert_eq!(genesis.protocol_params.min_pool_cost, 340000000);

        // Apply to protocol params
        let mut pp = ProtocolParameters::mainnet_defaults();
        genesis.apply_to_protocol_params(&mut pp);
        assert_eq!(pp.n_opt, 150);
        assert_eq!(pp.min_pool_cost, Lovelace(340000000));
        assert_eq!(pp.max_block_body_size, 65536);
    }
}
