"""The product L3 app contract, shared by both packers and the installer.

A product image carries exactly one product app, `l3/app.json`; any other
L3 file (such as `example-app.json`) is never read as the app. The app must
declare every capability its bundled L2 programs call, and every capability
it declares must exist in the bundled L2 Host ABI. Declaring more than the
programs use is allowed: an app may legitimately need a capability no
bundled program exercises. Every violation fails closed.
"""

from __future__ import annotations

import json
from pathlib import Path

APP = "l3/app.json"


class L3AppError(ValueError):
    pass


def check_contract(app: object, host_abi: object, programs: dict[str, object]) -> dict:
    """Validate parsed JSON; `programs` maps a program's path to its value."""
    if not isinstance(app, dict) or app.get("schema") != 1:
        raise L3AppError(f"{APP} must be a schema 1 object")
    name = app.get("name")
    if not isinstance(name, str) or not name:
        raise L3AppError(f"{APP} must name the app")
    declared = app.get("capabilities")
    if not isinstance(declared, list) or not all(isinstance(cap, str) for cap in declared):
        raise L3AppError(f"{APP} capabilities must be a list of strings")
    if len(set(declared)) != len(declared):
        raise L3AppError(f"{APP} declares a capability twice")
    if not isinstance(host_abi, dict) or not isinstance(host_abi.get("capabilities"), list):
        raise L3AppError("l2/host-abi.json has no capability list")
    known = {cap.get("id") for cap in host_abi["capabilities"] if isinstance(cap, dict)}
    unknown = sorted(set(declared) - known)
    if unknown:
        raise L3AppError(f"{APP} declares capabilities absent from the L2 Host ABI: {unknown}")
    needed: set[str] = set()
    for path, program in sorted(programs.items()):
        caps = program.get("caps") if isinstance(program, dict) else None
        if not isinstance(caps, list) or not all(isinstance(cap, str) for cap in caps):
            raise L3AppError(f"{path} has no caps list")
        needed.update(caps)
    missing = sorted(needed - set(declared))
    if missing:
        raise L3AppError(f"{APP} lacks capabilities its L2 programs call: {missing}")
    return app


def validate_layout(root: Path) -> dict:
    """Validate the contract for a composed layout or installed image."""
    app_path = root / APP
    if not app_path.is_file():
        raise L3AppError(f"missing {APP}: the product image has no L3 app")

    def load(path: Path) -> object:
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise L3AppError(f"cannot read {path.relative_to(root).as_posix()}: {error}") from None

    programs = {
        path.relative_to(root).as_posix(): load(path)
        for path in sorted((root / "l2" / "programs").glob("*.json"))
    }
    return check_contract(load(app_path), load(root / "l2" / "host-abi.json"), programs)
