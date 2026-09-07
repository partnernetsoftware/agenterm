#!/usr/bin/env python3
"""Bounded two-connection B4 court driver for the fixed Linux broker.

This is native qualification infrastructure, not a second product client. It
builds the same closed provider request from a public privilege-plan reply and
opens two simultaneous connections only to the fixed production socket.
"""

from __future__ import annotations

import argparse
import base64
import json
import socket
import struct
import threading

SOCKET_PATH = "/run/agenterm/cu-privilege.sock"
MAX_REPLY_BYTES = 16 * 1024


def read_exact(stream: socket.socket, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise RuntimeError("broker reply ended before the declared frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def round_trip(payload: bytes, ready: threading.Barrier) -> dict[str, object]:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(30)
        stream.connect(SOCKET_PATH)
        ready.wait(timeout=10)
        stream.sendall(struct.pack(">I", len(payload)) + payload)
        stream.shutdown(socket.SHUT_WR)
        length = struct.unpack(">I", read_exact(stream, 4))[0]
        if length == 0 or length > MAX_REPLY_BYTES:
            raise RuntimeError("broker reply exceeded its fixed frame ceiling")
        reply = json.loads(read_exact(stream, length))
        if stream.recv(1) != b"":
            raise RuntimeError("broker returned trailing bytes")
        return reply


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("plan_reply")
    parser.add_argument("request_id")
    args = parser.parse_args()

    with open(args.plan_reply, encoding="utf-8") as source:
        planned = json.load(source)["data"]
    encoded = planned["request"]
    encoded += "=" * (-len(encoded) % 4)
    plan = json.loads(base64.urlsafe_b64decode(encoded))
    request = {
        "protocol_version": 1,
        "request_id": args.request_id,
        "plan": plan,
        "authorization": {"mode": "one_shot_native_consent"},
        "origin": {"session_id": "native-b4-court", "target_scope": "current"},
        "client": {"contract_version": 1},
    }
    payload = json.dumps(request, separators=(",", ":")).encode()
    if not payload or len(payload) > 64 * 1024:
        raise RuntimeError("provider request exceeded its fixed frame ceiling")

    ready = threading.Barrier(2)
    replies: list[dict[str, object] | None] = [None, None]
    failures: list[BaseException] = []

    def worker(index: int) -> None:
        try:
            replies[index] = round_trip(payload, ready)
        except BaseException as error:  # preserve the first exact court failure
            failures.append(error)

    threads = [threading.Thread(target=worker, args=(index,)) for index in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=40)
    if any(thread.is_alive() for thread in threads):
        raise RuntimeError("broker concurrency court exceeded its wall deadline")
    if failures:
        raise failures[0]

    first, second = replies
    if not isinstance(first, dict) or not isinstance(second, dict):
        raise RuntimeError("broker concurrency court produced no reply")
    if first.get("state") != "completed" or second.get("state") != "completed":
        raise RuntimeError("both duplicate callers must receive the completed outcome")
    if first.get("receipt_sha256") != second.get("receipt_sha256"):
        raise RuntimeError("duplicate callers received different terminal receipts")
    print(
        json.dumps(
            {
                "connections": 2,
                "same_receipt": True,
                "state": "completed",
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
