# Linux privilege broker results

Status: **in progress; no promotion verdict**.

Frozen criteria are in `plan/design-linux-privilege-broker-experiment.md`.
No criterion or threshold below was changed after observing a result.

## Exact tested generation

| Item | Value |
|---|---|
| Source | `5991c529` |
| Linux x86_64 `agenterm-cu` SHA-256 | `9e9b4cc2c768830efeced3b51eceb678fb6cad629a6e2b35ddea91b475c71783` |
| polkit policy SHA-256 | `30dcb292117339c450a7b87d8ac1d852765c622d842a074a97bb56a0df50828c` |
| socket unit SHA-256 | `c475f4099ec1c4674a414789874005a83000834198ba919598be8c1fc56cebdf` |
| service unit SHA-256 | `c67287bf7700566b36d2c8d572b87df07ad085948a3d5a5b40b3dd91266427ea` |
| Linux aarch64 `agenterm-cu` SHA-256 | `6b38d11ef5c1b092c7ff8384c66a98bfae3416973be39d03a04b8ac08dfbf49c` (built, not yet executed) |

## Court facts

| Cell | Distribution / desktop | Session proof | Result |
|---|---|---|---|
| Linux x86_64 | Ubuntu-family Xubuntu / XFCE / X11 | ordinary uid, unique active local session, DISPLAY, D-Bus, AT-SPI and WM connected | partial pass |
| Linux aarch64 | pending | pending | not run |

The x86_64 package installer verified all four input digests, installed the
fixed provider, enabled and started only the socket, and the installed provider
digest matched the source artifact. The first real public apply proved:

- Missing request caused systemd activation and the desktop polkit agent showed
  the fixed AgenTerm action/vendor prompt.
- Explicit cancellation returned `privilege_consent_canceled` with
  `effect=not_performed`; the root-owned fixture process remained live.
- Explicit approval returned `completed`, `verification=verified`, a sealed
  receipt id/digest, and the exact root-owned fixture process terminated.
- Replaying one completed request returned the identical receipt and did not
  repeat an effect. One guest-local timed replay measured `0.13 s`, below T1's
  500 ms ceiling.

That original run did not satisfy criteria requiring broker-owned counters or
both ISAs. In particular, “no second dialog was visible” was not substituted
for the precommitted native-consent counter.

The `5991c529` x86_64 rerun added broker-owned durable boundary counters. One
fresh request followed by its exact replay and one conflicting fingerprint
with the same request id produced this exact delta:

| Counter | Delta |
|---|---:|
| requests | 3 |
| native consent calls | 1 |
| effect attempts | 1 |
| finalized replays | 1 |
| request conflicts | 1 |
| completed replies | 2 |
| refused replies | 1 |

The fresh and replayed calls returned the same immutable receipt digest. The
conflict returned the typed public `request_id_conflict` refusal with no second
consent or effect. An earlier build exposed that the server closed a conflict
without a frame, making the ordinary client conservatively report
`privilege_outcome_unknown`; `5991c529` corrected that false uncertainty and
the same real court proved the typed result.

An attempted public-CLI B4 court did not reach the broker concurrently: two
processes sharing one runtime session are intentionally serialized by the
ordinary-user runtime lock, and the loser returned `runtime_lock_contended`.
This is not counted as B4. A native court driver must open two broker
connections with one identical already-bound request without weakening the
public session lock.

## Criteria ledger

| ID | State | Current evidence / missing evidence |
|---|---|---|
| B1 | partial | x86_64 broker counters prove finalized replay and fingerprint conflict each use zero additional consent/effect; retained-unknown counter proof remains missing, and aarch64 is pending |
| B2 | partial | Exact x86_64 desktop prompt passed; aarch64 pending |
| B3 | partial | x86_64 approve = one verified effect; cancel = zero effect; deny/no-agent and counter proof pending |
| B4 | pending | public callers are serialized by their runtime-session lock; a direct native two-connection court is still required |
| B5 | pending | pre-reservation client death and post-reservation disconnect not run |
| C1 | partial | forged-input unit tests pass; real installed rejection matrix not yet executed |
| C2 | partial | five failure cuts, five SIGKILL cuts, first install, bad digest and uninstall pass in the package selftest; real Linux interrupted upgrade pending |
| T1 | partial | one warm replay = 0.13 s; required 20-run p50/max pending |

## Size court — blocking

The current stripped dynamically linked release artifacts are approximately
10 MiB on Linux x86_64 and 7.7 MiB on Linux aarch64. They exceed the governing
2 MiB `agenterm-cu` release budget after the polkit transport became live.
The budget is not raised. This generation cannot be promoted even if functional
criteria later pass; the implementation must recover the size through a typed
architecture change and rerun the exact courts.

## Protocol/court findings

- The first Linux session bridge used fixed result filenames. Review caught a
  late-result race; `utm-court` now binds every result path and JSON receipt to
  one opaque job id and ignores stale results.
- A first systemd-user bridge sandbox made `/` unreadable, changing product
  filesystem semantics and causing `session-start` to fail. The court removed
  path sandboxing while retaining `NoNewPrivileges`; a test harness must not
  silently reduce the product surface it claims to qualify.
- Packaging was implemented before B2 although the frozen timebox said not to.
  This deviation did not change any criterion and packaging cannot count as
  acceptance evidence until the court completes.

## Decision trace

Carrier A remains alive. The x86_64 result proves that a custom systemd socket
peer can be associated with the active desktop polkit agent, so fallback B is
not justified. No final decision exists until the missing table rows are
executed on both ISAs and the size blocker is removed.
