#!/usr/bin/env python3
"""Behavior tests for the locally staged Candidate bundle verifier."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import sys
import subprocess
import tarfile
import re
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location(
    "verify_local_candidate", Path(__file__).with_name("verify-local-candidate.py")
)
assert SPEC is not None and SPEC.loader is not None
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)

SOURCE = "a" * 40
ROOT = Path(__file__).resolve().parents[1]
ROOT_CARGO = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
VERSION_MATCH = re.search(r"(?ms)^\[package\].*?^version\s*=\s*\"([^\"]+)\"", ROOT_CARGO)
assert VERSION_MATCH is not None
VERSION = VERSION_MATCH.group(1)
CELLS = VERIFY.CELLS


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> bytes:
    data = (json.dumps(value, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return data


def make_fixture(root: Path) -> None:
    root.mkdir(parents=True)
    pre_push = b"pre-push: clean\n"
    (root / "pre-push-check.log").write_bytes(pre_push)
    sbom = b"{}\n"
    (root / f"agenterm-{VERSION}-sbom.spdx.json").write_bytes(sbom)

    cell_records = []
    loader_records = []
    l1_hashes = {}
    for platform, (cell, target) in CELLS.items():
        if platform.startswith("linux-"):
            names = ["agenterm", "agenterm-cu", "libagenterm.so", "agenterm-cu-provider.so"]
            artifact_root = root / "raw-linux" / platform
        else:
            os_name = platform.split("-", 1)[0]
            suffix = "-unsigned-preview" if os_name == "macos" else ""
            names = [f"agenterm-{VERSION}-{os_name}-{platform.rsplit('-', 1)[1]}{suffix}.zip"]
            artifact_root = root / f"candidate-part-{platform}"
        artifacts = []
        for name in names:
            content = f"{platform}:{name}".encode()
            artifact_root.mkdir(parents=True, exist_ok=True)
            (artifact_root / name).write_bytes(content)
            artifacts.append({"name": name, "bytes": len(content), "sha256": digest(content)})
            if not platform.startswith("linux-"):
                (artifact_root / f"{name}.sha256").write_text(
                    f"{digest(content)}  {name}\n", encoding="utf-8"
                )
                write_json(artifact_root / f"{name}.provenance.json", {
                    "artifact": name,
                    "source_commit": SOURCE,
                    "version": VERSION,
                    "sha256": digest(content),
                })
        cell_records.append({
            "platform_id": platform,
            "target": target,
            "state": "PASS",
            "artifacts": artifacts,
            "build_artifacts": [],
        })

        loader = f"thin-loader:{cell}".encode()
        loader_dir = root / "chassis-l1" / cell
        loader_dir.mkdir(parents=True, exist_ok=True)
        (loader_dir / "loader").write_bytes(loader)
        descriptor = {
            "schema": 1,
            "kind": "agenterm-chassis-l1-loader",
            "cell": cell,
            "version": VERSION,
            "source_sha": SOURCE,
            "bytes": len(loader),
            "sha256": digest(loader),
            "max_bytes": 2 * 1024 * 1024,
        }
        write_json(loader_dir / "loader.json", descriptor)
        loader_records.append({"cell": cell, "bytes": len(loader), "sha256": digest(loader)})
        l1_hashes[cell] = digest(loader)

    chassis_path = root / f"agenterm-{VERSION}-chassis-product.tgz"
    with tarfile.open(chassis_path, "w:gz") as archive:
        manifest_data = json.dumps({"l1_sha256": l1_hashes}, sort_keys=True).encode()
        for name, content in [("manifest.json", manifest_data)]:
            member = tarfile.TarInfo(name)
            member.size = len(content)
            archive.addfile(member, io.BytesIO(content))
        for platform, (cell, _) in CELLS.items():
            content = (root / "chassis-l1" / cell / "loader").read_bytes()
            member = tarfile.TarInfo(f"l1/{cell}/loader")
            member.size = len(content)
            archive.addfile(member, io.BytesIO(content))
    chassis_bytes = chassis_path.read_bytes()
    chassis_sha = digest(chassis_bytes)
    (root / f"{chassis_path.name}.sha256").write_text(
        f"{chassis_sha}  {chassis_path.name}\n", encoding="utf-8"
    )
    write_json(root / f"{chassis_path.name}.provenance.json", {
        "source_commit": SOURCE,
        "version": VERSION,
        "sha256": chassis_sha,
    })

    inputs = {
        "cargo_lock_sha256": "1" * 64,
        "artifact_manifest_sha256": "2" * 64,
        "release_policy_sha256": "3" * 64,
        "gate_manifest_sha256": "4" * 64,
        "sbom_sha256": digest(sbom),
    }
    pre_push_record = {
        "status": "passed",
        "name": "pre-push-check.log",
        "size": len(pre_push),
        "sha256": digest(pre_push),
    }
    manifest_bytes = write_json(root / "local-build-manifest.json", {
        "schema_version": 1,
        "kind": "agenterm-local-six-cell-build",
        "source_sha": SOURCE,
        "version": VERSION,
        "profile": "release",
        "pre_push_check": pre_push_record,
        "source_inputs": inputs,
        "cells": cell_records,
        "chassis_loaders": loader_records,
    })
    write_json(root / "qualification-receipt.json", {
        "schema_version": 2,
        "product": "AgenTerm",
        "profile": "prebuilt-six-cell-execute-only",
        "release": True,
        "stress_included": False,
        "source_sha": SOURCE,
        "version": VERSION,
        "release_policy_sha256": inputs["release_policy_sha256"],
        "pre_push_check": pre_push_record,
        "local_build_manifest": {
            "name": "local-build-manifest.json",
            "size": len(manifest_bytes),
            "sha256": digest(manifest_bytes),
        },
    })


def expect_rejected(label: str, call) -> None:
    try:
        call()
    except SystemExit:
        print(f"PASS negative control: {label}")
        return
    raise AssertionError(f"negative control was accepted: {label}")


def test_validate_and_negative_controls() -> None:
    with tempfile.TemporaryDirectory(prefix="verify-local-candidate-") as temporary:
        root = Path(temporary) / "candidate-input"
        make_fixture(root)
        summary = VERIFY.validate(root, SOURCE, VERSION)
        assert summary["source_sha"] == SOURCE and len(summary["cells"]) == 6

        linux_cell = root / "raw-linux/linux-x86_64/agenterm"
        original = linux_cell.read_bytes()
        linux_cell.write_bytes(original + b"x")
        expect_rejected("mutated Linux executable bytes", lambda: VERIFY.validate(root, SOURCE, VERSION))
        linux_cell.write_bytes(original)

        windows_provenance = root / f"candidate-part-windows-x86_64/agenterm-{VERSION}-windows-x86_64.zip.provenance.json"
        original_provenance = windows_provenance.read_bytes()
        changed_provenance = json.loads(original_provenance)
        changed_provenance["source_commit"] = ""
        write_json(windows_provenance, changed_provenance)
        expect_rejected("archive with missing source commit", lambda: VERIFY.validate(root, SOURCE, VERSION))
        windows_provenance.write_bytes(original_provenance)

        loader = root / "chassis-l1/win-x86_64/loader"
        original_loader = loader.read_bytes()
        loader.write_bytes(original_loader + b"x")
        expect_rejected("mutated Chassis loader bytes", lambda: VERIFY.validate(root, SOURCE, VERSION))
        loader.write_bytes(original_loader)

        expect_rejected("foreign source SHA", lambda: VERIFY.validate(root, "b" * 40, VERSION))


def archive_with_member(path: Path, name: str) -> None:
    with tarfile.open(path, "w:gz") as archive:
        member = tarfile.TarInfo(name)
        payload = b"x"
        member.size = len(payload)
        archive.addfile(member, io.BytesIO(payload))
    (path.with_name(path.name + ".sha256")).write_text(
        f"{digest(path.read_bytes())}  {path.name}\n", encoding="utf-8"
    )


def test_safe_unpack() -> None:
    with tempfile.TemporaryDirectory(prefix="verify-local-candidate-unpack-") as temporary:
        base = Path(temporary)
        valid = base / "valid.tar.gz"
        archive_with_member(valid, "candidate-input/file.txt")
        output = base / "out"
        output.mkdir()
        root = VERIFY.safe_unpack(valid, valid.with_name(valid.name + ".sha256"), output)
        assert (root / "file.txt").read_bytes() == b"x"

        unsafe = base / "unsafe.tar.gz"
        archive_with_member(unsafe, "candidate-input/../../escape")
        unsafe_out = base / "unsafe-out"
        unsafe_out.mkdir()
        expect_rejected(
            "archive path traversal",
            lambda: VERIFY.safe_unpack(unsafe, unsafe.with_name(unsafe.name + ".sha256"), unsafe_out),
        )

        bad_checksum = base / "bad-checksum.tar.gz"
        archive_with_member(bad_checksum, "candidate-input/file.txt")
        bad_checksum.with_name(bad_checksum.name + ".sha256").write_text(
            f"{'0' * 64}  {bad_checksum.name}\n", encoding="utf-8"
        )
        bad_out = base / "bad-out"
        bad_out.mkdir()
        expect_rejected(
            "modified staging bundle",
            lambda: VERIFY.safe_unpack(
                bad_checksum, bad_checksum.with_name(bad_checksum.name + ".sha256"), bad_out
            ),
        )


def test_command_line_entrypoint() -> None:
    with tempfile.TemporaryDirectory(prefix="verify-local-candidate-cli-") as temporary:
        base = Path(temporary)
        root = base / "candidate-input"
        make_fixture(root)
        archive_path = base / f"agenterm-{VERSION}-local-candidate-input.tar.gz"
        with tarfile.open(archive_path, "w:gz") as archive:
            for path in sorted(root.rglob("*")):
                if path.is_file():
                    archive.add(path, arcname=path.relative_to(base))
        checksum_path = archive_path.with_name(archive_path.name + ".sha256")
        checksum_path.write_text(
            f"{digest(archive_path.read_bytes())}  {archive_path.name}\n",
            encoding="utf-8",
        )
        output = base / "verified"
        result = subprocess.run(
            [
                sys.executable,
                str(Path(__file__).with_name("verify-local-candidate.py")),
                "--archive", str(archive_path),
                "--checksum", str(checksum_path),
                "--out", str(output),
                "--source-sha", SOURCE,
                "--version", VERSION,
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stderr
        summary = json.loads(result.stdout)
        assert summary["source_sha"] == SOURCE
        assert len(summary["cells"]) == 6
        assert (output / "candidate-input/local-build-manifest.json").is_file()


if __name__ == "__main__":
    test_validate_and_negative_controls()
    test_safe_unpack()
    test_command_line_entrypoint()
    print("verify-local-candidate: PASS")
