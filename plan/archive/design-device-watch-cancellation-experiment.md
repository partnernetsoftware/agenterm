> **已归档 2026-09-16 — 已判决并交付。** 实验选择具名取消错误携带完整、受界限约束的部分观测；生产 `device-watch` 已实现该语义，PRD 28 已吸收现行契约。
> 现行权威：`prd/PRD_02_28_agenterm_cu.md` 的 device-watch 与 cooperative cancellation 分支。

# Device-watch cancellation evidence experiment

## Background and fixed facts

`device-watch` is a bounded observe verb. It takes an immediate authoritative
inventory sample and may then accumulate further complete-snapshot diffs until
the duration or event ceiling ends the watch. `ExecutionControl` is available
at dispatch but is not currently passed into this loop.

The cancellation contract cannot be copied mechanically from waits that have
not yet performed an observation:

- before the first inventory call, `cancelled` with `effect: not_performed` is
  exact;
- after any successful sample, that claim is false;
- samples and events already returned by the platform authority remain facts
  and must not be silently discarded;
- a sample result, provider error, reached event ceiling, or reached deadline
  from the current round is authoritative and must not be hidden by a token
  that becomes set during that round.

The current successful payload names only `duration` and `event-limit` as
termination reasons. The current public smoke treats those as the closed set.
`CuError.detail` is the existing structured failure-evidence carrier, but its
actual end-to-end preservation and consumers must be measured rather than
assumed.

## Hard constraints and pathology detector

Any acceptable design must satisfy all of these:

1. Pre-first-sample cancellation issues zero platform inventory calls and uses
   the existing named cancellation shape with `effect: not_performed`.
2. Post-sample cancellation preserves the exact bounded partial observation:
   sample count, events, provider coverage, truncation, and suppression count.
3. No response may claim `effect: not_performed` after a sample completed.
4. Same-round platform results and errors win over late cancellation.
5. Normal duration and event-limit wire shapes and named outcomes do not
   change.
6. No thread, signal, async runtime, second inventory mechanism, or shortened
   platform timeout is introduced.
7. Existing consumers must not mistake a cancelled partial observation for a
   complete normal watch.

The pathology detector is a token flipped inside the injected inventory
provider while that provider returns a real sample containing a distinguishable
event. A design fails if the event disappears, the response says no effect was
performed, or cancellation replaces that same-round result.

## Candidate structures

### A. Named cancellation error with structured partial evidence

Return `code: cancelled` and carry the complete partial watch payload in the
existing structured error detail, with a phase/effect description that says an
observation was partially performed. This option survives only if the complete
detail is preserved through every public reply path and is accessible to the
actual Script consumer.

### B. Successful partial payload with an explicit cancelled termination

Return the normal data payload with a new `termination: cancelled` and an
explicit incomplete/completion field. This option survives only if current
consumers use termination/completion rather than `ok: true` alone when deciding
that a watch completed normally, and the schema extension is backward-safe.

### C. Cancellation only before the first sample

After the first sample, ignore the token and run to the ordinary bound. This is
truthful but does not provide cooperative cancellation for the long-running
portion. It is a control, not an acceptable delivery, unless both evidence
carriers above are proven unusable without breaking an existing contract.

## Minimal experiment

This experiment is consumer- and serialization-focused; it does not require a
live device or browser.

1. Trace a `CuError` with nested partial-watch detail from executor return
   through `CuReply`, CLI stdout and the qjswasm CU door. Record whether any
   layer drops, rewrites, or hides the detail.
2. Enumerate repository consumers of `device-watch`, `termination`, `samples`,
   `events`, `coverage_complete`, and command success. Classify whether each
   consumer distinguishes a cancelled partial response from a normal complete
   response under A and B.
3. Build the smallest production-loop seam that injects inventory samples and
   a cancellation probe. Exercise the pathology detector against the surviving
   wire shape without changing platform enumeration.

No tracked implementation change is permitted until steps 1 and 2 select a
single surviving structure.

## Precommitted decision tree

1. If A preserves the full partial payload end to end and callers can read it,
   choose A. It retains failure signalling while keeping already-observed facts.
2. Otherwise, if B is unambiguous to every current consumer and can explicitly
   distinguish incomplete cancellation from duration/event-limit completion,
   choose B.
3. If both A and B survive, choose A because it preserves the existing meaning
   that cooperative cancellation is a named non-success outcome.
4. If neither survives, stop. Do not ship C as if cooperative cancellation were
   closed; record the missing structured-result capability instead.

## Owning evidence

The implementation selected by the decision tree must have production-driver
tests proving:

- pre-cancel performs zero inventory calls;
- cancellation after one unmatched sample returns within one poll interval and
  preserves that sample's full bounded evidence;
- a sample that flips the token and produces a real event wins in the same
  round and the event remains present;
- a provider error that flips the token remains the named provider error;
- ordinary duration and event-limit responses are byte-shape compatible except
  for fields explicitly approved by this experiment.

## Timebox and kill criterion

Timebox the consumer/serialization audit and injected proof to one small CU
leaf. Stop if either surviving option requires a new public transport, a second
wait mechanism, changing platform inventory semantics, weakening a named
provider error, or teaching existing consumers to infer completion from prose.
Stop as well if the same-round pathology cannot be driven through the real
production loop.

## Result

Selected: **A, named cancellation error with structured partial evidence**.

The measured reply path preserves `CuError.detail` verbatim through the public
JSON envelope. `CuReply::err` intentionally has no success `data`, so the
partial watch belongs in that detail. The existing `WatchState::into_value`
must remain the sole encoder for both normal and cancelled observations: it
applies the installation-scoped public device projection, row truncation, and
the 1 MiB response ceiling before the resulting value is placed in error
detail. The nested partial payload names `termination: cancelled`; normal
successful payloads retain only `duration|event-limit`. No raw `DeviceRecord`
may be serialized directly on the error path.

B was rejected because every `ok: true` device-watch currently exits zero, the
public court treats `duration|event-limit` as the closed termination set, and
`coverage_complete` already means provider completeness rather than watch
completion. A third success termination would therefore change existing
success semantics and could be mistaken for a normally completed watch.

C was rejected as a delivery because it leaves the long-running post-sample
portion unresponsive. It remains only the truthful fallback if structured
partial evidence later proves impossible; the measurement found no such
blocker.
