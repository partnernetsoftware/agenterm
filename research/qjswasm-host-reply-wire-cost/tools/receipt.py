#!/usr/bin/env python3
"""Independent reference values for the receipt.

Two digests per (journey, shape), so the shapes cannot all be wrong together:

  reply_set_sha256  over every frozen shape byte, in file-name order
  value_set_sha256  over the canonical JSON value of every parsed reply, so B
                    and C can be shown value-equal (or not) to A independently
                    of spelling, whitespace and byte count

  python3 research/qjswasm-host-reply-wire-cost/tools/receipt.py
"""

from __future__ import annotations

import hashlib
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE / "tools"))

import json_shape as js  # noqa: E402
import make_shapes as ms  # noqa: E402


def main() -> int:
    shapes = HERE / "shapes"
    manifest = json.loads((shapes / "manifest.json").read_text())
    receipt = {"shapes": {}, "bytes": manifest["bytes"]}
    for journey in sorted(manifest["shapes"]):
        index = json.loads((shapes / journey / "index.json").read_text())
        receipt["shapes"][journey] = {}
        for shape in ("A", "B", "C"):
            directory = shapes / journey / shape
            blobs = b"".join(
                path.name.encode() + b"\0" + path.read_bytes()
                for path in sorted(directory.glob("*.txt"))
            )
            values = []
            for reply in index["replies"]:
                name = f"{reply['seq']:04d}"
                if reply["parse"] != "json" or reply["form"] == "text":
                    values.append(None)
                    continue
                env = json.loads((directory / f"env-{name}.txt").read_text())
                payload = (
                    env["stdout"]
                    if reply["source"] == "envelope"
                    else (directory / f"pay-{name}.txt").read_text()
                )
                values.append(json.loads(ms.wrap(payload, reply.get("form", "object"))))
            canonical = json.dumps(values, sort_keys=True, separators=(",", ":")).encode()
            receipt["shapes"][journey][shape] = {
                "reply_set_sha256": hashlib.sha256(blobs).hexdigest(),
                "value_set_sha256": hashlib.sha256(canonical).hexdigest(),
                "files": len(list(directory.glob("*.txt"))),
            }
        a = receipt["shapes"][journey]["A"]["value_set_sha256"]
        for shape in ("B", "C"):
            same = receipt["shapes"][journey][shape]["value_set_sha256"] == a
            print(f"{journey:<19}{shape} value-set identical to A: {same}")
    (HERE / "receipt.json").write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
