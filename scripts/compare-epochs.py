#!/usr/bin/env python3
"""Compare torsten epoch snapshots against cstreamer reference snapshots.

Usage:
    python3 scripts/compare-epochs.py <torsten_dir> <cstreamer_dir> [--start-epoch N] [--stop-epoch N] [--verbose]

Compares key ledger state fields at each epoch boundary and reports divergences.
"""

import json
import os
import sys
import argparse
from pathlib import Path
from collections import defaultdict


# Fields to compare and their significance
CORE_FIELDS = [
    "reserves",
    "treasury",
    "epochFees",
    "activeStake",
    "totalStake",
    "totalPools",
    "snapshotEraName",
    "epochNonce",
]

DEPOSIT_FIELDS = ["stakeKey", "pool", "dRep", "proposal", "total"]

PROTOCOL_PARAM_FIELDS = [
    "a0", "d", "minPoolCost", "nOpt", "rho", "tau", "protocolVersion",
]

RUPD_FIELDS = ["deltaR", "deltaT", "deltaF", "rewardPot"]


def load_snapshots(directory):
    """Load all epoch snapshot JSON files from a directory."""
    snapshots = {}
    for f in Path(directory).glob("*.json"):
        if f.name == "cstreamer.log":
            continue
        try:
            with open(f) as fh:
                data = json.load(fh)
            epoch = data.get("epoch")
            if epoch is not None:
                snapshots[int(epoch)] = data
        except (json.JSONDecodeError, KeyError) as e:
            print(f"  Warning: Could not parse {f.name}: {e}", file=sys.stderr)
    return snapshots


def compare_value(path, torsten_val, cstreamer_val):
    """Compare two values, returning a description of the difference or None if equal."""
    if torsten_val == cstreamer_val:
        return None

    # Skip fields that torsten doesn't emit yet — avoids false divergences when
    # the reference (cstreamer) carries extra fields not present in our snapshot.
    if torsten_val is None and cstreamer_val is not None:
        return None

    # Handle numeric comparison with tolerance info
    if isinstance(torsten_val, (int, float)) and isinstance(cstreamer_val, (int, float)):
        diff = torsten_val - cstreamer_val
        return f"{path}: torsten={torsten_val} cstreamer={cstreamer_val} (diff={diff:+d})"

    # Handle rational numbers (numerator/denominator dicts)
    if isinstance(torsten_val, dict) and isinstance(cstreamer_val, dict):
        if set(torsten_val.keys()) == {"numerator", "denominator"} and \
           set(cstreamer_val.keys()) == {"numerator", "denominator"}:
            t_val = torsten_val["numerator"] / torsten_val["denominator"]
            c_val = cstreamer_val["numerator"] / cstreamer_val["denominator"]
            if abs(t_val - c_val) < 1e-18:
                return None
            return f"{path}: torsten={t_val:.18f} cstreamer={c_val:.18f}"

    return f"{path}: torsten={json.dumps(torsten_val, sort_keys=True)[:200]} != cstreamer={json.dumps(cstreamer_val, sort_keys=True)[:200]}"


def compare_pool_distribution(torsten_pools, cstreamer_pools):
    """Compare pool distribution lists."""
    diffs = []

    # Build lookup by poolId
    t_pools = {p["poolId"]: p for p in (torsten_pools or [])}
    c_pools = {p["poolId"]: p for p in (cstreamer_pools or [])}

    t_ids = set(t_pools.keys())
    c_ids = set(c_pools.keys())

    only_torsten = t_ids - c_ids
    only_cstreamer = c_ids - t_ids

    if only_torsten:
        diffs.append(f"  poolDistribution: {len(only_torsten)} pools only in torsten")
    if only_cstreamer:
        diffs.append(f"  poolDistribution: {len(only_cstreamer)} pools only in cstreamer")

    # Compare stakeLovelace for common pools (not stake fraction, which differs
    # when total_active_stake changes even if individual pool stakes match)
    common = t_ids & c_ids
    stake_diffs = 0
    for pid in sorted(common):
        t_lv = t_pools[pid].get("stakeLovelace", 0)
        c_lv = c_pools[pid].get("stakeLovelace", 0)
        if t_lv != c_lv:
            stake_diffs += 1

    if stake_diffs:
        diffs.append(f"  poolDistribution: {stake_diffs}/{len(common)} common pools have different stakes")

    return diffs


