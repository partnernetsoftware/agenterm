#!/usr/bin/env python3
"""Compile agenterm-platform with MiniCon's exact target feature unions.

MiniCon's Cargo.toml is the consumer source of truth. Its alignment test pins
the five dependency blocks and asks for a seam handoff when they change; keep
BASE and target additions here synchronized with that handoff. Cross-target
checks prove compilation only. `--test-native` executes tests on this host.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

BASE = (
    "clipboard",
    "entropy",
    "filesystem-publish",
    "filesystem-read",
    "font",
    "ime",
    "input",
    "ipc",
    "parent-console",
    "pty",
    "runtime",
    "screenshot",
    "window",
)
TARGETS = (
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)


def features(target: str, dev: bool) -> tuple[str, ...]:
    added = ("native-pixel-window",) if "windows" in target else ("portable-pixel-window",)
    # Windows has a separate dev-dependency block for `runtime`, already in
    # BASE. Cargo unifies it without adding another effective feature.
    return tuple(dict.fromkeys((*BASE, *added, *(("input-inject",) if dev else ()))))


def native_target() -> str:
    report = subprocess.run(
        ["rustc", "-vV"], check=True, capture_output=True, text=True
    ).stdout
    for line in report.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise SystemExit("rustc did not report a host target")


def run(repo: Path, target: str, *, dev: bool, test: bool) -> None:
    cargo = ["cargo"]
    if "windows" in target and os.name != "nt":
        cargo.append("xwin")
    cargo += [
        "test" if test else "check",
        "--quiet",
        "--locked",
        "-p",
        "agenterm-platform",
        "--lib",
        "--target",
        target,
        "--no-default-features",
        "--features",
        ",".join(features(target, dev)),
    ]
    label = f"{target} {'dev' if dev else 'normal'} {'test' if test else 'check'}"
    print(f"MiniCon consumer: {label}", flush=True)
    result = subprocess.run(cargo, cwd=repo, check=False)
    if result.returncode:
        raise SystemExit(f"MiniCon consumer {label} failed with exit {result.returncode}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--target", choices=TARGETS)
    selection.add_argument("--all", action="store_true")
    parser.add_argument("--test-native", action="store_true")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    os.environ.setdefault("CARGO_TARGET_DIR", str(repo / "target/minicon-consumer-matrix"))
    targets = TARGETS if args.all else (args.target,)
    native = native_target() if args.test_native else None
    for target in targets:
        for dev in (False, True):
            run(repo, target, dev=dev, test=args.test_native and target == native)
    if args.test_native and native not in targets:
        raise SystemExit(f"native target {native} was not selected for execution")


if __name__ == "__main__":
    main()
