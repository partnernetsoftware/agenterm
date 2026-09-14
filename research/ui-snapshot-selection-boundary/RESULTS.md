# ui-snapshot selection boundary result

## Verdict

The text-reparse adapter **T is admitted** to a separate product implementation
leaf. S0, W0 and H0 all passed. This experiment adds no public option and makes
no capability-state claim.

The result is intentionally narrower than “ship selection”: `ui-snapshot` is
always JSON, so a later leaf must expose selection without inventing `--json`,
must reuse the one selector grammar and typed refusals, and must preserve the
current cached text byte for byte when no selector is present.

## Frozen environment

| field | value |
|---|---|
| source | `d249df8bf472f69529b8dafeade618446d68c882` plus the ignored reporter diff recorded with this result |
| host | macOS 26.5.1, arm64 |
| Rust | `rustc 1.97.0 (2d8144b78 2026-07-07)` |
| Cargo | `cargo 1.97.0 (c980f4866 2026-06-30)` |
| warm-ups | 100 per document |
| measured iterations | 1,000 per sample, three samples per complete run |
| complete runs | 3 |
| allocations | **未测定** |

The exact envelope and extracted-document SHA-256 values are in
`snapshot-manifest.json`. The documents were not copied: their immutable paths
already belong to the frozen host-reply experiment, and the manifest binds both
the outer evidence and the exact extracted bytes.

## Raw result summary

| document | selector | full bytes | selected bytes | ratio | median range | p95 range |
|---|---|---:|---:|---:|---:|---:|
| rendered-one-tab | `event_position` | 13,075 | 89 | 0.681% | 245,917–248,458 ns | 259,750–277,625 ns |
| rendered-three-tabs | `tabs[].id,tabs[].render.text` | 18,310 | 690 | 3.768% | 365,458–368,500 ns | 382,750–388,416 ns |

All nine median/p95 pairs for each document are preserved in
`measurements.json`; the table does not hide run spread behind one aggregate.

## Decision trace

1. **S0 passed.** The prototype returns `Cow::Borrowed(source)` immediately
   when no selector exists and asserts exact byte equality. The selected path
   uses the production `Selector`. The 12 ordinary `json_select` tests passed,
   covering exact paths, missing paths, retained subtrees, duplicate and shallow
   precedence, malformed selectors and the stable refusal code. Source audit
   confirms `ControlHost::ui_snapshot_json` supplies the same text seam for the
   cached snapshot and locally serialized fallback; a later adapter can branch
   before parsing without changing either no-selector producer.
2. **W0 passed.** 0.681% and 3.768% are both below the precommitted 25% ceiling.
3. **H0 passed.** The worst observed p95 was 388,416 ns (0.388416 ms), below
   the precommitted 1.0 ms ceiling.
4. The §4 tree therefore reaches **T admitted to a separate product
   implementation leaf**. V is not required by this boundary measurement.

M0 is reported above and in `measurements.json`. Host allocation count/bytes is
**未测定** because no existing allocation reporter served this Rust boundary.
C0 is **未测定** for the future product leaf: the experiment changed zero
production files, and estimating code that was deliberately not implemented
would be false precision.

## Deviations and corrections

Before any measurement, the spec's “headless plus rendered” input assumption
was corrected: the frozen reply set contains two real rendered snapshots at
different sizes and no headless snapshot. The cost axis therefore uses those
two real inputs, while local-versus-cached producer ownership remains an S0
source audit. No criterion was relaxed.

The evidence layout reuses the existing immutable envelope files instead of
copying their extracted stdout into a second research directory. Both levels
are hash-bound in `snapshot-manifest.json`. The reporter remains default-ignored
so another host can reproduce the measurement without turning elapsed time into
a unit-test assertion.

## Third-party rerun

From repository root:

```bash
CARGO_TARGET_DIR=target/ui-snapshot-selection-boundary cargo test --lib json_select::tests
CARGO_TARGET_DIR=target/ui-snapshot-selection-boundary cargo test --lib json_select::tests::measure_ui_snapshot_text_reparse_boundary -- --ignored --nocapture
```

Run the second command three times. Compare every emitted
`UI_SNAPSHOT_SELECTION_MEASUREMENT` object with `measurements.json`. The first
command is the semantic/refusal negative control; elapsed time is judged only
from the explicit ignored reporter.

## Honesty clause

This experiment does not prove end-to-end guest savings, Windows workbench
behavior, allocation cost, cross-host latency, or the final public spelling.
The admitted product leaf must run one successful public journey before and
after at one source state, preserve the bare `ui-snapshot` bytes, and keep
product-internal `wait-ui` migration out of scope.
