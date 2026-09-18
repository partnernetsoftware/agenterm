#!/usr/bin/env python3
"""Audit AgenTerm's public macOS signing receipt and the exact signed files.

Two subcommands:

  list-macho  print the exact Mach-O names the macOS signing lane owns for one
              architecture, derived from scripts/artifacts.json rather than from
              a handwritten glob.
  audit       verify a macos-signing-receipt.json against the bytes it claims,
              and reject any protected Apple identifier that leaked into it.

The certificate's Team identifier and the Developer ID publisher name are public
provenance, exactly like the Windows publisher name. The App Store Connect Key
ID, the Issuer ID, the .p8/.p12 material, the p12 password and any keychain
password are not, and a receipt carrying one of them is rejected here rather
than after it has been published.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
TEAM_RE = re.compile(r"^[A-Z0-9]{10}$")
EXPECTED_PLATFORMS = {"macos-aarch64": "aarch64", "macos-x86_64": "x86_64"}

# Protected Apple coordinates. These are matched on normalized key names, so
# "key-id", "key_id" and "Key Id" are all caught.
FORBIDDEN_KEYS = {
    "apple_id",
    "asc_api_issuer_id",
    "asc_api_key_id",
    "certificate",
    "client_secret",
    "issuer",
    "issuer_id",
    "key_id",
    "keychain",
    "keychain_password",
    "notary_issuer_id",
    "notary_key",
    "notary_key_id",
    "notary_password",
    "p12",
    "p12_password",
    "p8",
    "password",
    "private_key",
}
# A .p8 or .p12 blob must never appear as a value either, whatever it is keyed on.
FORBIDDEN_VALUE_RE = re.compile(
    r"-----BEGIN (?:EC |RSA )?PRIVATE KEY-----|BEGIN CERTIFICATE"
)


def load(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError("receipt must be a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def reject_protected(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).strip().casefold().replace("-", "_")
            if normalized in FORBIDDEN_KEYS:
                raise ValueError(f"protected configuration key at {path}.{key}")
            reject_protected(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_protected(child, f"{path}[{index}]")
    elif isinstance(value, str) and FORBIDDEN_VALUE_RE.search(value):
        raise ValueError(f"key or certificate material at {path}")


def safe_file(root: Path, relative: str) -> Path:
    parsed = PurePosixPath(relative)
    if (
        parsed.is_absolute()
        or not parsed.parts
        or ".." in parsed.parts
        or "\\" in relative
        or re.match(r"^[A-Za-z]:", relative)
    ):
        raise ValueError(f"unsafe signed asset path: {relative}")
    path = root.joinpath(*parsed.parts)
    if not path.is_file():
        raise ValueError(f"missing signed asset: {relative}")
    return path


def macho_names(manifest_path: Path, arch: str) -> list[str]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    matches = [
        platform
        for platform in manifest.get("platforms", [])
        if platform.get("os") == "macos" and platform.get("arch") == arch
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one macos-{arch} manifest entry, found {len(matches)}")
    # Libraries first: the signing order is inner-out and the receipt's asset set
    # must be the same closed set the signer walked.
    names = [
        artifact["name"]
        for artifact in matches[0].get("libraries", []) + matches[0].get("executables", [])
        if artifact.get("name")
    ]
    if len(names) != len(set(names)) or not names:
        raise ValueError(f"macos-{arch} manifest artifact names are not a unique set")
    return names


def audit(args: argparse.Namespace) -> None:
    receipt_path = Path(args.receipt)
    receipt = load(receipt_path)
    reject_protected(receipt)

    if receipt.get("schema_version") != 1:
        raise ValueError("unsupported receipt schema_version")
    if receipt.get("kind") not in {
        "agenterm-macos-signing-qualification",
        "agenterm-macos-signing-candidate",
    }:
        raise ValueError(f"unexpected receipt kind: {receipt.get('kind')}")
    if receipt.get("signing_provider") != "apple-developer-id":
        raise ValueError("receipt does not name the Apple Developer ID provider")

    source = receipt.get("source_sha")
    if not isinstance(source, str) or not SOURCE_RE.match(source):
        raise ValueError("receipt source_sha is not a 40-character commit id")
    if args.expected_source and source != args.expected_source:
        raise ValueError("receipt source_sha does not match the expected source")

    version = receipt.get("version")
    if not isinstance(version, str) or not VERSION_RE.match(version):
        raise ValueError("receipt version is not semver-shaped")
    if args.expected_version and version != args.expected_version:
        raise ValueError("receipt version does not match the expected version")

    platform_id = receipt.get("platform_id")
    if platform_id not in EXPECTED_PLATFORMS:
        raise ValueError(f"unexpected platform_id: {platform_id}")
    if args.expected_platform and platform_id != args.expected_platform:
        raise ValueError("receipt platform_id does not match the expected platform")

    team = receipt.get("team_identifier")
    if not isinstance(team, str) or not TEAM_RE.match(team):
        raise ValueError("receipt team_identifier is not an Apple Team identifier")

    expected_eligible = args.release_eligible == "true"
    if receipt.get("release_eligible") is not expected_eligible:
        raise ValueError(
            "receipt release eligibility does not match the declared mode"
        )

    assets = receipt.get("assets")
    if not isinstance(assets, dict) or not assets:
        raise ValueError("receipt carries no signed assets")
    expected_names = set(
        macho_names(Path(args.manifest), EXPECTED_PLATFORMS[platform_id])
    )
    if set(assets) != expected_names:
        missing = sorted(expected_names - set(assets))
        extra = sorted(set(assets) - expected_names)
        raise ValueError(
            f"signed asset set drift; missing={missing} unexpected={extra}"
        )

    root = Path(args.root)
    for name, row in sorted(assets.items()):
        if not isinstance(row, dict):
            raise ValueError(f"asset {name} is not an object")
        before = row.get("before_sha256")
        after = row.get("after_sha256")
        for label, value in (("before_sha256", before), ("after_sha256", after)):
            if not isinstance(value, str) or not SHA256_RE.match(value):
                raise ValueError(f"asset {name} has an invalid {label}")
        if before == after:
            raise ValueError(f"asset {name} was not changed by the provider")
        if row.get("hardened_runtime") is not True:
            raise ValueError(f"asset {name} was not signed with the hardened runtime")
        path = safe_file(root, name)
        observed = sha256(path)
        if observed != after:
            raise ValueError(f"asset {name} on disk does not match after_sha256")
        size = row.get("bytes")
        if not isinstance(size, int) or size != path.stat().st_size:
            raise ValueError(f"asset {name} byte count does not match the file")

    bundle = receipt.get("bundle")
    if not isinstance(bundle, dict):
        raise ValueError("receipt carries no bundle evidence")
    if bundle.get("name") != "AgenTerm.app":
        raise ValueError("receipt bundle is not AgenTerm.app")
    if bundle.get("stapled") is not True:
        raise ValueError("receipt bundle is not stapled")
    if bundle.get("hardened_runtime") is not True:
        raise ValueError("receipt bundle was not signed with the hardened runtime")
    notarization = bundle.get("notarization_id")
    if not isinstance(notarization, str) or not notarization.strip():
        raise ValueError("receipt bundle has no notarization submission id")
    verdict = bundle.get("spctl")
    if not isinstance(verdict, list) or not verdict:
        raise ValueError("receipt bundle has no Gatekeeper verdict")
    joined = " ".join(str(line) for line in verdict)
    if "accepted" not in joined:
        raise ValueError("Gatekeeper did not accept the receipt bundle")
    if "source=Notarized Developer ID" not in joined:
        raise ValueError("receipt bundle is not a notarized Developer ID")

    print(
        "MACOS SIGNING RECEIPT OK "
        f"{platform_id} v{version} assets={len(assets)} "
        f"release_eligible={str(expected_eligible).lower()}"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    listing = sub.add_parser("list-macho")
    listing.add_argument("--manifest", default="scripts/artifacts.json")
    listing.add_argument("--arch", required=True, choices=sorted(set(EXPECTED_PLATFORMS.values())))

    auditor = sub.add_parser("audit")
    auditor.add_argument("receipt")
    auditor.add_argument("--root", required=True)
    auditor.add_argument("--manifest", default="scripts/artifacts.json")
    auditor.add_argument("--release-eligible", required=True, choices=["true", "false"])
    auditor.add_argument("--expected-source")
    auditor.add_argument("--expected-version")
    auditor.add_argument("--expected-platform")

    args = parser.parse_args(argv)
    try:
        if args.command == "list-macho":
            for name in macho_names(Path(args.manifest), args.arch):
                print(name)
        else:
            audit(args)
    except (ValueError, OSError, json.JSONDecodeError) as error:
        print(f"macOS signing receipt audit failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
