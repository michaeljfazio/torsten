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

    // 1. proposal_list — find this specific proposal and its metadata.
    // Koios uses `proposal_index` (not `cert_index`).
    let proposal_list = koios_get(
        &client,
        &format!("/proposal_list?proposal_tx_hash=eq.{tx_hex}&proposal_index=eq.{idx}"),
    )
    .await;
    let proposal = match proposal_list.as_array().and_then(|a| a.first()).cloned() {
        Some(p) => p,
        None => panic!("proposal {} not found on Koios", args.proposal_id),
    };

    // The voting_summary and proposal_votes RPCs want the bech32 `gov_action1...`
    // form, not the raw `tx#index`.  Pull it from the proposal_list row.
    let proposal_id_bech32 = match proposal.get("proposal_id").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => panic!("proposal_list row missing proposal_id (bech32)"),
    };

    // 2. proposal_voting_summary — ratified/dropped + enacted_epoch (RPC).
    let voting_summary = koios_get(
        &client,
        &format!("/proposal_voting_summary?_proposal_id={proposal_id_bech32}"),
    )
    .await;

    // 3. proposal_votes — individual vote records (RPC).
    let votes = koios_get(
        &client,
        &format!("/proposal_votes?_proposal_id={proposal_id_bech32}"),
    )
    .await;

    // Extract the ratification epoch.  Koios exposes this as `enacted_epoch`
    // for ratified proposals, or `dropped_epoch` for expired/dropped ones.
    // Power snapshots are taken at (ratification_epoch - 1).
    // Koios returns these fields as `null` when inapplicable, so `.or_else`
    // on `Option<&Value>` won't fall through — we need to check for a numeric
    // value at each step and keep walking on `Value::Null`.
    let ratification_epoch: u64 = ["ratified_epoch", "enacted_epoch", "dropped_epoch"]
        .iter()
        .find_map(|k| proposal.get(k).and_then(|v| v.as_u64()))
        .unwrap_or_else(|| panic!("no ratification/dropped epoch in proposal row"));
    let was_ratified = proposal
        .get("ratified_epoch")
        .and_then(|v| v.as_u64())
        .is_some()
        || proposal
            .get("enacted_epoch")
            .and_then(|v| v.as_u64())
            .is_some();
    let snapshot_epoch = ratification_epoch.saturating_sub(1);

    // 4. drep_voting_power_history is a per-DRep RPC; capturing the full
    // snapshot would require enumerating drep_list and querying each one.
    // Deferred to Task 6 — see drep_power TODO in the fixture body below.

    // 5. pool_voting_power_history @ snapshot_epoch (RPC param: _epoch_no)
    let pool_power = koios_get(
        &client,
        &format!("/pool_voting_power_history?_epoch_no={snapshot_epoch}"),
    )
    .await;

    // 6. committee_info — current committee at ratification time
    let committee = koios_get(&client, "/committee_info").await;

    // 7. epoch_params @ ratification_epoch (RPC param: _epoch_no)
    let pparams = koios_get(
        &client,
        &format!("/epoch_params?_epoch_no={ratification_epoch}"),
    )
    .await;

    // Suppress unused warnings for diagnostic-only fields the canonical
    // fixture shape does not embed.
    let _ = (
        &voting_summary,
        &committee,
        &pparams,
        &pool_power,
        &snapshot_epoch,
    );

    // Transform the raw Koios proposal row into the canonical FixtureProposal
    // schema the loader expects.  Stake address bech32 → bytes is heavyweight
    // and `return_addr` is unused by ratify_proposals, so we substitute a
    // dummy 29-byte zero hex string.  The opaque `action` JSON is preserved
    // for Task 6 to reconstruct.
    let proposed_epoch = proposal
        .get("proposed_epoch")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("proposal row missing proposed_epoch"));
    let expiration = proposal
        .get("expiration")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("proposal row missing expiration"));
    let deposit: u64 = proposal
        .get("deposit")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("proposal row missing/non-numeric deposit"));
    let proposal_type_str = proposal
        .get("proposal_type")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("proposal row missing proposal_type"))
        .to_string();
    let enacted_bucket = match proposal_type_str.as_str() {
        // Map Koios proposal_type → canonical EnactedBucket variant.
        "ParameterChange" => "PParamUpdate",
        "HardForkInitiation" => "HardFork",
        "NewCommittee" => "Committee",
        "NewConstitution" => "Constitution",
        // Out-of-scope for first slice (loader rejects these).
        other => panic!(
            "proposal_type {other:?} is out of scope for the first slice (PParamUpdate / HardFork / Committee / Constitution only)"
        ),
    };

    let fixture_proposal = serde_json::json!({
        "gov_action_id": format!("{tx_hex}#{idx}"),
        // TODO(task-6): reconstruct GovAction from this opaque blob.
        "action": proposal.get("proposal_description").cloned().unwrap_or(serde_json::Value::Null),
        "deposit": deposit,
        // 29-byte zero stake credential (header byte 0xe0 + 28 zero bytes).
        // Loader doesn't read this for ratify_proposals — refunds aren't asserted.
        "return_addr_hex": "e0000000000000000000000000000000000000000000000000000000000000",
        "expiration": expiration,
        "anchor": null,
    });

    // Transform Koios votes → canonical FixtureVote list.
    let mut canonical_votes: Vec<serde_json::Value> = Vec::new();
    if let Some(arr) = votes.as_array() {
        for v in arr {
            let role = v.get("voter_role").and_then(|x| x.as_str()).unwrap_or("");
            let has_script = v
                .get("voter_has_script")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let voter_hex = v
                .get("voter_hex")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| panic!("vote row missing voter_hex: {v}"));
            let vote_str = v
                .get("vote")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| panic!("vote row missing vote: {v}"));
            let voter_type = match (role, has_script) {
                ("DRep", false) => "DRepKeyHash",
                ("DRep", true) => "DRepScriptHash",
                ("SPO", _) => "StakePoolKeyHash",
                ("ConstitutionalCommittee", false) => "ConstitutionalCommitteeHotKeyHash",
                ("ConstitutionalCommittee", true) => "ConstitutionalCommitteeHotScriptHash",
                _ => panic!("unknown voter_role/has_script: {role}/{has_script}"),
            };
            canonical_votes.push(serde_json::json!({
                "voter_type": voter_type,
                "voter_id": voter_hex,
                "vote": vote_str,
            }));
        }
    }

    let fixture = serde_json::json!({
        "proposal": fixture_proposal,
        "proposed_epoch": proposed_epoch,
        "votes": canonical_votes,
        // TODO(task-6): populate from drep_voting_power_history (per-DRep).
        "drep_power": serde_json::Map::<String, serde_json::Value>::new(),
        // TODO(task-6): aggregate from drep_voting_power_history.
        "drep_no_confidence": 0u64,
        // TODO(task-6): aggregate from drep_voting_power_history.
        "drep_abstain": 0u64,
        // TODO(task-6): bech32-decode pool_voting_power_history pool_id_bech32 → hex.
        "spo_stake": serde_json::Map::<String, serde_json::Value>::new(),
        // TODO(task-6): transform Koios committee_info into canonical
        // FixtureCommittee shape (cold/hot keys, expiration, threshold).
        "committee": {
            "members": [],
            "threshold": { "numerator": 2, "denominator": 3 },
            "min_size": 0,
            "resigned": [],
        },
        "pparams_epoch": ratification_epoch,
        // pparams JSON is opaque to the loader for now.
        "pparams": {},
        // TODO(task-6): sum drep_voting_power_history rows at snapshot epoch.
        "total_drep_stake": 0u64,
        // TODO(task-6): sum pool_voting_power_history rows at snapshot epoch.
        "total_spo_stake": 0u64,
        "expected_outcome": {
            // A proposal is ratified iff `ratified_epoch` or `enacted_epoch` is a
            // non-null number.  `Value::Null.is_some()` is true, so we have to
            // check `as_u64()`.  For dropped proposals, `enacted_id` is left
            // `null` — the test asserts the ledger's `enacted_*` slot does
            // *not* contain this proposal id.
            "ratified": was_ratified,
            "enacted_bucket": enacted_bucket,
            "enacted_epoch": ratification_epoch,
            "enacted_id": if was_ratified {
                serde_json::Value::String(format!("{tx_hex}#{idx}"))
            } else {
                serde_json::Value::Null
            },
        },
        // TODO(task-6): seed each bucket from a recursive capture of
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
