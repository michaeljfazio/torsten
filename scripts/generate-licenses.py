#!/usr/bin/env python3
"""Generate the third-party licenses documentation page from Cargo metadata.

Usage:
    python3 scripts/generate-licenses.py > docs/src/reference/third-party-licenses.md
"""

import json
import subprocess
import sys
from collections import defaultdict


def main():
    result = subprocess.run(
        ["cargo", "metadata", "--format-version=1"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print("Error: cargo metadata failed", file=sys.stderr)
        sys.exit(1)

    meta = json.loads(result.stdout)
    ws_names = set()
    for pkg in meta["packages"]:
        if pkg["name"].startswith("torsten"):
            ws_names.add(pkg["name"])

    # Collect deps (latest version per name)
    deps = {}
    for pkg in meta["packages"]:
        if pkg["name"] in ws_names:
            continue
        name = pkg["name"]
        if name not in deps or pkg["version"] > deps[name]["version"]:
            deps[name] = {
                "version": pkg["version"],
                "license": (pkg.get("license") or "Unknown").strip(),
                "repository": (pkg.get("repository") or ""),
                "description": (pkg.get("description") or "").strip()[:120],
            }

    def normalize_license(lic):
        return lic.replace("/", " OR ")

    license_groups = defaultdict(list)
    for name in sorted(deps.keys()):
        d = deps[name]
        license_groups[normalize_license(d["license"])].append((name, d))

    summary = defaultdict(int)
    for lic, pkgs in license_groups.items():
        summary[lic] += len(pkgs)

    # Key direct dependencies
    key_crates = [
        "pallas-codec", "pallas-crypto", "pallas-primitives", "pallas-traverse",
        "pallas-addresses", "pallas-network",
        "cardano-lsm", "uplc",
        "tokio", "hyper", "reqwest", "clap",
        "serde", "serde_json", "bincode",
        "blake2b_simd", "sha2", "ed25519-dalek", "curve25519-dalek", "blst", "k256",
        "minicbor",
        "tracing", "tracing-subscriber",
        "dashmap", "crossbeam",
        "dashu-int",
        "memmap2", "lz4", "zstd", "tar",
        "crc32fast", "hex", "bs58", "bech32", "base64",
        "rand", "chrono", "uuid",
        "indicatif",
        "vrf_dalek",
    ]

    lines = []
    lines.append("# Third-Party Licenses")
    lines.append("")
    lines.append("Torsten depends on a number of open-source Rust crates. This page documents")
    lines.append("all third-party dependencies and their license terms.")
    lines.append("")
    lines.append(f"**Total dependencies:** {len(deps)}")
    lines.append("")

    lines.append("## License Summary")
    lines.append("")
    lines.append("| License | Count |")
    lines.append("|---------|-------|")
    for lic in sorted(summary.keys(), key=lambda l: -summary[l]):
        lines.append(f"| {lic} | {summary[lic]} |")
    lines.append("")

    lines.append("## Key Dependencies")
    lines.append("")
    lines.append("These are the primary libraries that Torsten directly depends on:")
    lines.append("")
    lines.append("| Crate | Version | License | Description |")
    lines.append("|-------|---------|---------|-------------|")
    for crate_name in key_crates:
        if crate_name in deps:
            d = deps[crate_name]
            desc = d["description"].replace("|", "-")
            if len(desc) > 80:
                desc = desc[:77] + "..."
            repo = d["repository"]
            name_link = f"[{crate_name}]({repo})" if repo else crate_name
            lines.append(f"| {name_link} | {d['version']} | {d['license']} | {desc} |")
    lines.append("")

    lines.append("## All Dependencies")
    lines.append("")
    lines.append("Complete list of all third-party crates used by Torsten, sorted alphabetically.")
    lines.append("")
    lines.append("| Crate | Version | License |")
    lines.append("|-------|---------|---------|")
    for name in sorted(deps.keys()):
        d = deps[name]
        repo = d["repository"]
        name_link = f"[{name}]({repo})" if repo else name
        lines.append(f"| {name_link} | {d['version']} | {d['license']} |")
    lines.append("")
    lines.append("## Regenerating This Page")
    lines.append("")
    lines.append("This page is generated from `Cargo.lock` metadata. To regenerate after dependency changes:")
    lines.append("")
    lines.append("```bash")
    lines.append("python3 scripts/generate-licenses.py > docs/src/reference/third-party-licenses.md")
    lines.append("```")

    print("\n".join(lines))


if __name__ == "__main__":
    main()
