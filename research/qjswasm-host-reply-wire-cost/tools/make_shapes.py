#!/usr/bin/env python3
"""Build the three frozen reply shapes A / B / C and validate V3 and V4.

Reads  research/qjswasm-host-reply-wire-cost/census.json
       research/qjswasm-host-reply-wire-cost/replies/<journey>/{index.jsonl,env-*.txt,pay-*.txt}
Writes research/qjswasm-host-reply-wire-cost/shapes/<journey>/<A|B|C>/{index.json,env-*.txt,pay-*.txt}
       research/qjswasm-host-reply-wire-cost/shapes/manifest.json

A  verbatim capture.
B  the same value, insignificant whitespace outside string literals removed.
C  only the census paths, values carrying their exact original spelling.

A reply whose payload travelled inside the door envelope has no pay file: its
envelope is re-emitted with the shaped payload in place, so the escaping the
guest pays for is recomputed for the shaped bytes and nothing else moves.
"""

from __future__ import annotations

import hashlib
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE / "tools"))

import json_shape as js  # noqa: E402


def name(seq: int) -> str:
    return f"{seq:04d}"


def wrap(text: str, form: str) -> str:
    """The text the guest hands to JSON.parse. For a grep-filtered fragment the
    journey wraps the lines in braces (and drops the trailing comma) first."""
    if form != "fragment":
        return text
    body = text.strip()
    if body.endswith(","):
        body = body[:-1]
    return "{" + body + "}"


def read_paths(value, paths: list[str]) -> list:
    """The census read sequence over a parsed value: dotted paths, `[]` walks
    every element of an array at that point."""
    out = []
    for path in paths:
        out.extend(_read_one(value, path.split(".")))
    return out


def _read_one(node, parts: list[str]):
    if not parts:
        return [node]
    head, rest = parts[0], parts[1:]
    if head.endswith("[]"):
        key = head[:-2]
        if key == "":
            container = node
        else:
            if not isinstance(node, dict) or key not in node:
                return []
            container = node[key]
        if not isinstance(container, list):
            return []
        out = []
        for item in container:
            out.extend(_read_one(item, rest))
        return out
    if not isinstance(node, dict) or head not in node:
        return []
    return _read_one(node[head], rest)


