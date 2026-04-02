/// Era-specific ledger transition logic
///
/// Each Cardano era introduces new ledger rules while maintaining
/// backward compatibility with previous eras.
pub mod byron;
pub mod conway;
pub mod shelley;
