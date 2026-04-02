use super::EraRules;

pub struct ShelleyRules;

impl EraRules for ShelleyRules {
    fn era_name(&self) -> &'static str {
        "Shelley"
    }
}
