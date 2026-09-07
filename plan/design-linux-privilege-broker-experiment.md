# Linux privilege broker decisive experiment

> Experimental only. This does not make privilege apply shipped, does not
> change the capability ledger state, and must not enter a Candidate until the
> result is recorded and the owning PRD gate is promoted.

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Purpose | Decide whether a system-activated root broker can read private replay state before consent while polkit authorizes the kernel-authenticated desktop peer |
| Implementation | `research/linux-privilege-broker/` |
| Product owner | `prd/PRD_02_28_agenterm_cu.md` privilege branch |
| Prerequisites | `docs/agenterm-rust-cheatsheet.md`; existing typed plan/provider/replay code |
| Source discipline | Public Linux APIs and official polkit/systemd contracts; no MCU code copying |

## §0 · Fixed background

The one-shot `pkexec` rehearsal proves a fixed no-shell elevation path, but
consent happens before the provider process can read its root-only ledger. It
therefore cannot prove the product invariant “exact replay returns before any
new consent”. More argument about `pkexec` cannot create that ordering.

Already decided and outside this experiment:

1. The replay ledger stays root-only; it never becomes readable by the client.
2. The request remains the existing closed, bounded typed plan protocol.
3. Native consent is an OS-owned surface. AgenTerm never captures a password,
   MFA response, biometric result or generic administrator command.
4. The provider remains the sole owner of root-effect at-most-once state.
5. Client-supplied uid, pid, session, executable path and “authorized” flags
   are not identity or consent evidence.

## §1 · Hard constraints

- The broker must inspect `ReplayFinalized`, `OutcomeUnknown` and fingerprint
  conflict before invoking polkit.
- The peer subject must come from kernel credentials plus process-start
  identity and remain live across consent; request JSON and environment values
  cannot select it.
- Exactly one fixed root-owned socket/service identity is accepted. No endpoint
  override exists in production.
- A fresh request can reach the effect reservation only after polkit authorizes
  that exact ordinary-user peer.
- Cancel, deny, no-agent, malformed input and stale peer produce zero effects.
- Any disconnect after reservation is completed durably or remains
  `OutcomeUnknown`; it never becomes Fresh again.
- The spike must not invoke `pkexec`, `pkcheck`, a shell or an internal password
  prompt.
- **Disease detector:** any urge to make the ledger world-readable, trust a
  caller-provided identity, add a second user broker, or treat “dialog opened”
  as authorization is a finding that invalidates the variant, not a feature to
  add quietly.

## §2 · Minimum experiment

| Dimension | Fixed choice | Why |
|---|---|---|
| Carrier A | Root-owned Unix socket, systemd-style inherited listener | Separates private root state from ordinary client while retaining kernel peer credentials |
| Authorization A | Root broker calls polkit `CheckAuthorization` for `unix-process(pid,start-time,uid)` | Tests the exact ordering and desktop-agent association we need |
| Fallback B | System-bus D-Bus mechanism with sender bus identity | Used only if custom-socket peer cannot be associated reliably with the login session |
| Effect | None in the first spike; fixture counter only after authorization | Isolates the disputed consent/identity ordering from process mutation |
| Replay | Existing provider ledger fixture with finalized, unknown, conflict and missing rows | Proves prompt count, not just response shape |
| Hosts | One active Linux x86_64 desktop and one active Linux aarch64 desktop | Separates ISA/image-specific polkit-agent behavior |

Implementation may share one state machine and swap only the peer carrier. Two
independent prototypes are forbidden because duplicated logic would confound
the result.

## §3 · Precommitted criteria

