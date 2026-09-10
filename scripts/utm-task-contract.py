#!/usr/bin/env python3
"""Resolve and verify one UTM qualification task from machine SSOTs."""

import json
import pathlib
import re
import sys


def fail(message: str) -> "None":
    raise SystemExit(f"utm_task_contract:{message}")


def resolve(repo: pathlib.Path, task_id: str):
    if not re.fullmatch(r"[a-z0-9-]+", task_id):
        fail("invalid_task_id")
    try:
        task_manifest = json.loads(
            (repo / "agenterm.tasks.json").read_text(encoding="utf-8")
        )
        gate_manifest = json.loads(
            (repo / "scripts/qualification-gates.json").read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError) as error:
        fail(f"manifest_unavailable:{type(error).__name__}")
    if (
        gate_manifest.get("schema_version") != 2
        or not isinstance(gate_manifest.get("required_gates"), list)
        or not isinstance(gate_manifest.get("registered_gates"), list)
    ):
        fail("qualification_manifest_schema")

    tasks = [
        task for task in task_manifest.get("tasks", []) if task.get("id") == task_id
    ]
    if len(tasks) != 1:
        fail(f"task_count:{len(tasks)}")
    expected_entry = f"scripts/qjs/{task_id}.qjs"
    if tasks[0].get("entry") != expected_entry:
        fail("task_entry_mismatch")
    if not (repo / expected_entry).is_file():
        fail("task_entry_missing")

    declared_gates = gate_manifest.get("required_gates", []) + gate_manifest.get(
        "registered_gates", []
    )
    gates = [
        gate
        for gate in declared_gates
        if gate.get("id") == task_id
    ]
    if len(gates) != 1:
        fail(f"qualification_gate_count:{len(gates)}")
    evidence = gates[0].get("evidence")
    if not isinstance(evidence, list) or not evidence:
        fail("qualification_evidence_missing")
    if any(
        not isinstance(item, str) or not re.fullmatch(r"[a-z0-9.-]+", item)
        for item in evidence
    ):
        fail("qualification_evidence_invalid")
    if len(evidence) != len(set(evidence)):
        fail("qualification_evidence_duplicate")
    return evidence


def verify(evidence, text: str) -> str:
    actual = [line[9:] for line in text.splitlines() if line.startswith("EVIDENCE ")]
    if sorted(actual) != sorted(evidence):
        fail("qualification_evidence_mismatch")
    return next((line for line in text.splitlines() if line.startswith("PASS: ")), "")


def self_test() -> None:
    expected = ["cu.alpha", "cu.beta"]
    valid = "EVIDENCE cu.beta\nPASS: words may change\nEVIDENCE cu.alpha\n"
    assert verify(expected, valid) == "PASS: words may change"
    invalid_logs = (
        "EVIDENCE cu.alpha\n",
        "EVIDENCE cu.alpha\nEVIDENCE cu.beta\nEVIDENCE cu.beta\n",
        "EVIDENCE cu.alpha\nEVIDENCE cu.gamma\n",
    )
    for invalid in invalid_logs:
        try:
            verify(expected, invalid)
        except SystemExit:
            continue
        raise AssertionError("invalid evidence set was accepted")


if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
    self_test()
elif len(sys.argv) == 4 and sys.argv[1] == "resolve":
    for item in resolve(pathlib.Path(sys.argv[2]), sys.argv[3]):
        print(item)
elif len(sys.argv) == 5 and sys.argv[1] == "verify":
    expected = resolve(pathlib.Path(sys.argv[2]), sys.argv[3])
    try:
        log_text = pathlib.Path(sys.argv[4]).read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        fail(f"log_unavailable:{type(error).__name__}")
    print(verify(expected, log_text))
else:
    fail(
        "usage: utm-task-contract.py resolve REPO TASK | verify REPO TASK LOG | --self-test"
    )
