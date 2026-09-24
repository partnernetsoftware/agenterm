# AgenTerm agent guide

This file contains only repository-wide rules that agents must see before
working. Start product work at `PRD.md`, follow its owning `prd/PRD_*.md`, and
use `plan/ARCHITECTURE.md` as the only current source-layout map. Plans belong
in `plan/`; machine alignment belongs in `prd/alignment-contract.json`.

## Hard rules

### Protect written artifacts

Never write host-home or expanded checkout paths, real credentials, personal
contact details, IP or MAC addresses, or personal hostnames into repository
content, commits, screenshots, prompts, or handoffs.

- Use repository-relative paths inside the clone and `~/...` under any home.
- Use placeholders such as `<TOKEN>`, `<IP>`, and `<HOST>` in examples.
- Fixed synthetic absolute paths are allowed only in path-parser unit tests.
- Run `./scripts/doc-redact-check.sh path/to/file` after relevant writes and
  rewrite every hit.

### Respect the shared checkout

- Everyone works in one `main` checkout. Do not create worktrees, task
  branches, stashes, or hidden copies, and do not use checkout/reset to discard
  state.
- Inspect `git status` before editing. Existing changes belong to their owner.
- Give delegated work exclusive files and a bounded deliverable. Agents return
  edits unstaged with changed files, tests, findings, and assumptions.
- Serialize edits to shared files and Cargo work using the same target tree.
- The primary agent reviews and stages exact paths. Do not commit generated
  binaries.

Named tmux executor supervision is documented in
`docs/tmux-executor-supervision.md`.

### Preserve authority boundaries

- Product code does not import raw OS APIs. Neutral contracts belong in
  `crates/agenterm-platform`, adapters own OS calls, and shared product modules
  own behavior.
- The Script Runtime is an unrestricted local runtime, not a permission system.
  Do not add path, process, endpoint, credential, or tool allowlists to script
  profiles, catalogs, registration, or brokers. Resource limits, typed failures,
  and cleanup are robustness controls. Legacy `profile` fields are inert.
- Public release promotion requires human authority. Local release commands
  validate or rehearse only.
- Before asking the user to decide or approve a step, review the current task,
  prior authorization and concrete evidence with the strongest available
  independent model when one is callable. The review classifies the step as
  already authorized, a routine implementation choice, missing information,
  or a genuine human authority boundary. Continue without asking for the first
  two. A model review is advice and never grants GitHub Environment approval,
  public Promotion authority or access that the executor lacks. If no
  independent model is available, make the same classification directly rather
  than stopping merely because review could not run.
- Preserve terminal tree safety, remain-on-exit, and explicit-close invariants.
  Unsupported tmux/RMUX behavior must fail explicitly.

## Before editing

For material work, identify the outcome, owner, dependencies, shared files,
public evidence, safe failure, and final validation path. Parallelize only
independent work with exclusive ownership. Keep one current truth in the owning
PRD; do not create another roadmap or source map.

Before Rust, Cargo, FFI, unsafe, SIMD, PTY, IPC, filesystem publication,
rendering, platform, or build changes, read
`docs/agenterm-rust-cheatsheet.md`. It owns feature-isolation, target-matrix,
unsafe/ISA, Cargo-lane, build-profile, and recurring engineering rules.

Before writing `.qjs`, read `crates/agenterm-qjswasm/README.md`. The entry
extension selects the engine; `AGENTERM_SCRIPT_BACKEND` is an explicit
override. Rhai and old qjs manuals are historical only. Use bounded
`check-many --manifest` for repository-wide validation and direct `check` for
single-file diagnostics.

Before Candidate, Promotion, release authentication, dispatch, or monitoring,
read `skills/agenterm-release/SKILL.md` and its required references.

### Platform crate vs product UI — shared-first

For shared UI work, put product semantics in shared modules before host
adapters. Update `src/frontend/ui_action_catalog.rs` for cross-host actions or
record a genuine host-only `parity-gap:`. Ownership and parity references live
in `plan/ARCHITECTURE.md`, `plan/plan-platform-encapsulation-gap.md`, and
`plan/platform-ux-parity-evidence-matrix.md`.

## Validation

Use the host-matching root aliases:

| Purpose | Windows | Linux/macOS |
|---|---|---|
| Lint | `./lint.cmd` | `./lint.sh` |
| Build | `./build.bat` (with a lane, below) | `./build.sh` (with a lane, below) |
| Quick | `./check.cmd --quick` | `./check.sh --quick` |
| CI-grade | `./check.cmd --skip-smoke` | `./check.sh --skip-smoke` |
| Full | `./check.cmd` | `./check.sh` |
| Release rehearsal | `./release.cmd --rehearse` | `./release.sh --rehearse` |

