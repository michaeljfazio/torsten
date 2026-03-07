use crate::hash::{Hash28, PolicyId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Lovelace (1 ADA = 1,000,000 Lovelace)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lovelace(pub u64);

impl Lovelace {
    pub const ZERO: Self = Lovelace(0);

    pub fn to_ada(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }

    pub fn from_ada(ada: f64) -> Self {
        Lovelace((ada * 1_000_000.0) as u64)
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Lovelace)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Lovelace)
    }
}

impl std::ops::Add for Lovelace {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Lovelace(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Lovelace {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Lovelace(self.0 - rhs.0)
    }
}

impl std::ops::AddAssign for Lovelace {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl std::fmt::Display for Lovelace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} lovelace", self.0)
    }
}

/// Asset name (up to 32 bytes)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssetName(pub Vec<u8>);

impl AssetName {
    pub const MAX_LENGTH: usize = 32;

    pub fn new(bytes: Vec<u8>) -> Result<Self, &'static str> {
        if bytes.len() > Self::MAX_LENGTH {
            return Err("Asset name exceeds 32 bytes");
        }
        Ok(AssetName(bytes))
    }

    pub fn empty() -> Self {
        AssetName(Vec::new())
    }

    pub fn as_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Display for AssetName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.as_utf8() {
            Some(s) if s.chars().all(|c| c.is_ascii_graphic()) => write!(f, "{}", s),
            _ => write!(f, "0x{}", self.to_hex()),
        }
    }
}

/// Multi-asset value: ADA + native tokens
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub coin: Lovelace,
    pub multi_asset: MultiAsset,
}

/// Map from PolicyId -> AssetName -> quantity
pub type MultiAsset = BTreeMap<PolicyId, BTreeMap<AssetName, u64>>;

impl Value {
    pub fn lovelace(coin: u64) -> Self {
        Value {
            coin: Lovelace(coin),
            multi_asset: BTreeMap::new(),
        }
    }

    pub fn is_pure_ada(&self) -> bool {
        self.multi_asset.is_empty()
    }

    pub fn add(&self, other: &Value) -> Self {
        let coin = Lovelace(self.coin.0 + other.coin.0);
        let mut multi_asset = self.multi_asset.clone();
        for (policy, assets) in &other.multi_asset {
            let entry = multi_asset.entry(*policy).or_default();
            for (name, qty) in assets {
                *entry.entry(name.clone()).or_insert(0) += qty;
            }
        }
        Value { coin, multi_asset }
    }

    /// Check if this value is greater than or equal to another (for UTxO validation)
    pub fn geq(&self, other: &Value) -> bool {
        if self.coin.0 < other.coin.0 {
            return false;
        }
        for (policy, assets) in &other.multi_asset {
            match self.multi_asset.get(policy) {
                None => return false,
                Some(self_assets) => {
                    for (name, qty) in assets {
                        match self_assets.get(name) {
                            None => return false,
                            Some(self_qty) if self_qty < qty => return false,
                            _ => {}
                        }
                    }
                }
            }
        }
        true
    }

    pub fn policy_count(&self) -> usize {
        self.multi_asset.len()
    }

    pub fn asset_count(&self) -> usize {
        self.multi_asset.values().map(|a| a.len()).sum()
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::lovelace(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash28;

    #[test]
    fn test_lovelace_ada_conversion() {
        let l = Lovelace(2_500_000);
        assert!((l.to_ada() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_value_geq() {
        let v1 = Value::lovelace(5_000_000);
        let v2 = Value::lovelace(3_000_000);
        assert!(v1.geq(&v2));
        assert!(!v2.geq(&v1));
    }

    #[test]
    fn test_multi_asset_value() {
        let policy = Hash28::from_bytes([1u8; 28]);
        let asset_name = AssetName::new(b"TestToken".to_vec()).unwrap();

        let mut v = Value::lovelace(2_000_000);
        v.multi_asset
            .entry(policy)
            .or_default()
            .insert(asset_name, 100);

        assert!(!v.is_pure_ada());
        assert_eq!(v.policy_count(), 1);
        assert_eq!(v.asset_count(), 1);
    }
}
