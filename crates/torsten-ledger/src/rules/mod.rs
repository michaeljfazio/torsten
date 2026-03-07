pub mod shelley;

/// Marker trait for era-specific ledger rules
pub trait EraRules {
    fn era_name(&self) -> &'static str;
}
