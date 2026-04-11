//! One-shot offline capture tool.
//!
//! Queries the public preview Koios endpoint for the data `ratify_proposals()`
//! needs and writes a JSON fixture under `fixtures/conway-ratification/`.
//! Not a test dependency — never runs in CI.

use clap::Parser;
use std::path::PathBuf;

/// Capture a Conway ratification fixture from Koios preview.
///
/// One-shot offline dev tool that writes a JSON fixture under
/// `fixtures/conway-ratification/` for use by
/// `crates/dugite-ledger/tests/conway_ratification.rs`.  Not a CI
/// dependency.
#[derive(Parser, Debug)]
#[command(name = "capture-ratification-fixture", version, about)]
struct Args {
    /// Network (only "preview" is supported for this first slice).
    #[arg(long, default_value = "preview")]
    network: String,

    /// Governance action id in the form `<tx_hex>#<cert_index>`.
    #[arg(long)]
    proposal_id: String,

    /// Output path (parent directory must exist).
    #[arg(long)]
    output: PathBuf,
}

// Exit code convention (Task 4 should preserve or deliberately replace this):
//   1 = todo / not yet implemented
//   2 = bad arguments
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    if args.network != "preview" {
        // exit 2 = bad args
        eprintln!("only --network=preview is supported (exit 2 = bad args)");
        std::process::exit(2);
    }
    // exit 1 = not yet implemented (Task 4 replaces this body)
    eprintln!(
        "capture-ratification-fixture: TODO — fetch {} and write {}",
        args.proposal_id,
        args.output.display()
    );
    std::process::exit(1);
}
