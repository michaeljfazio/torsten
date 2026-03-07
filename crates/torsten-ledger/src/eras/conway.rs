/// Conway era ledger rules
///
/// Conway introduces on-chain governance (CIP-1694):
/// - DRep (Delegated Representatives)
/// - Constitutional Committee
/// - Governance actions and voting
/// - Treasury withdrawals
/// - Protocol parameter updates via governance
/// - Plutus V3
#[derive(Default)]
pub struct ConwayLedger;

impl ConwayLedger {
    pub fn new() -> Self {
        ConwayLedger
    }
}
