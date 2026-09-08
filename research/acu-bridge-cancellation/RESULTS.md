# ACU bridge cancellation results

Status: decided — detached helper rejected; cooperative cancellation is the
only in-process route that passed the safety gates.

## Measurement

Environment: Rust 1.97.0, `aarch64-apple-darwin`. All variants used the same
250 ms callback, 25 ms cancellation trigger, one provider mutex, and one active
callback counter.

| variant | elapsed | active at return | lock immediately free | verdict |
|---|---:|---:|---|---|
| synchronous | 260 ms | 0 | yes | safe but not preemptible |
| detached helper | 29 ms | 1 | no | reject: C1/C2 fail |
| cooperative | 33 ms | 0 | yes | pass C1-C3 |

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
provider-lock availability for synchronous, detached, and cooperative variants.

## Decision trace

1. Detached passed the sub-100 ms timing gate.
2. It still had one active callback and held the provider lock, so C1 and C2
   failed and the decision tree rejects it without considering convenience.
3. Cooperative cancellation passed the same timing gate with zero active work
   and an available lock.
4. Production therefore keeps process containment for uncooperative callbacks;
   an in-process change waits for a real Executor/native cooperative token.

No criterion or duration changed after seeing the result. The useful reversal
is that the fastest-looking implementation is the one rejected by the safety
gates.
