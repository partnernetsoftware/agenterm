# qjswasm host-reply wire cost

Verdict directory for
[`plan/design-qjswasm-host-reply-wire-cost-experiment.md`](../../plan/design-qjswasm-host-reply-wire-cost-experiment.md).
It contains no product wire change, no door or API addition, no guest change and
no upstream work.

**Verdict: `V0 yes → W0-C yes → W1 no → owner = host-side field selection`.**
Two usable journeys, both above the frozen 10% `ΔC` share (server-smoke 49.58%,
native-ipc-smoke 30.00%) and both well below the 75% indentation-only share
(12.59%, 30.14%), so indentation is not the owner and the field-selection route
is. `workbench-smoke` is retained as a truncated row and excluded from the gate
(its control fails at the frozen macOS adapter boundary). All figures are
post-redaction; §3 of `RESULTS.md` records the redaction as a pre-freeze step.

Layout:

- `lane.json` — frozen run identity: HEAD, pin, lane, binary sha256, toolchain,
  budgets, the commands actually run.
- `replies/<journey>/` — the V1 frozen reply set (one capture per journey) with
  `index.jsonl` (per-reply command, envelope/payload source, exit code).
- `capture/` — the instrumented reporter copies derived from the pristine
  journeys by `tools/make_capture.py`, with the diff against the journey beside
  each copy. The control runs use the pristine scripts, never these.
- `census.json` — per-reply read census: parse kind, read paths in code order,
  anchor line, and the over-retention notes. Static derivation from the journey
  sources.
- `shapes/<journey>/{A,B,C}/` + `shapes/manifest.json` — the three shapes and
  their sha256 digests; `index.json` also lists the census paths that do not
  exist in the frozen reply.
- `runs/` — raw `cost` envelopes for the control runs, the court runs and the
  allocation-probe runs.
- `measurements.json` — the judged table, the two deltas and the shares, with
  `usable` / `unusable_reason` per journey.
- `receipt.json` — independent per-shape digests (`reply_set_sha256`,
  `value_set_sha256`) so the shapes cannot all be wrong together.
- `V1-recheck.json` — the one re-capture shape check.
- `RESULTS.md` — the third-party rerunnable receipt, written per §8.
- `tools/` — `make_capture.py`, `run-capture.sh`, `run-control.sh`,
  `make_shapes.py`, `run-court.sh`, `court.qjs`, `aggregate.py`, `receipt.py`,
  `json_shape.py`. Every judged number comes from the lane binary; the Python
  tools only derive bytes and validate shapes.

The captured replies are stored **redacted**: host identity (the clone root
spelled absolutely, the user home, the account name, the personal hostname and
the instance labels derived from them) was replaced by generic placeholders
before the set was frozen, and the one-off redaction script was deleted so the
tree never carries the replaced values. Every shape, digest and measurement
derives from the redacted bytes; `RESULTS.md` §3 and §8.3 record the step and its
effect on the numbers.
