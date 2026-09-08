# ACU bridge cancellation results

Status: policy decision demonstrated — detached helper is incompatible with
H1/H2; cooperative cancellation is the only in-process control that satisfies
the model, but its production implementability remains unproven.

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
- The cooperative row proves only that C1-C3 can coexist. The current provider
  ABI has no cancellation parameter, so it does not prove production
  feasibility.
- A real provider may eventually return and release its lock; the objection is
  post-cancel authority work and measured next-call delay, not a claim of a
  permanent deadlock.