def main() -> int:
    census = json.loads((HERE / "census.json").read_text())
    shapes_root = HERE / "shapes"
    manifest = {"shapes": {}, "checks": {}, "bytes": {}}
    problems: list[str] = []
    for journey, spec in census["journeys"].items():
        replies = spec["replies"]
        index_lines = [
            json.loads(line)
            for line in (HERE / "replies" / journey / "index.jsonl").read_text().splitlines()
            if line.strip()
        ]
        assert len(index_lines) == len(replies), f"{journey}: index/census length mismatch"
        rows = []
        totals = {}
        for record, captured in zip(replies, index_lines):
            assert record["seq"] == captured["seq"], f"{journey}: seq mismatch"
            if record.get("command") and captured.get("arguments"):
                command = " ".join(str(a) for a in captured["arguments"])
                if not all(token in command for token in record["command"].split(" ")):
                    problems.append(f"{journey} seq {record['seq']}: command drift {command!r}")
            seq = name(record["seq"])
            base = HERE / "replies" / journey
            env_a = (base / f"env-{seq}.txt").read_text()
            pay_a = (base / f"pay-{seq}.txt").read_text() if (base / f"pay-{seq}.txt").exists() else None
            in_envelope = captured["source"] == "envelope"
            if in_envelope:
                assert pay_a is None, f"{journey} seq {seq}: envelope source has a pay file"
                pay_a = json.loads(env_a)["stdout"]
            else:
                assert pay_a is not None, f"{journey} seq {seq}: file source without a pay file"
                assert json.loads(env_a).get("stdout") == "", f"{journey} seq {seq}: envelope carries a payload"
            parse_as = record.get("parse", "text")
            paths = record.get("reads", [])
            if parse_as != "json":
                paths = []  # a non-JSON reply is read whole: no byte is unread

            # ---- encoder parity: our compact emission must reproduce bytes
            if js.emit(js.parse(env_a)) != env_a:
                problems.append(f"{journey} seq {seq}: envelope is not compact-emittable (parity)")
            if in_envelope:
                original = json.loads(env_a)["stdout"]
                if json.loads("{" + '"s":' + js.escape(original) + "}")["s"] != original:
                    problems.append(f"{journey} seq {seq}: escape round-trip failed")
                # the host's own escaping, reproduced: the token we would write
                # for the unchanged payload must equal the capture's token
                env_node = js.parse(env_a)
                token = None
                for key, value in env_node.members:
                    if js._unescape(key[1:-1]) == "stdout":
                        token = value.raw
                if token != js.escape(original):
                    problems.append(f"{journey} seq {seq}: envelope escaping differs from the capture")

            # ---- shape B / C payloads
            form = record.get("form", "object")
            body = pay_a.strip()
            if form == "fragment" and body.endswith(","):
                body = body[:-1]
            wrapped = "{" + body + "}" if form == "fragment" else pay_a
            node_a = None
            value_a = None
            payload_is_json = False
            if wrapped.strip() != "":
                try:
                    node_a = js.parse(wrapped)
                    value_a = json.loads(wrapped)
                    payload_is_json = True
                except (ValueError, IndexError):
                    payload_is_json = False
            if payload_is_json:
                # B is a property of the reply text; C is the census over what
                # the journey reads (an unparsed reply has no read field).
                node_b = node_a
                spec_tree = js.spec_from_paths(paths) if parse_as == "json" else {}
                node_c = js.project(node_a, spec_tree)
                pay_b = js.emit(node_b)
                pay_c = js.emit(node_c)
                if form == "fragment":
                    # the wire carries the fragment, not the braces the guest adds
                    pay_b = pay_b[1:-1]
                    pay_c = pay_c[1:-1]
            else:
                # an empty or non-JSON answer is read whole by the journey
                pay_b = pay_c = pay_a

            # ---- V4: B parses to the same value as A
            if payload_is_json:
                if json.loads(wrap(pay_b, form)) != value_a:
                    problems.append(f"{journey} seq {seq}: V4 B != A")
            # ---- V3: C agrees with A at every census path, and the read
            # sequence yields byte-identical values under both
            if parse_as == "json":
                value_c = json.loads(wrap(pay_c, form))
                read_a = read_paths(value_a, paths)
                read_c = read_paths(value_c, paths)
                if len(read_a) != len(read_c):
                    problems.append(f"{journey} seq {seq}: V3 read count differs")
                for left, right in zip(read_a, read_c):
                    if left != right:
                        problems.append(f"{journey} seq {seq}: V3 {paths} {left!r} != {right!r}")
                resolved = [path for path in paths if read_paths(value_a, [path])]
                unresolved = [path for path in paths if not read_paths(value_a, [path])]
                if paths and not resolved:
                    # every census path of a JSON reply resolving to nothing is
                    # the shape of a wrong root (e.g. a root array addressed as
                    # an object member); a single absent optional field is fine
                    problems.append(
                        f"{journey} seq {seq}: no census path resolves in the reply (root mismatch)"
                    )

            # ---- envelopes
            if in_envelope:
                env_node = js.parse(env_a)
                payload_holder = "stdout"
                env_b = js.emit(js.set_member(env_node, payload_holder, js.Scalar(js.escape(pay_b))))
                env_c = js.emit(js.set_member(env_node, payload_holder, js.Scalar(js.escape(pay_c))))
            else:
                env_b = env_c = env_a
            for shape, env_text, pay_text, write_pay in (
                ("A", env_a, pay_a, not in_envelope),
                ("B", env_b, pay_b, not in_envelope),
                ("C", env_c, pay_c, not in_envelope),
            ):
                out = shapes_root / journey / shape
                out.mkdir(parents=True, exist_ok=True)
                (out / f"env-{seq}.txt").write_text(env_text)
                if write_pay:
                    (out / f"pay-{seq}.txt").write_text(pay_text)
                bucket = totals.setdefault(shape, {"envelope": 0, "payload": 0})
                bucket["envelope"] += len(env_text.encode())
                bucket["payload"] += len(pay_text.encode())
            rows.append(
                {
                    "seq": record["seq"],
                    "arguments": captured["arguments"],
                    "source": captured["source"],
                    "parse": parse_as,
                    "form": form,
                    "trim": record.get("trim", False),
                    "reads": paths,
                    "unresolved_reads": (
                        [path for path in paths if not read_paths(value_a, [path])]
                        if parse_as == "json" and value_a is not None
                        else []
                    ),
                    "envelope_bytes": len(env_a.encode()),
                    "payload_bytes": len(pay_a.encode()),
                }
            )
        index_out = shapes_root / journey
        (index_out / "index.json").write_text(
            json.dumps({"journey": journey, "replies": rows}, indent=1, ensure_ascii=False) + "\n"
        )
        manifest["bytes"][journey] = totals
        manifest["shapes"][journey] = {}
        for shape in ("A", "B", "C"):
            digests = {}
            for path in sorted((shapes_root / journey / shape).glob("*.txt")):
                digests[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
            manifest["shapes"][journey][shape] = digests
        print(f"{journey}: {len(rows)} replies; bytes {json.dumps(totals)}")
    manifest["checks"] = {"problems": problems}
    (shapes_root / "manifest.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")
    if problems:
        for problem in problems:
            print(f"FAIL {problem}", file=sys.stderr)
        return 1
    print("V3/V4 and encoder-parity checks: all passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