def compare_snapshots_field(torsten_snaps, cstreamer_snaps):
    """Compare the snapshots (mark/set/go) fields."""
    diffs = []
    if torsten_snaps is None and cstreamer_snaps is None:
        return diffs

    for snap_name in ["mark", "set", "go"]:
        t_snap = (torsten_snaps or {}).get(snap_name) or {}
        c_snap = (cstreamer_snaps or {}).get(snap_name) or {}

        # Compare high-level snapshot fields
        for field in ["blocks", "totalStake"]:
            t_val = t_snap.get(field)
            c_val = c_snap.get(field)
            if t_val != c_val:
                diffs.append(f"  snapshots.{snap_name}.{field}: torsten={t_val} cstreamer={c_val}")

        # Compare delegations count
        t_deleg = len(t_snap.get("delegations", {}))
        c_deleg = len(c_snap.get("delegations", {}))
        if t_deleg != c_deleg:
            diffs.append(f"  snapshots.{snap_name}.delegations: torsten={t_deleg} cstreamer={c_deleg} entries")

        # Compare pool params count
        t_pp = len(t_snap.get("poolParams", {}))
        c_pp = len(c_snap.get("poolParams", {}))
        if t_pp != c_pp:
            diffs.append(f"  snapshots.{snap_name}.poolParams: torsten={t_pp} cstreamer={c_pp} entries")

        # Compare stake count
        t_stake = len(t_snap.get("stake", {}))
        c_stake = len(c_snap.get("stake", {}))
        if t_stake != c_stake:
            diffs.append(f"  snapshots.{snap_name}.stake: torsten={t_stake} cstreamer={c_stake} entries")

    return diffs


def compare_epoch(torsten, cstreamer, verbose=False):
    """Compare two epoch snapshots. Returns list of difference strings."""
    diffs = []

    # Core fields
    for field in CORE_FIELDS:
        d = compare_value(field, torsten.get(field), cstreamer.get(field))
        if d:
            diffs.append(f"  {d}")

    # Deposits
    t_deps = torsten.get("deposits", {})
    c_deps = cstreamer.get("deposits", {})
    for field in DEPOSIT_FIELDS:
        d = compare_value(f"deposits.{field}", t_deps.get(field), c_deps.get(field))
        if d:
            diffs.append(f"  {d}")

    # Protocol params
    t_pp = torsten.get("protocolParams", {})
    c_pp = cstreamer.get("protocolParams", {})
    for field in PROTOCOL_PARAM_FIELDS:
        d = compare_value(f"protocolParams.{field}", t_pp.get(field), c_pp.get(field))
        if d:
            diffs.append(f"  {d}")

    # RUPD
    for rupd_name in ["rupdApplied", "rupdNext"]:
        t_rupd = torsten.get(rupd_name, {})
        c_rupd = cstreamer.get(rupd_name, {})
        if t_rupd is None:
            t_rupd = {}
        if c_rupd is None:
            c_rupd = {}
        for field in RUPD_FIELDS:
            d = compare_value(f"{rupd_name}.{field}", t_rupd.get(field), c_rupd.get(field))
            if d:
                diffs.append(f"  {d}")

    # Eta
    d = compare_value("eta", torsten.get("eta"), cstreamer.get("eta"))
    if d:
        diffs.append(f"  {d}")

    # Pool distribution
    pool_diffs = compare_pool_distribution(
        torsten.get("poolDistribution"), cstreamer.get("poolDistribution")
    )
    diffs.extend(pool_diffs)

    # Snapshots (mark/set/go)
    if verbose:
        snap_diffs = compare_snapshots_field(
            torsten.get("snapshots"), cstreamer.get("snapshots")
        )
        diffs.extend(snap_diffs)

    return diffs


