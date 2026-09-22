#!/usr/bin/env python3
"""Bind six installed-product runtime receipts to one sealed chassis tgz.

Each runtime cell uploads `candidate-chassis-runtime-<platform>-<run>-<attempt>`
holding `receipt.json`. Download every such artifact of this run into its own
directory (never merged) and pass the parent directory here. A rerun of failed
cells leaves the cells that passed at their earlier attempt, so the receipt
for each cell is the one from its highest attempt present in this run, never
one named from the consumer's own attempt. Every cell must be present and its
receipt must name this run, its own attempt, its cell, the Candidate source
SHA and the exact tgz, and must have passed the journey.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

PLATFORM_CELLS = {
    "windows-x86_64": "win-x86_64",
    "windows-aarch64": "win-aarch64",
    "linux-x86_64": "lnx-x86_64",
    "linux-aarch64": "lnx-aarch64",
    "macos-x86_64": "osx-x86_64",
    "macos-aarch64": "osx-aarch64",
}
NAME = re.compile(r"^candidate-chassis-runtime-(?P<platform>[a-z0-9_-]+?)-(?P<run>\d+)-(?P<attempt>\d+)$")


class ReceiptError(ValueError):
    pass


def select(receipts_dir: Path, run_id: str, run_attempt: int) -> dict[str, tuple[int, Path]]:
    """The highest-attempt receipt directory of this run for each platform."""
    chosen: dict[str, tuple[int, Path]] = {}
    for entry in sorted(receipts_dir.iterdir()):
        match = NAME.match(entry.name)
        if not entry.is_dir() or match is None:
            raise ReceiptError(f"unexpected receipt artifact: {entry.name}")
        if match["run"] != run_id:
            raise ReceiptError(f"receipt from another run: {entry.name}")
        attempt = int(match["attempt"])
        if attempt < 1 or attempt > run_attempt:
            raise ReceiptError(f"receipt attempt out of range: {entry.name}")
        platform = match["platform"]
        if platform not in PLATFORM_CELLS:
            raise ReceiptError(f"receipt for an unknown platform: {entry.name}")
        if platform not in chosen or attempt > chosen[platform][0]:
            chosen[platform] = (attempt, entry)
    return chosen


def verify(receipts_dir: Path, run_id: str, run_attempt: int, source_sha: str, tgz_sha256: str) -> dict:
    chosen = select(receipts_dir, run_id, run_attempt)
    missing = sorted(set(PLATFORM_CELLS) - set(chosen))
    if missing:
        raise ReceiptError(f"missing installed-product receipts: {missing}")
    cells = {}
    for platform, (attempt, directory) in sorted(chosen.items()):
        path = directory / "receipt.json"
        try:
            receipt = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ReceiptError(f"{directory.name}: unreadable receipt: {error}") from None
        expected = {
            "platform_id": platform,
            "cell": PLATFORM_CELLS[platform],
            "expected_cell": PLATFORM_CELLS[platform],
            "run_id": run_id,
            "run_attempt": str(attempt),
            "source_sha": source_sha,
            "tgz_sha256": tgz_sha256,
            "passed": True,
        }
        for key, value in expected.items():
            if receipt.get(key) != value:
                raise ReceiptError(
                    f"{directory.name}: {key} is {receipt.get(key)!r}, expected {value!r}"
                )
        if receipt.get("active_tab_id") != receipt.get("expected_active_tab_id"):
            raise ReceiptError(f"{directory.name}: the journey did not reach the expected tab")
        cells[platform] = {
            "attempt": attempt,
            "cell": receipt["cell"],
            "image_id": receipt.get("image_id"),
            "active_tab_id": receipt["active_tab_id"],
        }
    return {"schema": 1, "run_id": run_id, "tgz_sha256": tgz_sha256, "cells": cells}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipts-dir", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True, type=int)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--tgz-sha256", required=True)
    args = parser.parse_args()
    try:
        summary = verify(
            Path(args.receipts_dir), args.run_id, args.run_attempt, args.source_sha, args.tgz_sha256
        )
    except ReceiptError as error:
        sys.exit(f"chassis runtime receipts rejected: {error}")
    print(json.dumps(summary, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
