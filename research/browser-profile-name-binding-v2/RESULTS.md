# Browser Profile name binding owned-lifecycle experiment v2 results

Status: **terminal without a design verdict**.

Both permitted lifecycle rehearsals were consumed and finished on 2026-09-11.
Neither produced `REHEARSAL_PASS`, so D1 is machine-ineligible. No A0, A1, or B
alternative was selected; no product evidence was registered; the stable TODO
and capability state remain unchanged.

## Attempt summary

Both runs used `Brave Browser 152.1.94.121`, browser executable SHA-256
`f2a34a08b7aeb4b46706edfaa7f0d529482c9e1128e90ec8538ed117706ba3e2`,
`agenterm` SHA-256
`d3d6119f0fbd6d2ceca3985d6fef60d46dbcf85d8afb5bbbee1e57b3f84a4a0f`,
and `agenterm-cu` SHA-256
`a670767155bc72bcd10377df0c6e9d8e15b80b0969a503f6d057659d1caa9083`.
The browser was the official signed stable macOS distribution installed at
`~/Applications/Brave Browser.app`; the path itself is not an identity claim.

| Attempt | Source | Input digest | Template digest | Receipt digest | Terminal |
|---|---|---|---|---|---|
| R1 | `f8e8799a4913807a205e300618fba997c1c5f91a` | `e4c2dc4b555ccf253fa977a5985ce06c0e4e1c5cd73088a609eddbbcf6b1fcb5` | `a036764abcb272bfd06c3698b00c406bced120421fef36643e8d67a2c0da9925` | `a562b88104c7785d1ea2648834ff379903ed23c05d71db7d18a5cf3e578f7633` | `INCONCLUSIVE_DEPENDENCY` |
| R2 | `e361329d114089d62a509a9af5e875cb682641a5` | `4287082f4ab94b5a28ef5d43941dbb9690578d2728819cc72298b29a82be15e6` | `973aa2aec7c224d74c60c23089b72b38e9b3cb031cfe101ff6c126be1bb05b69` | `94d33953e2582230224452425dea9dca3968c9ddb010f7155e63b9353fd927d3` | `INCONCLUSIVE_OWNERSHIP` |

There is no successful rehearsal receipt. The authoritative external ledger is
exactly:

```jsonl
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"e4c2dc4b555ccf253fa977a5985ce06c0e4e1c5cd73088a609eddbbcf6b1fcb5","kind":"rehearsal","ordinal":"R1","receipt_sha256":null,"run_id":"22269f85c720e610b05071243230b211","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"f8e8799a4913807a205e300618fba997c1c5f91a","status":"reserved","terminal_code":null}
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"e4c2dc4b555ccf253fa977a5985ce06c0e4e1c5cd73088a609eddbbcf6b1fcb5","kind":"rehearsal","ordinal":"R1","receipt_sha256":"a562b88104c7785d1ea2648834ff379903ed23c05d71db7d18a5cf3e578f7633","run_id":"22269f85c720e610b05071243230b211","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"f8e8799a4913807a205e300618fba997c1c5f91a","status":"finished","terminal_code":"INCONCLUSIVE_DEPENDENCY"}
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"4287082f4ab94b5a28ef5d43941dbb9690578d2728819cc72298b29a82be15e6","kind":"rehearsal","ordinal":"R2","receipt_sha256":null,"run_id":"2fa2444f407b840726bc862948d7e68a","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"e361329d114089d62a509a9af5e875cb682641a5","status":"reserved","terminal_code":null}
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"4287082f4ab94b5a28ef5d43941dbb9690578d2728819cc72298b29a82be15e6","kind":"rehearsal","ordinal":"R2","receipt_sha256":"94d33953e2582230224452425dea9dca3968c9ddb010f7155e63b9353fd927d3","run_id":"2fa2444f407b840726bc862948d7e68a","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"e361329d114089d62a509a9af5e875cb682641a5","status":"finished","terminal_code":"INCONCLUSIVE_OWNERSHIP"}
```

## Stage traces

R1 ended at journal sequence 8 and digest
`4ea77d53b5f87c274e3b7ce3336fdb1a3b693e6d5c79aae7dcd183d334ef3677`.

| Seq | Stage | Producer | Deadline ms | Elapsed ms | Code |
|---:|---|---|---:|---:|---|
| 1 | preflight | runner | 10000 | 547 | `PREFLIGHT_PASS` |
| 2 | baseline | public-cli | 5000 | 717 | `BASELINE_CAPTURED` |
| 3 | stop | public-cli | 20000 | 901 | `PUBLIC_STOP_FAILED` |
| 4 | termination-proof | independent-ps | 15000 | 1022 | `TERMINATION_UNPROVED` |
| 5 | final-inventory | public-cli | 15000 | 1183 | `FINAL_INVENTORY_PROVED` |
| 6 | registry-hygiene | public-cli | 5000 | 1324 | `REGISTRY_HYGIENE_RECORDED` |
| 7 | root-removal | filesystem-witness | 20000 | 1478 | `ROOT_REMOVAL_UNPROVED` |
| 8 | terminal | court | 5000 | 1605 | `INCONCLUSIVE_DEPENDENCY` |

