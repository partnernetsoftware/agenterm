#!/usr/bin/env python3
"""Audit AgenTerm's public Windows-signing receipt and exact signed files."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
FORBIDDEN_KEYS = {
    "provider_resource",
    "endpoint",
    "account",
    "account_name",
    "signing_account",
    "certificate_profile",
    "profile_name",
    "resource_group",
    "validation_id",
    "application_id",
    "object_id",
    "federated_credential",
    "azure_client_id",
    "client_id",
    "azure_tenant_id",
    "tenant_id",
    "azure_subscription_id",
    "subscription_id",
    "client_secret",
    "access_token",
}
EXPECTED_PLATFORMS = {"windows-x86_64", "windows-aarch64"}
EXPECTED_ARCHES = {"windows-x86_64": "x86_64", "windows-aarch64": "aarch64"}


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


def require_text(row: dict[str, Any], field: str) -> str:
    value = row.get(field)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"missing {field}")
    return value.strip()


def reject_protected_keys(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).strip().casefold().replace("-", "_")
            if normalized in FORBIDDEN_KEYS:
                raise ValueError(f"protected configuration key at {path}.{key}")
            reject_protected_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_protected_keys(child, f"{path}[{index}]")


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


def audit(
    receipt: dict[str, Any],
    root: Path,
    archive_root: Path,
    release_eligible: bool,
    expected_source: str | None = None,
    expected_version: str | None = None,
) -> None:
    reject_protected_keys(receipt)
    if receipt.get("schema_version") != 1 or receipt.get("kind") != "agenterm-azure-artifact-signing":
        raise ValueError("signing receipt schema mismatch")
    source = str(receipt.get("source_sha", ""))
    if not SOURCE_RE.fullmatch(source) or (expected_source and source != expected_source):
        raise ValueError("source SHA mismatch")
    version = require_text(receipt, "version")
    if expected_version and version != expected_version:
        raise ValueError("version mismatch")
    if receipt.get("signing_provider") != "azure-artifact-signing":
        raise ValueError("signing provider mismatch")
    publisher = require_text(receipt, "publisher_organization")
    if publisher != "PARTNERNET SOFTWARE PTY LTD":
        raise ValueError("publisher organization mismatch")
    if receipt.get("file_digest") != "SHA256":
        raise ValueError("file digest policy mismatch")
    if receipt.get("timestamp_rfc3161") != "http://timestamp.acs.microsoft.com":
        raise ValueError("RFC 3161 timestamp policy mismatch")
    if receipt.get("timestamp_digest") != "SHA256":
        raise ValueError("timestamp digest policy mismatch")
    if receipt.get("release_eligible") is not release_eligible:
        raise ValueError("release eligibility mismatch")
    upstream = receipt.get("upstream")
    if not release_eligible:
        if (
            not isinstance(upstream, dict)
            or type(upstream.get("run_id")) is not int
            or type(upstream.get("run_attempt")) is not int
        ):
            raise ValueError("qualification upstream Candidate identity missing")
        if upstream["run_id"] <= 0 or upstream["run_attempt"] <= 0:
            raise ValueError("qualification upstream Candidate identity invalid")
    elif upstream is not None:
        raise ValueError("release-eligible receipt must not claim an upstream Candidate")
    if receipt.get("platform_count") != 2 or receipt.get("asset_count") != 10:
        raise ValueError("signed count mismatch")
    platforms = receipt.get("platforms")
    if not isinstance(platforms, dict) or set(platforms) != EXPECTED_PLATFORMS:
        raise ValueError("signed platform set mismatch")
    for platform, row in platforms.items():
        if not isinstance(row, dict) or row.get("arch") != EXPECTED_ARCHES[platform]:
            raise ValueError(f"{platform}: archive platform mismatch")
        name = require_text(row, "archive_name")
        if PurePosixPath(name).name != name:
            raise ValueError(f"{platform}: unsafe archive name")
        before = str(row.get("before_sha256", ""))
        after = str(row.get("after_sha256", ""))
        if not SHA256_RE.fullmatch(before) or not SHA256_RE.fullmatch(after) or before == after:
            raise ValueError(f"{platform}: invalid archive before/after SHA-256")
        archive = safe_file(archive_root, f"{platform}/{name}")
        if type(row.get("after_bytes")) is not int or row["after_bytes"] <= 0:
            raise ValueError(f"{platform}: invalid archive byte count")
        if sha256(archive) != after or archive.stat().st_size != row["after_bytes"]:
            raise ValueError(f"{platform}: final archive identity mismatch")
        if type(row.get("payload_uncompressed_bytes")) is not int or row["payload_uncompressed_bytes"] <= 0:
            raise ValueError(f"{platform}: invalid payload byte count")
    run = receipt.get("run")
    if not isinstance(run, dict) or type(run.get("id")) is not int or type(run.get("attempt")) is not int:
        raise ValueError("signing run identity missing")
    if run["id"] <= 0 or run["attempt"] <= 0:
        raise ValueError("signing run identity invalid")
    assets = receipt.get("assets")
    if not isinstance(assets, dict) or len(assets) != 10:
        raise ValueError("signed asset set mismatch")
    for name, row in assets.items():
        if not isinstance(row, dict) or row.get("path") != name:
            raise ValueError(f"{name}: asset path mismatch")
        before = str(row.get("before_sha256", ""))
        after = str(row.get("after_sha256", ""))
        if not SHA256_RE.fullmatch(before) or not SHA256_RE.fullmatch(after) or before == after:
            raise ValueError(f"{name}: invalid before/after SHA-256")
        path = safe_file(root, name)
        if type(row.get("after_bytes")) is not int or row["after_bytes"] <= 0:
            raise ValueError(f"{name}: invalid signed file byte count")
        if sha256(path) != after or path.stat().st_size != row["after_bytes"]:
            raise ValueError(f"{name}: signed file identity mismatch")
        if row.get("authenticode_status") != "Valid":
            raise ValueError(f"{name}: Authenticode is not Valid")
        if row.get("product_name") != "AgenTerm" or row.get("product_version") not in {version, f"{version}.0"}:
            raise ValueError(f"{name}: VERSIONINFO mismatch")
        signer = require_text(row, "signer_subject")
        if not re.search(r"(?:^|,\s*)O=" + re.escape(publisher) + r"(?:,|$)", signer):
            raise ValueError(f"{name}: signer organization mismatch")
        for field in ("signer_issuer", "signer_thumbprint", "signer_not_before", "signer_not_after", "timestamp_subject", "timestamp_issuer"):
            require_text(row, field)


def self_test() -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        archive_root = root / "archives"
        assets: dict[str, Any] = {}
        for platform in sorted(EXPECTED_PLATFORMS):
            for index in range(5):
                relative = f"{platform}/fixture-{index}.exe"
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(relative.encode())
                assets[relative] = {
                    "path": relative,
                    "before_sha256": hashlib.sha256(("before-" + relative).encode()).hexdigest(),
                    "after_sha256": sha256(path),
                    "after_bytes": path.stat().st_size,
                    "authenticode_status": "Valid",
                    "product_name": "AgenTerm",
                    "product_version": "0.0.0",
                    "signer_subject": "CN=PARTNERNET SOFTWARE PTY LTD, O=PARTNERNET SOFTWARE PTY LTD, C=AU",
                    "signer_issuer": "CN=Fixture CA",
                    "signer_thumbprint": "00",
                    "signer_not_before": "2026-01-01T00:00:00Z",
                    "signer_not_after": "2026-01-02T00:00:00Z",
                    "timestamp_subject": "CN=Fixture TSA",
                    "timestamp_issuer": "CN=Fixture TSA CA",
                }
        platforms: dict[str, Any] = {}
        for platform in sorted(EXPECTED_PLATFORMS):
            archive_name = f"agenterm-0.0.0-{platform}.zip"
            archive = archive_root / platform / archive_name
            archive.parent.mkdir(parents=True, exist_ok=True)
            archive.write_bytes(("archive-" + platform).encode())
            platforms[platform] = {
                "arch": EXPECTED_ARCHES[platform],
                "archive_name": archive_name,
                "before_sha256": hashlib.sha256(("before-" + platform).encode()).hexdigest(),
                "after_sha256": sha256(archive),
                "after_bytes": archive.stat().st_size,
                "payload_uncompressed_bytes": 1,
            }
        receipt = {
            "schema_version": 1,
            "kind": "agenterm-azure-artifact-signing",
            "source_sha": "a" * 40,
            "version": "0.0.0",
            "signing_provider": "azure-artifact-signing",
            "publisher_organization": "PARTNERNET SOFTWARE PTY LTD",
            "file_digest": "SHA256",
            "timestamp_rfc3161": "http://timestamp.acs.microsoft.com",
            "timestamp_digest": "SHA256",
            "release_eligible": False,
            "platform_count": 2,
            "asset_count": 10,
            "run": {"id": 1, "attempt": 1},
            "upstream": {"run_id": 2, "run_attempt": 1},
            "platforms": platforms,
            "assets": assets,
        }
        audit(receipt, root, archive_root, False, "a" * 40, "0.0.0")
        invalid_cases = []
        receipt["provider_resource"] = {"account": "must-not-ship"}
        invalid_cases.append(("protected configuration key", dict(receipt)))
        receipt.pop("provider_resource")
        receipt["timestamp_rfc3161"] = "http://example.invalid/timestamp"
        invalid_cases.append(("RFC 3161 timestamp policy mismatch", dict(receipt)))
        receipt["timestamp_rfc3161"] = "http://timestamp.acs.microsoft.com"
        receipt["run"] = {"id": 0, "attempt": 1}
        invalid_cases.append(("signing run identity invalid", dict(receipt)))
        for expected, invalid in invalid_cases:
            try:
                audit(invalid, root, archive_root, False)
            except ValueError as error:
                assert expected in str(error), (expected, str(error))
            else:
                raise AssertionError(f"invalid signing receipt accepted: {expected}")
    print("PASS AgenTerm signing receipt, exact files, and privacy court")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("receipt", type=Path, nargs="?")
    parser.add_argument("--root", type=Path)
    parser.add_argument("--archive-root", type=Path)
    parser.add_argument("--release-eligible", choices=("true", "false"), required=False)
    parser.add_argument("--expected-source")
    parser.add_argument("--expected-version")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.receipt is None or args.root is None or args.archive_root is None or args.release_eligible is None:
        parser.error("receipt, --root, --archive-root, and --release-eligible are required")
    audit(
        load(args.receipt),
        args.root,
        args.archive_root,
        args.release_eligible == "true",
        args.expected_source,
        args.expected_version,
    )
    print("PASS AgenTerm signing receipt, exact files, and privacy court")


if __name__ == "__main__":
    main()
