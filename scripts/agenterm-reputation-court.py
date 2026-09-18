#!/usr/bin/env python3
"""AgenTerm's reputation court: bind a local Defender scan to a sealed Candidate.

`release-policy.json` declares `reputation.windows_final_candidate_bytes:
"required"`. Until now nothing enforced it: the only Defender evidence AgenTerm
produced was the string `DEFENDER PASS` printed inside a GitHub-hosted Windows
runner, which is not bound to the sealed Candidate manifest and is not readable
by the release gate. This tool closes that gap.

  qualify  convert one `agenterm-defender-court` receipt (written by
           scripts/utm-win-defender-court.sh) plus the sealed Candidate manifest
           into an `agenterm-reputation-qualification`.
  verify   re-derive the same binding, so CI can check a qualification it did
           not produce.

The court proves one thing only: the exact sealed Windows Candidate bytes were
scanned by Microsoft Defender on a real Windows machine and came back clean, and
the scan did not alter them. It is not a signature, not a SmartScreen
reputation, and never release authority by itself.

This is deliberately AgenTerm-owned. It does not read, import or shell out to
any other product's reputation tooling, and its receipt kinds are namespaced to
`agenterm-` so a foreign receipt cannot satisfy this gate.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
from pathlib import Path
from typing import Any

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
COURT_KIND = "agenterm-defender-court"
QUALIFICATION_KIND = "agenterm-reputation-qualification"
# The policy field names the Windows final Candidate bytes, so that is exactly
# what must be scanned -- not a subset, and not a rebuilt convenience copy.
REQUIRED_PLATFORMS = {"windows-x86_64", "windows-aarch64"}


def load(path: Path) -> dict[str, Any]:
    value = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a JSON object")
    return value


def candidate_windows_assets(manifest: dict[str, Any]) -> dict[str, str]:
    """The sealed manifest's Windows archives, as {platform_id: sha256}."""
    assets = manifest.get("assets")
    if not isinstance(assets, list) or not assets:
        raise ValueError("candidate manifest carries no assets")
    selected: dict[str, str] = {}
    for asset in assets:
        if not isinstance(asset, dict) or asset.get("os") != "windows":
            continue
        arch = asset.get("arch")
        digest = asset.get("sha256")
        if not isinstance(arch, str) or not isinstance(digest, str):
            raise ValueError("windows asset is missing arch or sha256")
        if not SHA256_RE.match(digest):
            raise ValueError(f"windows-{arch} sha256 is malformed")
        platform_id = f"windows-{arch}"
        if platform_id in selected:
            raise ValueError(f"duplicate windows asset for {platform_id}")
        selected[platform_id] = digest
    if set(selected) != REQUIRED_PLATFORMS:
        raise ValueError(
            "candidate manifest does not carry exactly the two Windows archives: "
            f"{sorted(selected)}"
        )
    return selected


def check_court(court: dict[str, Any], manifest: dict[str, Any]) -> list[dict[str, Any]]:
    if court.get("schema_version") != 1:
        raise ValueError("unsupported Defender court schema_version")
    if court.get("kind") != COURT_KIND:
        raise ValueError(
            f"receipt kind is {court.get('kind')!r}, not {COURT_KIND!r}; "
            "a foreign product's court receipt is not AgenTerm evidence"
        )
    if court.get("verdict") != "clean":
        raise ValueError(f"Defender verdict is {court.get('verdict')!r}, not 'clean'")

    source = court.get("source_sha")
    if not isinstance(source, str) or not SOURCE_RE.match(source):
        raise ValueError("Defender court source_sha is malformed")
    if source != manifest.get("source_sha"):
        raise ValueError("Defender court was not run against the Candidate source SHA")

    run = court.get("candidate_run")
    manifest_run = manifest.get("run")
    if not isinstance(run, dict) or not isinstance(manifest_run, dict):
        raise ValueError("Defender court or manifest is missing run identity")
    if run.get("id") != manifest_run.get("id") or run.get("attempt") != manifest_run.get(
        "attempt"
    ):
        raise ValueError("Defender court is bound to a different Candidate run")

    expected = candidate_windows_assets(manifest)
    scanned = court.get("assets")
    if not isinstance(scanned, list) or not scanned:
        raise ValueError("Defender court scanned nothing")

    observed: dict[str, str] = {}
    for asset in scanned:
        if not isinstance(asset, dict):
            raise ValueError("Defender court asset row is not an object")
        platform_id = asset.get("platform_id")
        before = asset.get("sha256")
        after = asset.get("post_scan_sha256")
        for label, value in (("sha256", before), ("post_scan_sha256", after)):
            if not isinstance(value, str) or not SHA256_RE.match(value):
                raise ValueError(f"asset {platform_id} has an invalid {label}")
        # The scan must not have modified, quarantined or remediated the bytes.
        if before != after:
            raise ValueError(
                f"Defender changed the bytes of {platform_id}; the scanned "
                "artifact is no longer the sealed Candidate artifact"
            )
        if asset.get("threats_detected") not in (0, None):
            raise ValueError(f"Defender reported a detection on {platform_id}")
        if platform_id in observed:
            raise ValueError(f"duplicate Defender asset row for {platform_id}")
        observed[platform_id] = before

    if observed != expected:
        raise ValueError(
            "Defender scanned bytes are not the sealed Candidate Windows archives"
        )
    return sorted(
        (
            {
                "platform_id": platform_id,
                "sha256": digest,
                "post_scan_sha256": digest,
            }
            for platform_id, digest in observed.items()
        ),
        key=lambda row: row["platform_id"],
    )


