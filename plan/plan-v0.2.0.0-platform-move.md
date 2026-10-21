# AgenTerm platform-move — Phase E (agenterm depends on MiniCon)

Status: **implemented and locally committed on `main` (unpushed); Windows
build/fmt/clippy/unit courts green, five pre-existing unrelated failures
recorded.**
Parent: [`plan-v0.2.0.0.md`](plan-v0.2.0.0.md) ·
Mirror leaf: MiniCon `plan/plan-v0.2.5.md` `{PLATFORM-MOVE}` (which owns the
MiniCon side and named this side "Phase E, tracked separately").
PRD: [`PRD.md`](../PRD.md) · Structure SSOT:
[`plan/ARCHITECTURE.md`](ARCHITECTURE.md).

## Scope decision

Owner (2026-10-10/11): the shared platform layer lives in **MiniCon** and
`agenterm` depends on MiniCon — not the reverse. MiniCon has already moved,
renamed and pushed the four units:

- `agenterm-platform`  → MiniCon `crates/minicon-platform`  (crate
  `minicon-platform`,  Rust path `minicon_platform`),  published.
- `agenterm-ui-core`   → MiniCon `crates/minicon-ui-core`   (crate
  `minicon-ui-core`,   Rust path `minicon_ui_core`),    published.
- the `vt100` fork     → MiniCon `third_party/vt100`     (package name `vt100`).
- the `softbuffer` fork → MiniCon `third_party/softbuffer` (package name
  `softbuffer`).

This leaf is the `agenterm` side: consume the MiniCon crates through a pinned
git revision and delete this repository's own copies. It supersedes the
dependency direction recorded in `PRD.md` and `plan/ARCHITECTURE.md` before
2026-10-11 ("reuse runs minicon → agenterm and never the reverse").

## Pin

| Unit | Source |
|---|---|
| `minicon-platform` | `git https://github.com/partnernetsoftware/minicon` rev `0326e2abaf71cb2f8dc22f766de9b410efa4be1e` |
| `minicon-ui-core`  | same url, same rev |
| `minicon-core` | same url, same rev (incremental pure-rule consumption below) |

The two platform/ui crates are byte-identical across MiniCon 0.2.5 and its
predecessor revs, so the pin is stable for this move.

**Correction (found during implementation):** the pinned MiniCon 0.2.5 platform
crate is **not** byte-identical to agenterm's deleted copy. Diffing
`HEAD:crates/agenterm-platform` against the checkout shows 33 files differ;
after normalising the `agenterm-platform` → `minicon-platform` string rename,
6 files carry real MiniCon-side changes that landed after agenterm's copy was
taken:

- `adapters/windows/console_agent.rs` — the MiniCon `{TITLEFIX}` console title
  fix (`GetConsoleTitleW` → `announce_console_title` → OSC 0), plus its unit
  test; this is the fix recorded in MiniCon `plan/plan-v0.2.5.md` `{TITLEFIX}`.
- `adapters/windows/contained_process.rs` — MiniCon `{CU-TESTFIX}` (a) dead-handle
  test fix and (b) the `TerminateProcess` idempotency fix.
- `adapters/windows/pty.rs`, `adapters/windows/font.rs`, `locking.rs` — the
  MiniCon `{CLIPPY-WS}` lint fixes.
- `contract/window_host.rs` — a new `#[cfg_attr(not(unix), allow(dead_code))]`
  on a Unix-only constructor.

Consuming 0.2.5 therefore *imports those fixes into agenterm* rather than
reproducing the old bytes exactly. That is the intended direction (MiniCon owns
the platform layer) but it is a real delta the reviewer must accept; it is not a
mechanical rename. The boundary court in `src/platform/boundary_tests.rs` was
taught the one new predicate by the sanctioned `NATIVE_SELECTION_EXCEPTIONS`
route. If the owner wants agenterm frozen at its old platform bytes, the pin
must move to the earlier MiniCon rev this copy came from, and the title/CU lint
fixes must be re-applied there.

## What changed

- `Cargo.toml` (workspace root): `members` no longer lists
  `crates/agenterm-platform` or `crates/agenterm-ui-core`; the two root deps
  became git deps keeping every `features` entry and `default-features` flag.
- Per-crate deps rewritten path → git with identical feature lists:
  `crates/agenterm-abi/Cargo.toml`, `crates/agenterm-chassis/Cargo.toml`
  (incl. `loader = ["dep:minicon-platform"]`), `crates/agenterm-control-client/Cargo.toml`,
  `crates/agenterm-cu/Cargo.toml` (base + windows + linux targets),
  `crates/agenterm-qjswasm/Cargo.toml`, `examples/size-probe/Cargo.toml`.
