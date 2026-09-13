#!/usr/bin/env python3
"""Aggregate the experiment's runs into measurements.json and print the judged
table. Every judged number is `cost.steps` from the same lane binary; a median
of three; a share is reported to one decimal and never rounded up to a gate.

  python3 research/qjswasm-host-reply-wire-cost/tools/aggregate.py
"""

from __future__ import annotations

import glob
import json
import pathlib
import statistics

HERE = pathlib.Path(__file__).resolve().parents[1]
RUNS = HERE / "runs"
JOURNEYS = ["server-smoke", "native-ipc-smoke", "workbench-smoke"]


def load(pattern: str) -> list[dict]:
    out = []
    for path in sorted(RUNS.glob(pattern)):
        try:
            envelope = json.loads(pathlib.Path(path).read_text())
        except ValueError:
            envelope = {"ok": False, "failure": {"message": "unparsable"}}
        envelope["_file"] = path.name
        out.append(envelope)
    return out


def steps_of(envelope: dict) -> int | None:
    cost = envelope.get("cost") or {}
    return cost.get("steps")


def main() -> int:
    census = json.loads((HERE / "census.json").read_text())
    shapes_bytes = json.loads((HERE / "shapes" / "manifest.json").read_text())["bytes"]
    measurements: dict = {"journeys": {}, "lane": json.loads((HERE / "lane.json").read_text())}
    print("journey            shape   runs                          median      delta     share")
    for journey in JOURNEYS:
        envelopes = load(f"{journey}-*.json")
        controls = [steps_of(e) for e in envelopes]
        controls = [s for s in controls if s is not None]
        control_median = int(statistics.median(controls)) if controls else None
        control_ok = all(e.get("ok") is True for e in envelopes) and envelopes
        control_class = sorted({e.get("exit_class") for e in envelopes})
        row: dict = {
            "replies": len(census["journeys"][journey]["replies"]),
            "control_steps": controls,
            "control_median": control_median,
            "control_spread": (max(controls) - min(controls)) if controls else None,
            "control_ok": bool(control_ok),
            "control_exit_classes": control_class,
            "usable": bool(control_ok),
            "unusable_reason": None if control_ok else (
                "control run does not succeed on this host: its step total is a truncated stop point, not a "
                "journey total, so it cannot carry the W0-C denominator"
            ),
            "bytes": shapes_bytes[journey],
        }
        medians = {}
        for shape in ("A", "B", "C"):
            runs = [steps_of(e) for e in load(f"court-{journey}-{shape}-*.json") if "probe" not in e.get("_file", "")]
            runs = [s for s in runs if s is not None]
            probe_runs = [
                e for e in load(f"court-{journey}-{shape}-*-probe.json")
            ]
            row[f"court_steps_{shape}"] = runs
            row[f"court_median_{shape}"] = int(statistics.median(runs)) if runs else None
            medians[shape] = row[f"court_median_{shape}"]
            if probe_runs:
                row[f"probe_steps_{shape}"] = [steps_of(e) for e in probe_runs]
                row[f"json_parse_bytes_{shape}"] = [
                    (e.get("cost") or {}).get("json_parse_bytes") for e in probe_runs
                ]
            spread = (max(runs) - min(runs)) if runs else None
            share = ""
            if medians["A"] and medians[shape] and control_median:
                delta = medians["A"] - medians[shape]
                share = f"{delta / control_median * 100:.1f}%"
            print(
                f"{journey:<19}{shape:<8}{str(runs):<30}{str(medians[shape]):<12}{'':<10}{share}"
            )
        if medians["A"] is not None and medians["B"] is not None and medians["C"] is not None:
            row["delta_B"] = medians["A"] - medians["B"]
            row["delta_C"] = medians["A"] - medians["C"]
            row["delta_C_over_total"] = (
                row["delta_C"] / control_median if control_median else None
            )
            row["delta_B_over_delta_C"] = (
                row["delta_B"] / row["delta_C"] if row["delta_C"] else None
            )
        measurements["journeys"][journey] = row
    (HERE / "measurements.json").write_text(
        json.dumps(measurements, indent=1, ensure_ascii=False) + "\n"
    )

    print()
    print("gate walk")
    valid = []
    for journey in JOURNEYS:
        row = measurements["journeys"][journey]
        if not row["usable"]:
            print(f"  {journey:<19} UNUSABLE: {row['unusable_reason']}")
            continue
        share = row.get("delta_C_over_total")
        if share is None:
            continue
        valid.append((journey, share, row.get("delta_B_over_delta_C")))
        print(
            f"  {journey:<19} deltaC/total={share * 100:.4f}%  deltaB/deltaC="
            f"{row['delta_B_over_delta_C'] * 100:.4f}%"
        )
    passing = [j for j, share, _ in valid if share >= 0.10]
    print(
        f"  W0-C journeys at or above 10%: {len(passing)} of {len(valid)} usable "
        f"-> {passing} (the frozen gate wants at least two journeys)"
    )
    wins = [ratio for _, _, ratio in valid if ratio is not None]
    if len(passing) >= 2 and wins:
        verdict = "compact reply text only" if all(r >= 0.75 for r in wins) else "host-side field selection"
        print(f"  W1 (deltaB/deltaC per journey: {[round(r * 100, 4) for r in wins]}): owner = {verdict}")
    elif valid:
        print("  W0-C not met -> kill the product-wire route at this pin")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