The build task is declared to receive exactly two isolation variables, so a
bare build alias answers `task_environment_missing` and exits 2 before it
writes anything. Name a repository-local lane instead:

```powershell
$env:AGENTERM_BUILD_DIST_DIR = 'dist/<lane>'
$env:CARGO_TARGET_DIR = 'target/<lane>'
./build.bat <profile>
```

or on Linux/macOS:

```sh
AGENTERM_BUILD_DIST_DIR=dist/<lane> CARGO_TARGET_DIR=target/<lane> ./build.sh <profile>
```

Both values must be direct children of the repository's `dist/` and `target/`
(a bare `dist` or `target` is refused, and so is a symlink). Read the artifact
paths off the build's own output instead of assuming them: it lists every
artifact under a `Built client artifacts [<profile>, <os>-<arch>]:` header, one
path per line -- in this checkout a `dev` lane reported `target/<lane>/debug/`,
and a lane building for another target can carry a target-triple level as well.
Only the native Windows lane stages the shipped names under `dist/<lane>/`.

Run cheap formatting and lint while iterating, then owning unit and public
black-box courts, then integrated gates. Reserve stress, packaging, and exact-
byte qualification for their named release boundary. Root aliases stay thin;
do not add unmatched `.cmd` or `.bat` behavior, restore a global Cargo job
limit, or delete `.cargo/config.toml`. All smoke tests honor
`AGENTERM_NO_ACTIVATE=1`.

Use public CLI and wait operations for runtime tests, isolated IPC/workspace
values, and stable tab IDs. Avoid fixed sleeps. Rendering investigations need
both structured state and image evidence.

### Evidence discipline

These rules are not style. Each one is here because its absence produced a
result that looked green and proved nothing.

- **Every test must be falsifiable.** Before you trust a new test, change the
  code it guards and watch it fail. A test that still passes with its guard
  removed is documentation, not evidence. When you add one, confirm the gate's
  reported test count actually went up — a test that loses its `#[test]`
  attribute compiles as dead code and silently runs nothing.
- **A green build is not a compiled module.** Feature unification, default
  feature sets and `cfg` gates all let a check pass over code that was never
  built. `docs/agenterm-rust-cheatsheet.md` §2 owns the rule; the practical
  form is to name the package, features and targets your evidence covers and to
  claim nothing outside them. The `agenterm-platform` crate is the sharpest
  case: its default features omit `pty`, so whole modules do not compile in a
  default check, and a consumer that enables them is the only real proof.
- **One platform's evidence is never a cross-platform claim.** Record a result
  as belonging to the OS, ISA and host it was taken on. Evidence you could not
  obtain is `BLOCKED` with the reason — never a silent skip, and never a PASS
  inferred from the platform next to it.
- **A measurement is void unless the probe provably touched its subject.** A
  probe must assert it is attached to the thing it measures before its result
  counts; otherwise a redirected handle, a stale binary or an unattached
  console yields a confident answer about nothing. Keep a negative control
  beside every positive result: remove the fix, and watch the evidence
  disappear.
- **Never truncate a gate's output**, and when a gate fails, fix the input
  rather than relaxing the gate. If a gate must change, that is an owner
  decision recorded as such.
- **Fix the class, not the instance.** A reported layout, hit-test or geometry
  bug is a missing invariant. Close it with an invariant swept over the whole
  input space — see `LAYOUT_SWEEP` and the invariant tests in
  `src/ui_geometry.rs`, which run every window size and chrome scale through
  band tiling, containment, disjointness and "the last terminal row is never
  captured by host chrome" without needing a GPU or a window.

### Skills: look them up before acting

Signing, the release chain, the VM courts and terminal-IO diagnosis all have
registered skills under `~/.claude/skills/`, alongside this repository's own
`skills/`. Invoke the owning skill first — do not reconstruct a procedure from
memory or from reading a workflow file. **A skill's top-level page is only its
index; the operational detail lives in its `references/`.** When a task teaches
something durable, write it back into the owning skill, not only into notes.

GitHub Actions observation is bounded and read-only: one observer retains one
run and attempt, backs off while unchanged, and fetches details only when
needed. Never extract or repurpose credentials. If mutation authority is
unavailable, report the immutable run identity and required human action.

## Product invariants worth keeping visible

- Keep parsing, protocol, settings, and geometry out of native UI state
  machines and cover them with unit tests.
- Exercise shipped behavior through the public CLI.
- Update the owning PRD when capability state changes.
- Keep README files human-facing and brief.
- `agenterm.exe` remains the Windows GUI; `agenterm.com` is the minimal console
  forwarder. Mux and MCP are `agenterm cli` subcommands, not separate products.
- Do not claim full tmux/RMUX compatibility: one AgenTerm tab is currently one
  pane.