- Every `agenterm_platform` → `minicon_platform` and `agenterm_ui_core` →
  `minicon_ui_core` use/`extern crate`/doc-link/cfg occurrence in tracked
  source and live docs; the hyphenated crate names likewise. Frozen history
  (`plan/archive/`, `prd/archive/`, `archive/`, `research/`, release records)
  keeps the old spelling on purpose — it is history, not current truth.
- `crates/agenterm-platform/` and `crates/agenterm-ui-core/` deleted.

## Fork decision — keep local `third_party/` copies (option a)

MiniCon's fork copies each carry their own `[workspace]` marker, so they are
not part of MiniCon's workspace and a Cargo **git** patch to the MiniCon repo
cannot resolve them by package name. The two workable options were:

(a) keep `agenterm/third_party/{vt100,softbuffer}` and the local path
`[patch.crates-io]`, duplicating the fork in both repos this round; or
(b) some git-based patch that resolves MiniCon's copies without touching the
MiniCon repo (frozen — a release chain is running there).

**Chosen: (a).** A `[patch.crates-io]` **path** patch at a workspace root
applies to the *whole* dependency graph, including packages reached through a
git dependency. So the local patches keep patching `vt100` (col-wrap row
bookkeeping fix) and `softbuffer` (macOS display colour-space fix) for both
this repo's own use and the MiniCon crates pulled from git. `Cargo.lock`
confirms it: the `vt100` / `softbuffer` entries carry no `source =`, i.e. they
resolve to the local path forks.

Why not (b): a committed path pointing at `../minicon/third_party/...` is a
sibling checkout that a clean clone or CI does not have, and a git patch cannot
name those packages (own-workspace marker). (a) needs no change to MiniCon,
stays reproducible from this repository alone, and keeps both fixes live. Cost:
the fork is duplicated in the two repos for now.

## Evidence

- `Cargo.lock` locks `minicon-platform` / `minicon-ui-core` at the pinned git
  rev; no `agenterm-platform` entry remains; `vt100` / `softbuffer` have no
  `source` (local path patch intact).
- build/court result on this host: see "GATE status" below.

## GATE status (host: Windows, x86_64; `AGENTERM_FORCE_CONSOLE_AGENT` cleared)

Host: Windows, x86_64. `AGENTERM_FORCE_CONSOLE_AGENT` was set to `1` in the
shell and cleared for the runs below (the documented cause of the
`direct_conpty_*` platform failures — identical on unmodified main).

```text
cargo fmt --all -- --check                                            -> clean
cargo clippy --locked --all-targets --all-features -- -D warnings     -> exit 0
cargo test -p agenterm --all-features --lib                           -> 853 passed; 2 ignored
cargo test -p minicon-platform --lib -- --skip threading::            -> 624 passed; 1 ignored
cargo test -p agenterm --all-features --test release_workflow_policy  -> 119 passed
primary unit court (the exact check.qjs command, denials + skip set)  -> see below
```

The primary court (`cargo test --locked --all-features -p agenterm` with the
`-D dead_code/unused_variables/unused_must_use` rustflags and check.qjs's skip
set) passes every court **except five pre-existing failures that are unrelated
to this move** — each one's input file is byte-identical to `HEAD` (verified with
`git diff HEAD -- <input>` = empty, and the requiring logic is unchanged):

- `authenticode_inspector_policy::public_policy_names_the_signed_boundary_and_authority_split`
  — `CODE_SIGNING_POLICY.md` says "v0.1.19 assets are unsigned"; the test still
  expects "v0.1.16".
- `chassis_live_first_window::checked_image_dispatches_l2_against_real_pty_and_ipc_host`
  — `HEAD`'s `src/frontend/chassis_image.rs` already requires
  `l2/programs/adjacent-tab.json`, but `HEAD`'s test fixture never writes it.
- `chassis_loader_workflow_policy` (3 tests) — CI step names drifted in
  `.github/workflows/ci-chassis.yml.disabled` / `candidate.yml` (unmodified).
- `macos_signing_qualification_policy::candidate_supplies_every_value_the_macos_signer_requires`
  — `candidate.yml` has no `AGENTERM_APPLE_TEAM_ID` (unmodified).
- `windows_signing_resource_policy::abi_resource_is_cdylib_only_and_cross_build_has_a_pinned_compiler`
  — Windows ARM64 resource-tooling step absent from the unmodified workflows.

agenterm's `main` is therefore already red at `HEAD`; this move adds no new
failure. A single-host run cannot make these green without editing unrelated
release/chassis policy, which is out of scope for the platform move.

