/// Byron era ledger rules
///
/// The Byron era uses OBFT (Optimistic Byzantine Fault Tolerance) consensus
/// and has a simpler transaction model (no staking, no scripts).

pub struct ByronLedger;

impl ByronLedger {
    pub fn new() -> Self {
        ByronLedger
    }
}