R2 ended at journal sequence 11 and digest
`be6e11296633872769284e4f8db312030db834383f6240d1ed7cfd9a08a2351a`.

| Seq | Stage | Producer | Deadline ms | Elapsed ms | Code |
|---:|---|---|---:|---:|---|
| 1 | preflight | runner | 10000 | 553 | `PREFLIGHT_PASS` |
| 2 | baseline | public-cli | 5000 | 733 | `BASELINE_CAPTURED` |
| 3 | session-ready | public-cli | 30000 | 1530 | `SESSION_READY` |
| 4 | connection-ready | public-cli | 20000 | 1714 | `CONNECTION_READY` |
| 5 | baseline | public-cli | 5000 | 1877 | `CURRENT_INVENTORY_PROVED` |
| 6 | stop | public-cli | 20000 | 2483 | `PUBLIC_STOP_SUCCEEDED` |
| 7 | termination-proof | independent-ps | 15000 | 2608 | `TERMINATION_NOT_APPLICABLE` |
| 8 | final-inventory | public-cli | 15000 | 2781 | `FINAL_INVENTORY_PROVED` |
| 9 | registry-hygiene | public-cli | 5000 | 2929 | `REGISTRY_HYGIENE_RECORDED` |
| 10 | root-removal | filesystem-witness | 20000 | 3113 | `ROOT_REMOVAL_PROVED` |
| 11 | terminal | court | 5000 | 3243 | `INCONCLUSIVE_OWNERSHIP` |

R1 observed baseline/current/final live cardinalities `0/not-run/0` and
registry `visited=0`. R2 observed `0/1/0` and `visited=1`. R2 proved public
stop, final inventory restoration, and root removal. It did not prove V3a
exact-process termination, so aggregate V3 remained failed despite no observed
live inventory or owned-root residue.

R1 criteria were V1 and V4b `pass`; V3, V3a, and V3b `fail`; all others
`not-run`. R2 criteria were V1, V4a, V4b, and V3b `pass`; V3 and V3a `fail`;
V2 and every G criterion `not-run`. Thus the decision tree stopped before D1;
A0, G4a, G4c, and G7 were never evaluated.

Two result items required by the decision-run-oriented specification are
structurally unavailable rather than omitted. First, there is no successful
rehearsal receipt: the two actual receipt digests are listed above, but neither
finished with `REHEARSAL_PASS`. Second, there is no ownership proof: R1 stopped
before session start and R2 stopped before an ownership stage row was
persisted. The cleanup facts were exactly:

| Attempt | owner absent | browser absent | host absent | group absent | process observations | ps rows | root absent | root removal succeeded |
|---|---|---|---|---|---:|---:|---|---|
| R1 | false | false | false | false | 0 | 0 | true | false |
| R2 | false | false | false | false | 0 | 0 | true | true |

For R2, the three exact-process `false` values do not prove that those objects
remained alive. The probes were not run because cleanup unnecessarily coupled
them to the unavailable frozen process-group value; their zero observation
counts record that missing proof.

## Failures and deviations

R1 derived the public session name as `profile-binding-v2-` plus its
32-character run id: 51 ASCII bytes. The public validator permits at most 32
bytes. Non-empty, first-character, and character-set constraints all passed;
length alone failed. The forward lifecycle path stopped after baseline without
persisting the session-start subcause; the journal then continued through
cleanup and terminal rows. The permitted source repair derived an exactly
32-byte name, mirrored all four public constraints in no-side-effect preflight,
added a redacted session-start failure row, and disambiguated inapplicable
cleanup.
That repair changed the input digest and was retested as R2. It did not alter a
criterion after observing decision data; no decision-stage data was observed.

R2 confirmed the name repair: session start, one bridge connection, and current
inventory succeeded. It then failed inside `ownership_facts` before an
ownership row was persisted. The artifacts prove that boundary and typed class
but not the first internal exception. Separate read-only source and host
inspection identified a deterministic court defect: the independent `ps`
parser requires `sid > 0` for every snapshot row, while all 1,313 rows produced
by the host legally reported `sess=0`, including the first row for PID 1. If the
three preceding public process observations succeed, that assertion therefore
must reject; in any event this implementation cannot pass ownership on this
host. The frozen specification required equal nonzero session ids only for the
browser and host, not every system row. This is a court assertion defect, not
an environment failure. It is not presented as the precise first exception
proved by the journal.

