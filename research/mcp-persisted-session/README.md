# MCP persisted-session decision harness

This directory is a research-only, isolated Cargo workspace. It does not expose
an MCP mutation descriptor or modify product source. The harness compares the
negative lifecycle control A0, bounded derived-session alternative A1, and
request-direct alternative B against the frozen design in
`plan/design-mcp-persisted-session-experiment.md`.

This precommitment is closed. Attempt 1 was consumed and voided after its only
invocation lane disappeared before independent review. Do not run this harness
again under the current precommitment; the command below is retained only as a
historical record of the frozen procedure:

```sh
research/mcp-persisted-session/run.sh 1
```

Attempt `2` was reserved for the single fixture-only repair allowed by the
precommitment, but no review chain exists and it is not authorized. The runner
rejects any later attempt. It builds only into
`target/research-mcp-persisted-session`, runs the pure `--self-test` before
creating an invocation directory, and writes one bounded receipt under that
repo-local lane as well as to stdout. Keep the invocation directory only until
independent review, then remove the complete lane.

The receipt deliberately contains only bounded codes, counts, digests and
repo-relative input names. It never contains a grant or store selector, target
binding bytes, session lease, installation key, filesystem path, process id,
command output, or credential-shaped value. `RESULTS.md` is updated only after
independent review; the harness never edits it.

Metadata read or parse failure occurs before the harness has the frozen source
and digest context needed for a receipt. That narrow infrastructure failure is
therefore returned only as a typed process error; it is not listed as a
repairable fixture receipt and cannot consume a verdict branch. If it occurs
after the runner reserves an attempt, no terminal receipt or `finished` ledger
row exists and the ledger remains `reserved`; that run is an infrastructure
failure, not unused or fixture-repairable budget.

Each durable phase uses a separate grant-server child. The first child is
stopped at the frozen failure point and a second child reopens the independent
grant, request and mechanism witnesses. The A1 client holds only its bounded
scoped-session fields and the child request capability; it cannot open or write
witness files. For C4 that same tested server owns a bounded queue, session,
connection and worker child. The runner breaks its stdout while retaining the
stdin writer; cleanup is proven by parent `Child` reaping plus an independent
reopen of the append-only ownership-event witness, never by a child reply or a
child-authored final count.

The attempt budget is an evidence chain, not tamper resistance. Attempt 2
requires an attempt-1 terminal fixture/contract-failure receipt, its
independently reviewed digest, a descendant source, an allowlisted
fixture/observation-only diff, and unchanged contract/criteria/decision
digests. Fixture changes are field-allowlisted to opaque selectors, binding,
request fingerprint/id and marker; grant semantics, time, ceilings, failure
phases and protocol probes cannot change. `RESULTS.md` records one exact line of the form `Attempt 1 review:
receipt_sha256=<digest> failure_class=fixture-contract-failure
repair_marker_sha256=<digest>`; the repair marker is the domain-separated hash
of the reviewed receipt digest, attempt-1 source and the fixed repair label,
not an arbitrary hexadecimal token. Removing local invocation state
after a reviewed result therefore refuses attempt 2. Deliberately deleting the
lane before review destroys the only local evidence and cannot be detected
within this experiment's write boundary; such deletion invalidates the run and
must not be represented as unused budget.

The receipt freezes three complementary digest slices. `input_digest` covers
the tracked specification plus seven research inputs (all except `RESULTS.md`);
`contract_digest` locks the specification plus result-template contract; `criteria_digest` and
`decision_digest` separately lock the C1-C7 evaluator and survivor/tie
decision code. Attempt 2 requires every frozen slice to match attempt 1.

Each case gives child request, response, stop and cleanup waits one shared
10-second monotonic deadline. Small synchronous local-file operations are
bounded by input-size limits and checked against that deadline on return, but
cannot be preempted while the operating-system call itself is blocked; that is
an explicit P2 limit of this research harness rather than a product guarantee.
