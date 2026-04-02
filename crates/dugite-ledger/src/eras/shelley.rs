/// Shelley era ledger rules
///
/// Shelley introduces:
/// - Ouroboros Praos consensus
/// - Staking and delegation
/// - Reward distribution
/// - Multi-signature scripts
#[derive(Default)]
pub struct ShelleyLedger;

impl ShelleyLedger {
    pub fn new() -> Self {
        ShelleyLedger
    }
}