Three deviations weakened third-party diagnosability, not verdict correctness—no
design verdict exists:

1. Both rehearsals could fail before their intended stage row persisted a fixed
   redacted subcause. Their terminal classes and accounting are correct, but a
   third party cannot derive each precise cause from the products alone. The R1
   repair covered session start rather than every same-level pre-stage throw;
   the independent review likewise failed to demand that generalization.
2. R2 had exact session and connection identities, but cleanup coupled those
   identities to a process-group value that ownership had not returned. It
   skipped still-executable owner/browser/host absence probes and recorded V3a
   as not applicable. This prevents an exact-object termination proof even
   though stop, final inventory, and root removal succeeded.
3. The recorded browser and executable digests cannot be independently
   recomputed from this tracked result because the exact tested binaries were
   not archived with it. The input digest is reproducible from tracked inputs,
   but the browser, `agenterm`, and `agenterm-cu` digests remain recorded run
   facts rather than durable third-party reproduction commands.

D1 is independently blocked twice: R1 and R2 both have immutable reserved rows,
so the rehearsal budget is exhausted; and no finished row has terminal code
`REHEARSAL_PASS`, so no matching rehearsal exists. This is machine enforcement,
not voluntary restraint. Any future work requires a new precommitment and new
budget; this harness must not be rerun or repaired in place.

Before R1, a browser-version whitespace disagreement was caught by the
no-side-effect V1 handshake before reservation. That zero-budget refusal is
positive evidence that pre-admission validation protected formal budget.

## Reproduction boundary

The local external receipts can be checked without rerunning the experiment:

```sh
shasum -a 256 .agenterm-research-state/browser-profile-name-binding-v2/rehearsal-1-receipt.json
shasum -a 256 .agenterm-research-state/browser-profile-name-binding-v2/rehearsal-2-receipt.json
wc -l .agenterm-research-state/browser-profile-name-binding-v2/stage-journal/rehearsal-1.jsonl
wc -l .agenterm-research-state/browser-profile-name-binding-v2/stage-journal/rehearsal-2.jsonl
```

Expected receipt digests and row counts are respectively
`a562b88104c7785d1ea2648834ff379903ed23c05d71db7d18a5cf3e578f7633`
with 8 rows, and
`94d33953e2582230224452425dea9dca3968c9ddb010f7155e63b9353fd927d3`
with 11 rows. The ledger above is its durable tracked copy; external state
remains authoritative locally. There is intentionally no reproduction run
command because both rehearsal ordinals are consumed and D1 is ineligible.
The input digest can be independently recomputed from the ten tracked inputs by
the domain-separated algorithm in `run-current-host.sh`; it was independently
recomputed as
`4287082f4ab94b5a28ef5d43941dbb9690578d2728819cc72298b29a82be15e6`
for R2. No equivalent durable command can recompute the three executable
digests after the exact tested binaries cease to be available; that limitation
is recorded above as a deviation.

## Implementation self-test incident

During runner development, the first `--self-test` exposed a state-root
override capture bug. It wrote one 410-byte synthetic `R1` `reserved` row and
an empty lock into the formal broker namespace. The unmistakable test values
were `a` and `b` sentinel digests plus run id
`0123456789abcdef0123456789abcdef`. It created no journal row, receipt, browser
process, product state or GUI activity.

The row was read back before cleanup, then those two exact files, the empty
`stage-journal` directory and the empty experiment state directory were
removed. No sibling external state was touched. This was namespace pollution
and is recorded here rather than hidden. It was not a formal `R1`: the
implementation was not frozen, the values were sentinels, and no real V1
preflight or admission handshake occurred.

For durable audit, the deleted ledger line was exactly 410 bytes including its
trailing newline and had the following complete canonical content:

```json
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"rehearsal","ordinal":"R1","receipt_sha256":null,"run_id":"0123456789abcdef0123456789abcdef","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"reserved","terminal_code":null}
```

Cleanup removed that ledger, the exact zero-byte `state.lock`, the empty
`stage-journal` directory, and then the empty experiment directory. It did not
append `abandoned`: that transition belongs to an independently audited
admitted attempt, while this unfrozen synthetic write bypassed both real V1 and
admission. Preserving it in the formal ordinal namespace would therefore have
spent `R1` on an implementation self-test rather than an experiment attempt.

The capture bug is fixed. `--self-test` now asserts before and after the test
that the formal experiment directory does not exist, places all broker state
under a fresh temporary root, exercises reserve, journal/receipt publication,
finish, independently audited abandonment and state inspection there, and
removes the temporary root.

The frozen criteria remain unchanged in
`plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md`.
