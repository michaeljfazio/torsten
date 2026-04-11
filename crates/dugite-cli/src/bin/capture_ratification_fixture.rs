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

const KOIOS_BASE: &str = "https://preview.koios.rest/api/v1";

async fn koios_get(client: &reqwest::Client, path: &str) -> serde_json::Value {
    let url = format!("{KOIOS_BASE}{path}");
    eprintln!("GET {url}");
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => panic!("koios GET {url} failed: {e}"),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        panic!("koios {url} returned {status}: {body}");
    }
    match resp.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(e) => panic!("koios {url} body was not JSON: {e}"),
    }
}

// Exit code convention:
//   2 = bad arguments
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    if args.network != "preview" {
        // exit 2 = bad args
        eprintln!("only --network=preview is supported (exit 2 = bad args)");
        std::process::exit(2);
    }

    let client = reqwest::Client::builder()
        .user_agent("dugite-capture-ratification-fixture/0.1")
        .build()
        .expect("reqwest client");

    let (tx_hex, idx_str) = match args.proposal_id.split_once('#') {
        Some(parts) => parts,
        None => panic!("malformed --proposal-id: {}", args.proposal_id),
    };
    let idx: u32 = idx_str.parse().expect("--proposal-id index not u32");

    // 1. proposal_list — find this specific proposal and its metadata
    let proposal_list = koios_get(
        &client,
        &format!("/proposal_list?proposal_tx_hash=eq.{tx_hex}&cert_index=eq.{idx}"),
    )
    .await;
    let proposal = match proposal_list.as_array().and_then(|a| a.first()).cloned() {
        Some(p) => p,
        None => panic!("proposal {} not found on Koios", args.proposal_id),
    };

    // 2. proposal_voting_summary — ratified/dropped + enacted_epoch
    let voting_summary = koios_get(
        &client,
        &format!("/proposal_voting_summary?proposal_id=eq.{tx_hex}"),
    )
    .await;

    // 3. proposal_votes — individual vote records
    let votes = koios_get(&client, &format!("/proposal_votes?proposal_id=eq.{tx_hex}")).await;

    // Extract the ratification epoch.  Koios exposes this as `enacted_epoch`
    // for ratified proposals, or `dropped_epoch` for expired/dropped ones.
    // Power snapshots are taken at (ratification_epoch - 1).
    let ratification_epoch: u64 = match proposal
        .get("ratified_epoch")
        .or_else(|| proposal.get("enacted_epoch"))
        .or_else(|| proposal.get("dropped_epoch"))
        .and_then(|v| v.as_u64())
    {
        Some(e) => e,
        None => panic!("no ratification/dropped epoch in proposal row"),
    };
    let snapshot_epoch = ratification_epoch.saturating_sub(1);

    // 4. drep_voting_power_history @ snapshot_epoch
    let drep_power = koios_get(
        &client,
        &format!("/drep_voting_power_history?epoch_no=eq.{snapshot_epoch}"),
    )
    .await;

    // 5. pool_voting_power_history @ snapshot_epoch
    let pool_power = koios_get(
        &client,
        &format!("/pool_voting_power_history?epoch_no=eq.{snapshot_epoch}"),
    )
    .await;

    // 6. committee_info — current committee at ratification time
    let committee = koios_get(&client, "/committee_info").await;

    // 7. epoch_params @ ratification_epoch
    let pparams = koios_get(
        &client,
        &format!("/epoch_params?_epoch_no={ratification_epoch}"),
    )
    .await;

    // Stub fields below are placeholders to be filled in during Task 5
    // (real fixture capture).  They are intentionally zero/null so that an
    // unedited capture round-trips through the loader but obviously fails
    // any meaningful ratification assertion — forcing the fixture author to
    // populate them by hand from Koios responses.  Greppable as TODO(task-5).
    let fixture = serde_json::json!({
        "proposal": proposal,
        "votes": votes,
        "drep_power": drep_power,
        // TODO(task-5): aggregate from drep_voting_power_history rows.
        "drep_no_confidence": 0u64,
        // TODO(task-5): aggregate from drep_voting_power_history rows.
        "drep_abstain": 0u64,
        "spo_stake": pool_power,
        "committee": committee,
        "pparams_epoch": ratification_epoch,
        "pparams": pparams,
        // TODO(task-5): sum drep_voting_power_history rows at snapshot epoch.
        "total_drep_stake": 0u64,
        // TODO(task-5): sum pool_voting_power_history rows at snapshot epoch.
        "total_spo_stake": 0u64,
        "voting_summary": voting_summary,
        "expected_outcome": {
            "ratified": proposal.get("ratified_epoch").is_some()
                || proposal.get("enacted_epoch").is_some(),
            "enacted_bucket": proposal
                .get("proposal_type")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "enacted_epoch": ratification_epoch,
            "enacted_id": format!("{tx_hex}#{idx}"),
        },
        // TODO(task-5): seed each bucket from a recursive capture of
        // proposal.prev_action_id when present.
        "parent_enacted": {
            "PParamUpdate": null,
            "HardFork": null,
            "Committee": null,
            "Constitution": null,
        }
    });

    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent).expect("create output parent dir");
    }
    let pretty = serde_json::to_string_pretty(&fixture).expect("serialize");
    std::fs::write(&args.output, pretty + "\n").expect("write output");
    eprintln!("wrote {}", args.output.display());
}