CI-only / BLOCKED: Linux and macOS cells are not buildable on this host; they
are validated by CI, not claimed here.

## Carried / non-goals

- Internal `AGENTERM_*` macro / env-var names inside the copied platform source
  are not renamed (huge, risky churn; MiniCon carries the same item).
- Live docs outside `PRD.md` / `plan/ARCHITECTURE.md` that described the old
  ownership in prose are not all re-written this round; the token rename keeps
  every crate identifier valid, and remaining prose drift is a documentation
  follow-up, not a build dependency.
- Frozen history under `plan/archive/`, `prd/archive/`, `archive/`, `research/`
  and `docs/release-*record*` is deliberately left with the old spelling.
- `scripts/apply_login_session_journey.py` still edits
  `crates/minicon-platform/src/**` by path and is now obsolete in this
  repository (the platform source is external); it is not run by the gate and is
  left for a follow-up rather than rewritten to reach into another repo.
- `plan/chassis-l1-surface.json` keeps its `crates/minicon-platform/**` entries.
  They can never appear in an agenterm diff now, but the surface still documents
  the L1 tax and the classifier tests exercise them as hypothetical inputs; the
  path-existence court resolves them against the pinned checkout.

## Incremental core consumption

2026-10-18: continue the existing agenterm -> minicon dependency direction by
consuming `minicon-core` at the same immutable SHA as platform/ui. No sibling
path dependency, MiniCon revision bump or MiniCon working-tree source is used.
Workspace `[patch.crates-io]` still selects the local vt100/softbuffer forks;
core adds only already-present itoa/unicode-width production dependencies.

The first bounded slice is `src/frontend/text_selection.rs::line_bounds`, used
by the Unix composer's triple-click selection. It adapts AgenTerm character
offsets to the core's `composer::line_index_at`/`line_range` byte contracts, then
returns character boundaries. This removes the local temporary `Vec<char>` and
leaves native Windows Edit handling unchanged. Product truth belongs to
[Human workspace](../prd/PRD_02_06_human_workspace.md#shared-core-reuse-boundary);
source ownership belongs to [ARCHITECTURE](ARCHITECTURE.md#1-分层验收尺).

Remaining consolidation items, in dependency order:

1. Compare pure composer helpers individually before migrating further. The two
   products differ in cursor units, tokenization, editing/history and limits;
   importing the complete MiniCon composer would change product behavior.
2. Compare click semantics before reuse. MiniCon cycles 1/2/3/1 at a fixed
   500 ms; AgenTerm has native Windows double-click routing and a future live
   host interval policy. Sharing a constant alone does not unify those rules.
3. Keep AgenTerm tree preorder/orphan recovery/collapse/cycle policy local.
   MiniCon core's typed depth calculation is strict about malformed parentage
   and duplicates, so it is not an equivalent replacement for `tab_tree`.
   Resolve core/UI tree and core/platform numeric ownership in MiniCon before
   migrating additional consumers to either duplicate export.
4. The version module is absent at this pin. Consider it only after an
   independently reviewed immutable revision contains it. The MiniCon bounded
   JSON codec is not a drop-in serde_json replacement for AgenTerm's protocol.

Validation for this incremental slice is recorded below with exact host scope;
peer-platform GUI behavior is not inferred from a Windows unit result.

Host evidence: Windows x86_64, Rust 1.97.0, task-local
`target/minicon-core-reuse` lane:

- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --locked --offline -p agenterm --all-targets --all-features
  -- -D warnings`: PASS on the host target, including the MiniCon core consumer.
- `cargo test --locked --offline -p agenterm --lib --no-default-features
  frontend::text_selection::tests`: 15 passed (14 existing plus one new test).
  The new test sweeps all 3,906 strings of length 0..5 over ASCII, CJK, emoji,
  CR and LF, every character position, one-past-end and `usize::MAX`.
- Negative control: temporarily returning the core's byte boundaries directly
  makes the new test fail for a one-character CJK buffer, returning `(0, 3)`
  instead of `(0, 1)`. Restoring the character conversion returns all 15 tests
  to green. The temporary mutation is absent from the final source.
- The narrow no-default-features root test build reports existing unused/dead
  script-engine warnings; those modules were not changed by this slice.
- `scripts/doc-redact-check.sh` on the three edited Markdown files: PASS;
  `git diff --check`: PASS.
- Linux/macOS GUI triple-click and ARM64 runtime evidence: BLOCKED on this
  Windows x86_64 host. No GUI black-box or six-cell claim is made by this slice.
