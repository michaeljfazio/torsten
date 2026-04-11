//! One-shot offline capture tool.
//!
//! Queries the public preview Koios endpoint for the data `ratify_proposals()`
//! needs and writes a JSON fixture under `fixtures/conway-ratification/`.
//! Not a test dependency — never runs in CI.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "capture-ratification-fixture")]
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

fn main() {
    let args = Args::parse();
    if args.network != "preview" {
        eprintln!("only --network=preview is supported");
        std::process::exit(2);
    }
    eprintln!(
        "capture-ratification-fixture: TODO — fetch {} and write {}",
        args.proposal_id,
        args.output.display()
    );
    std::process::exit(1);
}
