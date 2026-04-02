/// Conway era ledger rules
///
/// Conway introduces on-chain governance (CIP-1694):
/// - DRep (Delegated Representatives)
/// - Constitutional Committee
/// - Governance actions and voting
/// - Treasury withdrawals
/// - Protocol parameter updates via governance
/// - Plutus V3
///
/// The governance state is tracked in `LedgerState.governance` (GovernanceState).
/// Certificates are processed in `LedgerState::process_certificate()`.
/// Proposals and votes are processed in `LedgerState::apply_block()`.
///
/// Governance action lifecycle:
/// 1. Proposal submitted in a transaction → stored in `governance.proposals`
/// 2. Votes cast by DReps, CC members, and SPOs → tallied per proposal
/// 3. At epoch boundary, expired proposals are removed
/// 4. Ratified proposals are enacted at epoch boundary
#[derive(Default)]
pub struct ConwayLedger;

impl ConwayLedger {
    pub fn new() -> Self {
        ConwayLedger
    }
}