def main():
    parser = argparse.ArgumentParser(description="Compare torsten vs cstreamer epoch snapshots")
    parser.add_argument("torsten_dir", help="Path to torsten epoch-snapshots directory")
    parser.add_argument("cstreamer_dir", help="Path to cstreamer reference snapshots directory")
    parser.add_argument("--start-epoch", type=int, default=0, help="Start comparing from this epoch")
    parser.add_argument("--stop-epoch", type=int, default=999999, help="Stop comparing at this epoch")
    parser.add_argument("--verbose", action="store_true", help="Show snapshot field comparisons")
    parser.add_argument("--summary", action="store_true", help="Only show summary, not per-epoch details")
    args = parser.parse_args()

    print(f"Loading torsten snapshots from {args.torsten_dir}...")
    torsten_snaps = load_snapshots(args.torsten_dir)
    print(f"  Found {len(torsten_snaps)} epochs")

    print(f"Loading cstreamer snapshots from {args.cstreamer_dir}...")
    cstreamer_snaps = load_snapshots(args.cstreamer_dir)
    print(f"  Found {len(cstreamer_snaps)} epochs")

    # Find common epochs in range
    common_epochs = sorted(
        set(torsten_snaps.keys()) & set(cstreamer_snaps.keys())
    )
    common_epochs = [e for e in common_epochs if args.start_epoch <= e <= args.stop_epoch]

    print(f"\nComparing {len(common_epochs)} common epochs (range {args.start_epoch}-{args.stop_epoch})...\n")

    perfect = 0
    divergent = 0
    first_divergence = None
    divergence_summary = defaultdict(int)

    for epoch in common_epochs:
        diffs = compare_epoch(torsten_snaps[epoch], cstreamer_snaps[epoch], verbose=args.verbose)
        if diffs:
            divergent += 1
            if first_divergence is None:
                first_divergence = epoch
            if not args.summary:
                print(f"EPOCH {epoch}: {len(diffs)} differences")
                for d in diffs:
                    print(f"  {d}")
                print()
            for d in diffs:
                # Extract field name for summary
                field = d.strip().split(":")[0]
                divergence_summary[field] += 1
        else:
            perfect += 1

    # Only in torsten
    only_torsten = sorted(set(torsten_snaps.keys()) - set(cstreamer_snaps.keys()))
    only_torsten = [e for e in only_torsten if args.start_epoch <= e <= args.stop_epoch]

    # Only in cstreamer
    only_cstreamer = sorted(set(cstreamer_snaps.keys()) - set(torsten_snaps.keys()))
    only_cstreamer = [e for e in only_cstreamer if args.start_epoch <= e <= args.stop_epoch]

    print("=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"  Epochs compared: {len(common_epochs)}")
    print(f"  Perfect match:   {perfect}")
    print(f"  Divergent:       {divergent}")
    if first_divergence is not None:
        print(f"  First divergence: epoch {first_divergence}")
    if only_torsten:
        print(f"  Only in torsten:  {len(only_torsten)} epochs ({only_torsten[0]}-{only_torsten[-1]})")
    if only_cstreamer:
        print(f"  Only in cstreamer: {len(only_cstreamer)} epochs ({only_cstreamer[0]}-{only_cstreamer[-1]})")

    if divergence_summary:
        print(f"\n  Most common divergences:")
        for field, count in sorted(divergence_summary.items(), key=lambda x: -x[1])[:15]:
            print(f"    {field}: {count} epochs")

    if divergent == 0 and not only_torsten and not only_cstreamer:
        print(f"\n  *** PERFECT MATCH across all {len(common_epochs)} epochs! ***")
        return 0
    else:
        return 1


if __name__ == "__main__":
    sys.exit(main())