| ID | Property | Pass condition |
|---|---|---|
| B1 | Boolean / safety | Finalized, unknown and conflict fixtures each return with `polkit_calls=0` |
| B2 | Boolean / identity | Fresh Missing displays consent in the kernel peer's active desktop session on both ISAs |
| B3 | Boolean / safety | Approve yields exactly one authorization and one fixture effect; cancel/deny/no-agent yields zero reservations and zero effects |
| B4 | Boolean / concurrency | Two simultaneous identical requests yield at most one prompt and one effect; the second receives replay |
| B5 | Boolean / lifecycle | Client exit cancels pre-reservation consent; post-reservation exit still closes durable terminal/unknown state |
| C1 | Checklist / trust | Socket ancestry, activation FD, broker inode, peer uid/pid/start identity and root-only state are all independently rejected when forged |
| C2 | Checklist / delivery | Upgrade interruption cannot run a mixed binary/policy/socket/service set |
| T1 | Bounded latency | Warm finalized replay returns within 500 ms without a polkit agent; report p50/max for 20 runs, do not use it to waive B1–B5 |

Measurement discipline:

- Count polkit calls and effects in broker-owned fixture counters, not logs or
  UI impressions.
- Record host ISA, distribution, desktop, polkit version and whether the exact
  tested binary executed.
- Preserve exact commands and SHA-256 values in
  `research/linux-privilege-broker/RESULTS.md`.
- This experiment makes no binary-size claim; L1/L2/L3 size is “not measured”.

## §4 · Decision tree, kill criteria and timebox

1. Evaluate B1 first. Any replay/unknown/conflict polkit call kills the socket
   variant immediately.
2. If B1 passes, evaluate B2 on both ISAs. If both pass, continue with A.
3. If B2 fails only because custom-socket peer identity cannot select the
   desktop polkit agent, reuse the same state machine with system-bus sender
   identity and rerun B1–B2 once.
4. The surviving carrier must pass B3, B4, B5 and C1. Any failure kills that
   carrier; do not weaken the invariant.
5. C2 is a delivery prerequisite after the carrier wins. T1 diagnoses
   usability but cannot override any safety Boolean.

Kill immediately if any variant requires password capture, a shell,
`pkexec`/`pkcheck`, caller identity claims, a world-readable ledger, duplicate
effects, or a reservation that can become Fresh after a crash.

Timebox: stop when A has produced B1 and B2 results on both ISA courts. Only if
B2 fails for the named association reason may B be implemented through its B2
result. Do not build packaging, real process effects or other platforms before
that decision exists.

Criterion coverage: B1 → step 1; B2 → steps 2–3; B3/B4/B5/C1 → step 4; C2 →
step 5; T1 is explicitly diagnostic only. Every pass/fail combination exits at
one of those nodes.

## §5 · Expected layout

```text
research/linux-privilege-broker/
├─ README.md              # commands and fixture boundaries
├─ RESULTS.md             # empty until executed; owns final decision trace
├─ broker fixture source
├─ client fixture source
├─ polkit action + activation fixtures
└─ run scripts            # one host/ISA result per isolated directory
```

## §6 · Excluded alternatives

| Alternative | Reason excluded |
|---|---|
| One-shot `pkexec` | Consent precedes root-ledger lookup |
| World-readable/client-side replay index | Leaks private authority state and cannot own root effect truth |
| User broker forwarding an “authorized” token | Recreates bearer authority, revocation and race protocols |
| Parent-PID or client-binary attestation | Does not prove polkit consent or the current kernel peer |
| Password/MFA bridge | Violates the native-consent boundary |
| Always-on service | Unneeded residency; socket activation owns demand startup |

## §7 · Not answered here

- macOS Authorization Services / SMAppService provider shape.
- Windows UAC provider and protected installation.
- Real process-signal effect correctness, already owned by exact-object fixture
  tests and later native provider courts.
- Package signing, notarization and public version assignment.
- qjswasm `agenterm:acu` object embedding.

## §8 · Result backfill

Status: **specification frozen; experiment not yet executed**.

When complete, record the B1→B5/C1/C2/T1 table, the exact decision-tree path,
all deviations, any result that overturned expectations, and this statement:
“No measurement was changed after observing its result.” A result is not
decided until `research/linux-privilege-broker/RESULTS.md` contains third-party
rerun commands and this section links to it.
