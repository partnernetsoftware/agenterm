# ACU bridge cancellation results

Status: policy decision and first product slice complete — detached helper is
incompatible with H1/H2; the call-scoped cooperative path is implemented for
observe-only `process-watch`, while other native waits remain unqualified.

## Measurement

Environment: Rust 1.97.0, `aarch64-apple-darwin`. All variants used the same
250 ms callback, 25 ms cancellation trigger, one provider mutex, and one active
callback counter.

| variant | elapsed | active at return | lock immediately free | next-call wait | verdict |
|---|---:|---:|---|---:|---|
| synchronous | 252 ms | 0 | yes | 0 ms | safe but not preemptible |
| detached helper | 35 ms | 1 | no | 223 ms | reject: C1/C2/C5 fail |
| cooperative | 34 ms | 0 | yes | 0 ms | structural control passes C1-C3/C5 |

The synchronous row records its state after the callback completed; its purpose
is the C4 timing baseline. The detached row intentionally observes state before
joining the worker, then joins it so the probe itself leaves no thread behind.

Run from the repository root:

```sh
rustc --edition=2024 research/acu-bridge-cancellation/probe.rs \
  -o target/acu-bridge-cancellation-probe
target/acu-bridge-cancellation-probe
```

The output records elapsed milliseconds, active callback count, and immediate
provider-lock availability for synchronous, detached, and cooperative variants;
the detached row also measures the next call's wait on that residual lock.

## Decision trace

1. Detached is incompatible with H1/H2 by construction; the probe is a
   confirmation and impact measurement, not a discovery of that policy.
2. It returned below 100 ms but still had one active callback, held the provider
   lock, and delayed the next provider call, so C1, C2 and C5 failed.
3. Cooperative cancellation passed the structural timing control with zero
   active work and an available lock.
4. Production therefore keeps process containment for uncooperative callbacks;
   an in-process change waits for a real Executor/native cooperative token.

No criterion or duration changed after seeing the result. The useful reversal
is that the fastest-looking implementation is the one rejected by the safety
gates.

## Production differences and limits

- The real fixed-sibling path owns both the host reply mutex and a provider
  global call lock; the probe uses one mutex, so it is a lower-bound model.
- A real detached bridge must copy the command out of guest memory and preserve
  panic-to-door-fault translation. The probe measures neither.
- The cooperative row by itself proves only that C1-C3 can coexist. The product
  integration below separately proves feasibility through an additive provider
  symbol while retaining ABI v1 unchanged.
- A real provider may eventually return and release its lock; the objection is
  post-cancel authority work and measured next-call delay, not a claim of a
  permanent deadlock.

## Product integration result · process-watch vertical slice

Status: A, the call-scoped callback/probe, passed the bounded observe-only
slice. No helper thread was introduced.

The fixed-sibling provider keeps ABI v1 and its original call symbol unchanged.
An additive v2 symbol borrows one caller-sized callback/context descriptor for
the synchronous call; the callback runs on that same thread and is never
stored. `process-watch` checks it before the first snapshot, around later
snapshots, and during long sleeps in slices no larger than 10 ms.

Measured from the repository root with a freshly built debug client and
`abi-dev` provider sibling:

| observation | result |
|---|---:|
| `agenterm cli acu --timeout-ms 1000 process-watch ... --duration-ms 60000 --interval-ms 60000` | typed Script `cancelled`, 1.02 s total; no hard timeout |
| immediate next `agenterm cli acu capabilities` | success, 0.48 s |
| provider v1/v2 unit boundary | 11/11 pass |
| qjswasm ACU-door cancellation/reply boundary | 8/8 pass |
| process-watch focused tests | 7/7 pass; 25 ms trigger returns below the 100 ms test gate |

A 100 ms public-worker envelope was intentionally not accepted as evidence:
debug worker startup consumed the complete deadline before the invocation was
active, so the supervisor correctly used hard containment. The 1,000 ms case
places cancellation inside the active native wait and isolates the mechanism
being judged.

Two integration findings changed the patch before acceptance:

1. Packed `.wasm` execution discarded all invocation options. It now builds
   the engine from the same budget, arguments, tool door, replay inputs and
   cancellation identity as source execution, with an infinite-loop artifact
   regression.
2. The old ACU door checked the cancel flag unconditionally after every bridge
   reply. That could hide an authoritative mutation result. The bridge now has
   a separate acknowledgement bit and sets it only for typed
   `cancelled` + `effect:not_performed`; a late, unacknowledged flag preserves
   the reply. Tests cover both outcomes.

P1-P6 from the design pass for this slice. This is not permission to spread a
generic checkpoint through mutation code. Other native waits remain under the
worker hard-containment boundary until each owns phase-aware cancellation and
its own public court.
