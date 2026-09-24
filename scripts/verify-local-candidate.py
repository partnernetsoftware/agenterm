#!/usr/bin/env python3
"""Safely unpack and validate a locally built six-cell Candidate staging bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tarfile
from pathlib import Path, PurePosixPath

sys.dont_write_bytecode = True

CELLS = {
    "windows-x86_64": ("win-x86_64", "x86_64-pc-windows-msvc"),
    "windows-aarch64": ("win-aarch64", "aarch64-pc-windows-msvc"),
    "linux-x86_64": ("lnx-x86_64", "x86_64-unknown-linux-gnu"),
    "linux-aarch64": ("lnx-aarch64", "aarch64-unknown-linux-gnu"),
    "macos-x86_64": ("osx-x86_64", "x86_64-apple-darwin"),
    "macos-aarch64": ("osx-aarch64", "aarch64-apple-darwin"),
}
MAX_BUNDLE_BYTES = 2 * 1024 * 1024 * 1024
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_json(path: Path, label: str) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        raise SystemExit(f"invalid local Candidate {label}") from None
    if not isinstance(value, dict):
        raise SystemExit(f"invalid local Candidate {label}")
    return value


def safe_unpack(archive: Path, checksum: Path, output: Path) -> Path:
    sidecar = checksum.read_text(encoding="utf-8").split()
    if len(sidecar) != 2 or sidecar[1] != archive.name:
        raise SystemExit("local Candidate bundle checksum sidecar is malformed")
    if sha256(archive.read_bytes()) != sidecar[0]:
        raise SystemExit("local Candidate bundle checksum mismatch")

    root = output / "candidate-input"
    if root.exists():
        raise SystemExit("local Candidate extraction directory already exists")
    root.mkdir(parents=True)
    total = 0
    seen: set[str] = set()
    try:
        with tarfile.open(archive, "r:gz") as bundle:
            members = bundle.getmembers()
            for member in members:
                name = PurePosixPath(member.name)
                if (
                    name.is_absolute()
                    or not name.parts
                    or name.parts[0] != "candidate-input"
                    or any(part in {"", ".", ".."} for part in name.parts)
                    or member.issym()
                    or member.islnk()
                    or not (member.isfile() or member.isdir())
                    or member.name in seen
                ):
                    raise SystemExit("local Candidate bundle contains an unsafe entry")
                seen.add(member.name)
                total += max(0, member.size)
                if total > MAX_BUNDLE_BYTES:
                    raise SystemExit("local Candidate bundle exceeds the size limit")
            for member in members:
                relative = PurePosixPath(member.name).parts[1:]
                destination = root.joinpath(*relative)
                if member.isdir():
                    destination.mkdir(parents=True, exist_ok=True)
                    continue
                destination.parent.mkdir(parents=True, exist_ok=True)
                source = bundle.extractfile(member)
                if source is None:
                    raise SystemExit("local Candidate bundle entry could not be read")
                with source, destination.open("xb") as target:
                    target.write(source.read())
    except (OSError, tarfile.TarError):
        raise SystemExit("local Candidate bundle could not be unpacked") from None
    return root


def validate(root: Path, source_sha: str, version: str) -> dict:
    manifest = read_json(root / "local-build-manifest.json", "build manifest")
    receipt = read_json(root / "qualification-receipt.json", "qualification receipt")
    if (
        manifest.get("schema_version") != 1
        or manifest.get("kind") != "agenterm-local-six-cell-build"
        or manifest.get("source_sha") != source_sha
        or manifest.get("version") != version
        or manifest.get("profile") != "release"
    ):
        raise SystemExit("local Candidate build manifest identity mismatch")
    if (
        receipt.get("schema_version") != 2
        or receipt.get("profile") != "prebuilt-six-cell-execute-only"
        or receipt.get("source_sha") != source_sha
        or receipt.get("version") != version
        or receipt.get("release") is not True
        or receipt.get("stress_included") is not False
        or receipt.get("local_build_manifest", {}).get("name") != "local-build-manifest.json"
        or receipt.get("local_build_manifest", {}).get("size")
        != (root / "local-build-manifest.json").stat().st_size
        or receipt.get("local_build_manifest", {}).get("sha256") != sha256(
            (root / "local-build-manifest.json").read_bytes()
        )
    ):
        raise SystemExit("local Candidate qualification receipt identity mismatch")
    pre_push = manifest.get("pre_push_check", {})
    pre_push_log = root / "pre-push-check.log"
    if (
        pre_push.get("status") != "passed"
        or not SHA256.fullmatch(pre_push.get("sha256", ""))
        or pre_push.get("name") != "pre-push-check.log"
        or not pre_push_log.is_file()
        or not isinstance(pre_push.get("size"), int)
        or pre_push.get("size") != pre_push_log.stat().st_size
        or sha256(pre_push_log.read_bytes()) != pre_push.get("sha256")
        or receipt.get("pre_push_check") != pre_push
        or receipt.get("release_policy_sha256")
        != manifest.get("source_inputs", {}).get("release_policy_sha256")
    ):
        raise SystemExit("local Candidate pre-push evidence is missing")

    source_inputs = manifest.get("source_inputs", {})
    cells = manifest.get("cells")
    if not isinstance(cells, list) or len(cells) != len(CELLS):
        raise SystemExit("local Candidate build manifest does not contain six cells")
    cells_by_id = {item.get("platform_id"): item for item in cells if isinstance(item, dict)}
    if set(cells_by_id) != set(CELLS):
        raise SystemExit("local Candidate build manifest cell set is invalid")
    for platform_id, (_, target) in CELLS.items():
        cell = cells_by_id[platform_id]
        artifacts = cell.get("artifacts")
        if cell.get("target") != target or cell.get("state") != "PASS" or not isinstance(artifacts, list) or not artifacts:
            raise SystemExit(f"local Candidate build cell is invalid: {platform_id}")
        names: set[str] = set()
        for artifact in artifacts:
            name = artifact.get("name", "")
            digest = artifact.get("sha256", "")
            if (
                not isinstance(name, str)
                or not name
                or PurePosixPath(name).name != name
                or "\\" in name
                or name in names
                or not SHA256.fullmatch(digest)
                or not isinstance(artifact.get("bytes"), int)
                or artifact["bytes"] <= 0
            ):
                raise SystemExit(f"local Candidate artifact record is invalid: {platform_id}")
            names.add(name)
            if platform_id.startswith("linux-"):
                path = root / "raw-linux" / platform_id / name
            else:
                path = root / f"candidate-part-{platform_id}" / name
            if (
                not path.is_file()
                or path.stat().st_size != artifact["bytes"]
                or sha256(path.read_bytes()) != digest
            ):
                raise SystemExit(f"local Candidate artifact bytes mismatch: {platform_id}")

        if platform_id.startswith(("windows-", "macos-")):
            archive_name = artifacts[0]["name"]
            part = root / f"candidate-part-{platform_id}"
            provenance = read_json(
                part / f"{archive_name}.provenance.json",
                f"archive provenance {platform_id}",
            )
            checksum = (part / f"{archive_name}.sha256").read_text(encoding="utf-8").split()
            os_name, arch = platform_id.rsplit("-", 1)
            if (
                len(artifacts) != 1
                or len(checksum) != 2
                or checksum[1] != archive_name
                or checksum[0] != artifacts[0]["sha256"]
                or provenance.get("schema_version") != 1
                or provenance.get("product") != "AgenTerm"
                or provenance.get("artifact") != archive_name
                or provenance.get("source_commit") != source_sha
                or provenance.get("source_tag") != f"v{version}"
                or provenance.get("version") != version
                or provenance.get("sha256") != checksum[0]
                or provenance.get("os") != os_name
                or provenance.get("arch") != arch
                or provenance.get("cargo_lock_sha256") != source_inputs.get("cargo_lock_sha256")
                or provenance.get("artifact_manifest_sha256") != source_inputs.get("artifact_manifest_sha256")
                or (
                    os_name == "macos"
                    and provenance.get("sbom_sha256") != source_inputs.get("sbom_sha256")
                )
            ):
                raise SystemExit(f"local Candidate archive provenance mismatch: {platform_id}")

    loaders = manifest.get("chassis_loaders")
    if not isinstance(loaders, list) or len(loaders) != len(CELLS):
        raise SystemExit("local Candidate Chassis loader set is invalid")
    loader_map = {item.get("cell"): item for item in loaders if isinstance(item, dict)}
    if set(loader_map) != {cell for cell, _ in CELLS.values()}:
        raise SystemExit("local Candidate Chassis loader cells do not match")
    for platform_id, (cell, _) in CELLS.items():
        loader_dir = root / "chassis-l1" / cell
        loader_path = loader_dir / "loader"
        descriptor = read_json(loader_dir / "loader.json", f"loader descriptor {cell}")
        item = loader_map[cell]
        if (
            not loader_path.is_file()
            or descriptor.get("schema") != 1
            or descriptor.get("kind") != "agenterm-chassis-l1-loader"
            or descriptor.get("cell") != cell
            or descriptor.get("source_sha") != source_sha
            or descriptor.get("version") != version
            or descriptor.get("max_bytes") != 2 * 1024 * 1024
            or descriptor.get("sha256") != item.get("sha256")
            or descriptor.get("bytes") != item.get("bytes")
            or loader_path.stat().st_size != item.get("bytes")
            or sha256(loader_path.read_bytes()) != item.get("sha256")
        ):
            raise SystemExit(f"local Candidate Chassis loader mismatch: {cell}")

    chassis_name = f"agenterm-{version}-chassis-product.tgz"
    chassis_path = root / chassis_name
    if not chassis_path.is_file():
        raise SystemExit("local Candidate Chassis product is missing")
    checksum = (root / f"{chassis_name}.sha256").read_text(encoding="utf-8").split()
    provenance = read_json(root / f"{chassis_name}.provenance.json", "Chassis provenance")
    if (
        len(checksum) != 2
        or checksum[1] != chassis_name
        or sha256(chassis_path.read_bytes()) != checksum[0]
        or provenance.get("source_commit") != source_sha
        or provenance.get("version") != version
        or provenance.get("sha256") != checksum[0]
    ):
        raise SystemExit("local Candidate Chassis product identity mismatch")
    try:
        with tarfile.open(chassis_path, "r:gz") as chassis:
            members = chassis.getmembers()
            by_name = {member.name: member for member in members}
            if len(by_name) != len(members):
                raise SystemExit("local Candidate Chassis archive has duplicate members")
            manifest_member = by_name.get("manifest.json")
            if manifest_member is None or not manifest_member.isfile():
                raise SystemExit("local Candidate Chassis manifest is missing")
            stream = chassis.extractfile(manifest_member)
            if stream is None:
                raise SystemExit("local Candidate Chassis manifest could not be read")
            with stream:
                chassis_manifest = json.loads(stream.read().decode("utf-8"))
            l1 = chassis_manifest.get("l1_sha256", {})
            for platform_id, (cell, _) in CELLS.items():
                member = by_name.get(f"l1/{cell}/loader")
                expected = loader_map[cell]["sha256"]
                if (
                    member is None
                    or not member.isfile()
                    or l1.get(cell) != expected
                ):
                    raise SystemExit(f"local Candidate Chassis manifest mismatch: {cell}")
                content = chassis.extractfile(member)
                if content is None:
                    raise SystemExit(f"local Candidate Chassis loader is unreadable: {cell}")
                with content:
                    if sha256(content.read()) != expected:
                        raise SystemExit(f"local Candidate Chassis loader bytes mismatch: {cell}")
    except (OSError, UnicodeError, json.JSONDecodeError, tarfile.TarError):
        raise SystemExit("local Candidate Chassis product could not be verified") from None
    return {
        "kind": "agenterm-local-candidate-input",
        "source_sha": source_sha,
        "version": version,
        "cells": sorted(CELLS),
        "chassis_sha256": checksum[0],
        "pre_push_sha256": pre_push["sha256"],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--checksum", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    if len(args.source_sha) != 40 or any(ch not in "0123456789abcdef" for ch in args.source_sha):
        raise SystemExit("source SHA must be 40 lowercase hexadecimal characters")
    root = safe_unpack(args.archive, args.checksum, args.out)
    print(json.dumps(validate(root, args.source_sha, args.version), sort_keys=True))


if __name__ == "__main__":
    main()
