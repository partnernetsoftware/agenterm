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
| Build | `./build.bat` | `./build.sh` |
| Quick | `./check.cmd --quick` | `./check.sh --quick` |
| CI-grade | `./check.cmd --skip-smoke` | `./check.sh --skip-smoke` |
| Full | `./check.cmd` | `./check.sh` |
| Release rehearsal | `./release.cmd --rehearse` | `./release.sh --rehearse` |

Run cheap formatting and lint while iterating, then owning unit and public
black-box courts, then integrated gates. Reserve stress, packaging, and exact-
byte qualification for their named release boundary. Root aliases stay thin;
do not add unmatched `.cmd` or `.bat` behavior, restore a global Cargo job
limit, or delete `.cargo/config.toml`. All smoke tests honor
`AGENTERM_NO_ACTIVATE=1`.

Use public CLI and wait operations for runtime tests, isolated IPC/workspace
values, and stable tab IDs. Avoid fixed sleeps. Rendering investigations need
both structured state and image evidence.

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
