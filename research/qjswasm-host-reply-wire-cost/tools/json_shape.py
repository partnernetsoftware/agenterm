#!/usr/bin/env python3
"""Token-preserving JSON shaping for the host-reply wire cost experiment.

The three shapes differ only in bytes:

  A  the reply byte-for-byte as the guest receives it today;
  B  the same reply *value* with insignificant whitespace outside string
     literals removed (compact emission);
  C  only the field paths the journey's census says it reads, every retained
     value carrying its exact original spelling.

`Scalar.raw` keeps the original token text (numbers, `true`, `false`, `null`,
string literals with their original escapes), so shape C can never re-spell a
value. `emit` renders a node compactly, which reproduces serde_json's compact
output for a reply that was already compact -- `make_shapes.py` asserts that on
every envelope it rewrites.
"""

from __future__ import annotations

LEAF = True


class Scalar:
    __slots__ = ("raw",)

    def __init__(self, raw: str) -> None:
        self.raw = raw


class Array:
    __slots__ = ("items",)

    def __init__(self, items: list) -> None:
        self.items = items


class Object:
    __slots__ = ("members",)

    def __init__(self, members: list[tuple[str, object]]) -> None:
        # members keep the document's own key order; each key is its original
        # quoted token, so key spelling (escapes included) survives too.
        self.members = members


def _skip_ws(text: str, i: int) -> int:
    while i < len(text) and text[i] in " \t\r\n":
        i += 1
    return i


def _scan_string(text: str, i: int) -> int:
    assert text[i] == '"'
    i += 1
    while True:
        c = text[i]
        if c == "\\":
            i += 2
            continue
        if c == '"':
            return i + 1
        i += 1


def _parse(text: str, i: int) -> tuple[object, int]:
    i = _skip_ws(text, i)
    c = text[i]
    if c == "{":
        members: list[tuple[str, object]] = []
        i = _skip_ws(text, i + 1)
        if text[i] == "}":
            return Object(members), i + 1
        while True:
            i = _skip_ws(text, i)
            key_end = _scan_string(text, i)
            key = text[i:key_end]
            i = _skip_ws(text, key_end)
            assert text[i] == ":"
            value, i = _parse(text, i + 1)
            members.append((key, value))
            i = _skip_ws(text, i)
            if text[i] == ",":
                i += 1
                continue
            assert text[i] == "}"
            return Object(members), i + 1
    if c == "[":
        items: list[object] = []
        i = _skip_ws(text, i + 1)
        if text[i] == "]":
            return Array(items), i + 1
        while True:
            value, i = _parse(text, i)
            items.append(value)
            i = _skip_ws(text, i)
            if text[i] == ",":
                i += 1
                continue
            assert text[i] == "]"
            return Array(items), i + 1
    if c == '"':
        end = _scan_string(text, i)
        return Scalar(text[i:end]), end
    start = i
    while i < len(text) and text[i] not in ",]} \t\r\n":
        i += 1
    return Scalar(text[start:i]), i


def parse(text: str) -> object:
    node, end = _parse(text, 0)
    end = _skip_ws(text, end)
    if end != len(text):
        raise ValueError(f"trailing bytes after JSON value at {end}")
    return node


def emit(node: object) -> str:
    """Compact emission: no insignificant whitespace, scalars verbatim."""
    if isinstance(node, Scalar):
        return node.raw
    if isinstance(node, Array):
        return "[" + ",".join(emit(item) for item in node.items) + "]"
    return "{" + ",".join(f"{key}:{emit(value)}" for key, value in node.members) + "}"


def escape(text: str) -> str:
    """serde_json's string escaping: the five short forms, `\\u00xx` below
    0x20, and everything else (including non-ASCII) passed through."""
    out = ['"']
    for ch in text:
        code = ord(ch)
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\b":
            out.append("\\b")
        elif ch == "\f":
            out.append("\\f")
        elif code < 0x20:
            out.append("\\u%04x" % code)
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def spec_from_paths(paths: list[str]):
    """Turn `["a.b", "tabs[].id", "[].endpoint"]` into a nested allow-tree.

    A leaf is `True` (keep the whole value). `[]` marks array-element traversal:
    it applies to the array held by the segment it is attached to, and every
    following segment applies to each element. A path that resolves to nothing
    in the reply simply retains nothing.
    """
    root: dict = {}
    for path in paths:
        if path == "":
            return LEAF  # a reply read whole
        parts = path.split(".")
        node: object = root
        for index, part in enumerate(parts):
            last = index == len(parts) - 1
            array = part.endswith("[]")
            key = part[:-2] if array else part
            if array and key == "":
                # the value here is an array; the rest of the path reads its elements
                if last:
                    node["[]"] = LEAF
                    break
                child = node.setdefault("[]", {})
                if child is LEAF:
                    break
                node = child
                continue
            if last:
                if node.get(key) is not LEAF:
                    node[key] = LEAF
                break
            child = node.setdefault(key, {})
            if child is LEAF:
                break  # a shallower path already keeps this subtree whole
            node = child
            if array:
                element = node.setdefault("[]", {})
                if element is LEAF:
                    break
                node = element
    return root


def project(node: object, spec: object) -> object:
    """Keep only the spec's paths; a LEAF spec keeps the whole subtree."""
    if spec is LEAF:
        return node
    if isinstance(node, Object):
        if not isinstance(spec, dict):
            return node
        members = []
        for key, value in node.members:
            bare = key[1:-1]
            sub = spec.get(bare, spec.get(_unescape(bare)))
            if sub is None:
                continue
            members.append((key, project(value, sub)))
        return Object(members)
    if isinstance(node, Array):
        if not isinstance(spec, dict) or "[]" not in spec:
            return node
        sub = spec["[]"]
        return Array([project(item, sub) for item in node.items])
    return node


def _unescape(key: str) -> str:
    """Decode a JSON object key for comparison with a census path segment."""
    try:
        import json

        return json.loads('"' + key + '"')
    except Exception:
        return key


def set_member(node: Object, name: str, value: object) -> Object:
    """A copy of `node` with one top-level member replaced (order kept)."""
    members = []
    for key, item in node.members:
        if _unescape(key[1:-1]) == name:
            members.append((key, value))
        else:
            members.append((key, item))
    return Object(members)
