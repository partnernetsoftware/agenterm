# Archived execution plans

Archived files preserve decisions and evidence but are not task entrypoints.
Current product truth starts at `PRD.md`; current sequencing starts at
`plan/README.md`.

## Release and CI history

- `plan-v0.1.15.md` — superseded, never-public 0.1.15 work ledger.
- `plan-v0.1.16.md` — shipped exact-SHA v0.1.16 campaign.
- `ci-green-handoff-v0.1.16.md` and `claude-analyze-ci-v0.1.16.md` — bounded
  diagnostics from that campaign.
- `goal-codex-v0.1.16.md` — completed primary-agent goal snapshot for that
  release and its signing follow-up.
- `plan-v0.1.18-pre-qjswasm-app-pack.md` and
  `plan-v0.1.19-pre-cu-route.md` — superseded route drafts whose QJS/Rh/App
  assumptions no longer match the active qjswasm/CU route.

## Rh-era history

`plan-rh-3.md`, `design-rh-aot.md`, `design-rhai-rust-boundary.md`,
`goal-rh-product.md`, `rh-tdd-review.md`, `research-rhai-kernel-depth.md`, and
the `design-rh-standalone-product*` set preserve the extracted Rh product's
history. Rh left this repository; current `.qjs` work is qjswasm/tinyvm.

`design-scripting-boundary-comparison.md` preserves the Rh-era industry
comparison, and `design-script-engine-trait.md` preserves the completed shared
adapter campaign that preceded the current qjswasm/Lua/SQL routing shape.

The superseded dyn S-expression reviews, the completed qjs/wasmcore archive
gates, and the old qjs module-import design are retained here for the same
reason: their product eras have ended, while their decisions remain useful
history.

## Completed decision experiments

- `design-process-policy-authority-experiment.md` — rejected the exact
  arbitrary-process Mach authority route.
- `design-qjswasm-host-reply-wire-cost-experiment.md` — assigned oversized
  host-reply ownership to host-side field selection.
- `design-qjswasm-immediate-host-argument-region-experiment.md` and
  `design-qjswasm-region-lifetime-experiment.md` — preserved the rejected
  temporary-region specialization decisions.
- `design-ui-snapshot-selection-boundary-experiment.md` — admitted transport
  field selection without changing the product capability state.
- `design-cu-single-entry-size-experiment.md` — retained the bounded hot/cold
  catalog, rejected the ABI relocation, and returned the unresolved size gap
  to the CU product budget.
- `design-pty-process-control-experiment.md` — accepted exact POSIX foreground
  signaling while preserving the typed Windows limitation.
- `design-value-representation-experiment.md` — selected the measured V1
  `(tag: i32, payload: i64)` representation over NaN-boxing.

## Superseded execution notes

`phase0-baseline-measurements.md`, `note-six-runner-courts.md`, and
`grok.glm.refactor.review.md` preserve bounded historical measurements or
reviews whose current owners are named in their archive banners. The two
`*-shell-l1-l2-l3.md` files are retired rename pointers to the active chassis
documents. `libagenterm-verification-state.md` preserves the retired ABI
workflow ledger, while `experiment-headless-pty-owner.md` preserves the settled
single-authority decision now owned by the CU PRDs.
`prompt-grok-cursor-cloud.md` preserves the superseded cloud-agent polling
playbook; current execution ownership is declared only in `plan/goal.md`.
`design-script-process-witness-experiment.md` preserves the completed A/B
decision that moved Script process inventory onto the shipped `process.list`
door; PRD 02.36 and the current smoke helper own the delivered behavior.
`precision-audit.md` preserves the closed 92-item narrow audit ledger; every
finding is fixed, adjudicated, or bounded by an explicitly named owner.
`design-dyn-typed-symbol-fold-experiment.md` preserves the completed decision
that rejected a generic dyn seam and folded the two real consumers into one
platform-private loader with no delivered-byte growth.
`design-qjs-module-identity-experiment.md` preserves the stopped module-identity
court: canonical accounting shipped, while single evaluation remains blocked
on an upstream identity-bearing compiler interface.
`design-acu-bridge-cancellation-experiment.md` preserves the completed decision
that rejected detached bridge work and established the call-scoped cooperative
cancellation pattern now owned by the CU and qjswasm PRDs.
`ds4pro-analyze-wastime-and-scripting-layer.md` preserves the superseded
three-engine comparison; PRD 02.36 now owns the evidence-gated,
workload-by-workload Wasmtime-class replacement horizon.
`goal-agenterm-osx.md` preserves the completed v0.1.15-era macOS campaign;
current platform work and release qualification are owned by the platform goal,
active version plan and release skill.
`plan-ape-thin-shell-dynamic-packages.md` preserves the superseded APE/thin-shell
draft; the active fast-change boundary is the v0.1.19 Chassis plan.
`design-llm-gateway-rhai-logic-pack.md` preserves the retired Rhai-specific
implementation draft; PRD 13 retains only the engine-neutral Native Shell and
independently versioned Logic Pack boundary.
`design-device-watch-cancellation-experiment.md` preserves the completed
structured-partial cancellation decision; PRD 28 owns the shipped
`device-watch` behavior and remaining native-platform qualification.

## Restored decision records

The historical Markdown archive accidentally removed from the tree in an
earlier cleanup has been restored here so existing PRD/design links resolve.
Generated crate/research tarballs were deliberately not restored. Four compact
dynamic-core verdict documents needed by archived designs live under
`dynamic-core-results/`; they preserve conclusions without reviving the closed
research source tree.

Do not use archived status, commands or run identities as current evidence.
