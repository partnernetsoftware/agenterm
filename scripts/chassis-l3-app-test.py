#!/usr/bin/env python3
"""Owning test for the product L3 app contract (scripts/chassis_l3_app.py)."""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from chassis_l3_app import L3AppError, validate_layout  # noqa: E402


def layout(repo: Path, root: Path) -> Path:
    shutil.copytree(repo / "crates/agenterm-chassis/l2", root / "l2")
    shutil.copytree(repo / "crates/agenterm-chassis/l3", root / "l3")
    return root


def expect_refused(root: Path, label: str, message: str) -> None:
    try:
        validate_layout(root)
    except L3AppError as error:
        if message not in str(error):
            raise SystemExit(f"{label}: refused for the wrong reason: {error}")
        return
    raise SystemExit(f"{label}: accepted")


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="chassis-l3-app-test-") as raw:
        base = Path(raw)
        app = validate_layout(layout(repo, base / "repo"))
        if app["name"] != "agenterm.workbench":
            raise SystemExit("the repository product app is not agenterm.workbench")

        missing = layout(repo, base / "missing")
        (missing / "l3/app.json").unlink()
        expect_refused(missing, "missing app", "missing l3/app.json")

        uncovered = layout(repo, base / "uncovered")
        value = json.loads((uncovered / "l3/app.json").read_text(encoding="utf-8"))
        value["capabilities"].remove("tabs.step")
        (uncovered / "l3/app.json").write_text(json.dumps(value), encoding="utf-8")
        expect_refused(uncovered, "uncovered cap", "lacks capabilities its L2 programs call")

        unknown = layout(repo, base / "unknown")
        value = json.loads((unknown / "l3/app.json").read_text(encoding="utf-8"))
        value["capabilities"].append("no.such.cap")
        (unknown / "l3/app.json").write_text(json.dumps(value), encoding="utf-8")
        expect_refused(unknown, "unknown cap", "absent from the L2 Host ABI")

        # Declaring more than the programs call is allowed when the ABI has it.
        wider = layout(repo, base / "wider")
        value = json.loads((wider / "l3/app.json").read_text(encoding="utf-8"))
        value["capabilities"].append("tabs.list")
        (wider / "l3/app.json").write_text(json.dumps(value), encoding="utf-8")
        validate_layout(wider)

    # Every product path enforces the same contract.
    for script in ("chassis-candidate-pack.py", "chassis-ci-pack.py", "chassis-install-product.py"):
        text = (repo / "scripts" / script).read_text(encoding="utf-8")
        if "validate_layout(" not in text:
            raise SystemExit(f"{script} does not enforce the L3 app contract")
    print("PASS: product L3 app contract is enforced and fail closed")


if __name__ == "__main__":
    main()