def build_qualification(
    manifest: dict[str, Any], court: dict[str, Any]
) -> dict[str, Any]:
    assets = check_court(court, manifest)
    return {
        "schema_version": 1,
        "kind": QUALIFICATION_KIND,
        "source_sha": manifest["source_sha"],
        "version": manifest.get("version"),
        "candidate_run": {
            "id": manifest["run"]["id"],
            "attempt": manifest["run"]["attempt"],
        },
        "scanner": {
            "engine": "microsoft-defender",
            "court": court.get("court"),
            "product_version": court.get("scanner", {}).get("product_version"),
            "signature_version": court.get("scanner", {}).get("signature_version"),
        },
        "verdict": "clean",
        "assets": assets,
        "qualified_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    }


def check_qualification(
    qualification: dict[str, Any], manifest: dict[str, Any]
) -> None:
    if qualification.get("schema_version") != 1:
        raise ValueError("unsupported qualification schema_version")
    if qualification.get("kind") != QUALIFICATION_KIND:
        raise ValueError(
            f"qualification kind is {qualification.get('kind')!r}, "
            f"not {QUALIFICATION_KIND!r}"
        )
    if qualification.get("verdict") != "clean":
        raise ValueError("qualification verdict is not clean")
    if qualification.get("source_sha") != manifest.get("source_sha"):
        raise ValueError("qualification is not bound to the Candidate source SHA")
    run = qualification.get("candidate_run")
    manifest_run = manifest.get("run")
    if not isinstance(run, dict) or not isinstance(manifest_run, dict):
        raise ValueError("qualification or manifest is missing run identity")
    if run.get("id") != manifest_run.get("id") or run.get("attempt") != manifest_run.get(
        "attempt"
    ):
        raise ValueError("qualification is bound to a different Candidate run")
    if qualification.get("version") != manifest.get("version"):
        raise ValueError("qualification version does not match the Candidate")

    expected = candidate_windows_assets(manifest)
    rows = qualification.get("assets")
    if not isinstance(rows, list) or len(rows) != len(expected):
        raise ValueError("qualification asset set is the wrong size")
    observed: dict[str, str] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("qualification asset row is not an object")
        platform_id = row.get("platform_id")
        digest = row.get("sha256")
        if not isinstance(digest, str) or not SHA256_RE.match(digest):
            raise ValueError(f"qualification asset {platform_id} sha256 is malformed")
        if row.get("post_scan_sha256") != digest:
            raise ValueError(
                f"qualification asset {platform_id} changed during the scan"
            )
        if platform_id in observed:
            raise ValueError(f"duplicate qualification asset row for {platform_id}")
        observed[platform_id] = digest
    if observed != expected:
        raise ValueError(
            "qualification does not cover exactly the sealed Windows Candidate bytes"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    qualify = sub.add_parser("qualify")
    qualify.add_argument("--manifest", required=True)
    qualify.add_argument("--defender", required=True)
    qualify.add_argument("--output", required=True)

    verify = sub.add_parser("verify")
    verify.add_argument("--manifest", required=True)
    verify.add_argument("--qualification", required=True)

    args = parser.parse_args(argv)
    try:
        manifest = load(Path(args.manifest))
        if args.command == "qualify":
            court = load(Path(args.defender))
            qualification = build_qualification(manifest, court)
            output = Path(args.output)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(
                json.dumps(qualification, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            print(
                "REPUTATION QUALIFIED "
                f"assets={len(qualification['assets'])} -> {output}"
            )
        else:
            check_qualification(load(Path(args.qualification)), manifest)
            print("REPUTATION QUALIFICATION OK")
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        print(f"reputation court failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
