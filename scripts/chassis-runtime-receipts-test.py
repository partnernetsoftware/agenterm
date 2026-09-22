#!/usr/bin/env python3
"""Owning test for scripts/chassis-runtime-receipts.py."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
RUN = "4242"
SHA = "a" * 40
TGZ = "b" * 64
CELLS = {
    "windows-x86_64": "win-x86_64",
    "windows-aarch64": "win-aarch64",
    "linux-x86_64": "lnx-x86_64",
    "linux-aarch64": "lnx-aarch64",
    "macos-x86_64": "osx-x86_64",
    "macos-aarch64": "osx-aarch64",
}


def receipt(platform: str, attempt: int, **override: object) -> dict:
    value = {
        "schema": 1,
        "platform_id": platform,
        "cell": CELLS[platform],
        "expected_cell": CELLS[platform],
        "run_id": RUN,
        "run_attempt": str(attempt),
        "source_sha": SHA,
        "tgz_sha256": TGZ,
        "image_id": "c" * 64,
        "active_tab_id": "@1",
        "expected_active_tab_id": "@1",
        "passed": True,
    }
    value.update(override)
    return value


def write(root: Path, platform: str, attempt: int, value: dict, run: str = RUN) -> None:
    directory = root / f"candidate-chassis-runtime-{platform}-{run}-{attempt}"
    directory.mkdir(parents=True)
    (directory / "receipt.json").write_text(json.dumps(value), encoding="utf-8")


def run(root: Path, attempt: int) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(REPO / "scripts/chassis-runtime-receipts.py"),
            "--receipts-dir", str(root),
            "--run-id", RUN,
            "--run-attempt", str(attempt),
            "--source-sha", SHA,
            "--tgz-sha256", TGZ,
        ],
        check=False, capture_output=True, text=True,
    )


def full(root: Path, attempt: int = 1) -> Path:
    for platform in CELLS:
        write(root, platform, attempt, receipt(platform, attempt))
    return root


def expect_rejected(root: Path, attempt: int, label: str, message: str) -> None:
    result = run(root, attempt)
    if result.returncode == 0 or message not in result.stderr:
        raise SystemExit(f"{label} was not rejected as {message!r}\n{result.stdout}{result.stderr}")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="chassis-receipts-test-") as raw:
        base = Path(raw)

        passed = run(full(base / "ok"), 1)
        if passed.returncode != 0:
            raise SystemExit(f"six good receipts were rejected\n{passed.stderr}")

        # A rerun of one failed cell: five cells stay at attempt 1, the rerun
        # cell has a failed attempt 1 and a passing attempt 2. The consumer
        # runs at attempt 2 and must take each cell's own highest attempt.
        mixed = full(base / "mixed")
        shutil.rmtree(mixed / f"candidate-chassis-runtime-windows-x86_64-{RUN}-1")
        write(mixed, "windows-x86_64", 1, receipt("windows-x86_64", 1, passed=False))
        write(mixed, "windows-x86_64", 2, receipt("windows-x86_64", 2))
        mixed_result = run(mixed, 2)
        if mixed_result.returncode != 0:
            raise SystemExit(f"mixed attempts were rejected\n{mixed_result.stderr}")
        chosen = json.loads(mixed_result.stdout)["cells"]
        if chosen["windows-x86_64"]["attempt"] != 2 or chosen["linux-x86_64"]["attempt"] != 1:
            raise SystemExit(f"mixed attempts chose the wrong receipts: {chosen}")

        # Negative control for the old shape: looking only for the consumer's
        # own attempt finds one receipt, not six.
        only_own = base / "only-own"
        write(only_own, "windows-x86_64", 2, receipt("windows-x86_64", 2))
        expect_rejected(only_own, 2, "consumer-attempt-only", "missing installed-product receipts")

        missing = full(base / "missing")
        shutil.rmtree(missing / f"candidate-chassis-runtime-macos-x86_64-{RUN}-1")
        expect_rejected(missing, 1, "missing-cell", "missing installed-product receipts")

        for label, key, value, message in (
            ("wrong-tgz", "tgz_sha256", "d" * 64, "tgz_sha256"),
            ("wrong-source", "source_sha", "e" * 40, "source_sha"),
            ("wrong-cell", "cell", "win-x86_64", "cell"),
            ("wrong-attempt-field", "run_attempt", "2", "run_attempt"),
            ("failed-journey", "passed", False, "passed"),
            ("wrong-tab", "active_tab_id", "@3", "expected tab"),
        ):
            root = full(base / label)
            target = root / f"candidate-chassis-runtime-linux-aarch64-{RUN}-1"
            shutil.rmtree(target)
            write(root, "linux-aarch64", 1, receipt("linux-aarch64", 1, **{key: value}))
            expect_rejected(root, 1, label, message)

        foreign = full(base / "foreign-run")
        write(foreign, "linux-x86_64", 1, receipt("linux-x86_64", 1), run="9999")
        expect_rejected(foreign, 1, "foreign-run", "another run")

        future = full(base / "future-attempt")
        write(future, "linux-x86_64", 3, receipt("linux-x86_64", 3))
        expect_rejected(future, 2, "future-attempt", "out of range")

    print("PASS: installed-product receipts bind six cells, mixed attempts and one tgz")


if __name__ == "__main__":
    main()
