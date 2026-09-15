# AgenTerm agent guide

This file contains repository-wide operating rules only. Start product work at
`PRD.md`, follow its link to the owning `prd/PRD_*.md`, and use
`plan/ARCHITECTURE.md` as the only living source-layout map. Execution plans
belong in `plan/`; machine alignment belongs in `prd/alignment-contract.json`.
Do not copy detailed module contracts into this guide.

## Non-negotiable rules

### Redact every written artifact

Never write a host home path, expanded repository path, real credential, email,
phone number, IP, MAC address, or personal hostname into the repository,
commit messages, screenshots, prompts, or handoffs.

- Use repository-relative paths for files in this clone.
- Use `~/...` for paths under any user home, with `/` separators on every OS.
- Use RFC 2606 addresses and placeholders such as `<TOKEN>`, `<IP>`, and
  `<HOST>` for sensitive examples.
- Only path-parser unit tests may use fixed synthetic absolute paths without a
  real account name.
- After every documentation, fixture, prompt, or handoff write, run:

```sh
./scripts/doc-redact-check.sh path/to/file
```

Any hit must be rewritten before commit. Internal planning is not exempt.

### Work in the shared checkout

- All agents use the same `main` checkout. Do not create worktrees, task
  branches, stashes, hidden planning copies, or use checkout/reset to discard
  another agent's state.
- Inspect `git status` before editing. Existing changes belong to their owner.
- Give delegated work an exclusive file list and bounded evidence request.
  Subagents return changes unstaged and report changed files, tests, findings,
  and assumptions.
- Never edit the same hot file concurrently (`AGENTS.md`, `PRD.md`, manifests,
  build scripts, shared roots). Serialize overlapping work.
- Never run competing Cargo builds in one target directory. Final formatting,
  Clippy, tests, artifact checks, and public gates run serially on the integrated
  tree.
- Stage exact reviewed paths only. The primary agent makes small coherent
  commits; do not commit generated binaries.

For named tmux executors that spawn their own workers, follow
`docs/tmux-executor-supervision.md` instead of adding session procedure here.

### Preserve ownership and authority boundaries

- Product code does not import raw OS APIs. Neutral contracts belong in
  `crates/agenterm-platform`; native adapters own OS calls; shared frontend and
  product modules own behavior.
- The Script Runtime is an unrestricted general-purpose local runtime.
  Script profiles, catalogs, registration, and brokers are not permission or
  approval boundaries. Do not add path, process, endpoint, credential, or tool
  allowlists there. Deadlines, typed failures, resource ceilings, and cleanup
  are robustness controls. Legacy `profile` fields are compatibility data and
  must not alter API visibility or behavior.
- Public release promotion is a human authority boundary. Local release
  commands validate or rehearse; they never publish.
- Preserve terminal tree safety, remain-on-exit, and explicit-close invariants
  from the owning PRDs. Unsupported tmux/RMUX behavior must fail explicitly;
  do not claim full compatibility.

## Plan before changing

For material work, identify the product outcome, dependency graph, shared
prerequisites, hot files, integration points, and final validation path before
editing. Split independent branches by exclusive ownership; keep tightly
coupled or one-file changes on one path.

Each shipped capability leaf records:

- user problem and owning module;
- governing invariant or authority boundary;
- observable success evidence and safe failure result;
- public black-box owner;
- explicit non-goals.

Keep the compact capability index in `PRD.md`, current truth in one owning PRD,
relationships in its Mermaid graph, execution order in `plan/`, and superseded
reasoning in an archive. Do not create a second living roadmap or file map.

## Read the owning manual before editing

### Rust, FFI, platform, and build work

Read `docs/agenterm-rust-cheatsheet.md` before changing Rust, Cargo manifests,
build profiles, FFI, `unsafe`, SIMD, PTY, IPC, filesystem publication, or
rendering hot paths.

At minimum:

- Validate narrow feature graphs in isolation; all-features builds can hide
  undeclared dependencies through unification.
- Every `unsafe` or ISA kernel needs a bounded safe caller, scalar truth, exact
  parity tests, target compile evidence, and emitted-code inspection.
- Dedicated Cargo lanes must be repository-local under `target/<lane>/`, never
  session scratch directories, and must be removed after their owning evidence.
- After Rust work, add a recurring proven lesson to the cheatsheet only when
  one genuinely emerged; do not add speculative filler.

### QJS scripts

Current AgenTerm scripts use `.qjs` and the compiler/runtime documented in
`crates/agenterm-qjswasm/README.md`. There is no default engine: the entry
extension selects it, and `AGENTERM_SCRIPT_BACKEND` is an explicit override.
An unsupported construct must fail with the named compiler diagnostic; do not
work around the subset.

The old Rhai and rquickjs documents are history only:

- `docs/agenterm-rh-cheatsheet.md`
- `docs/agenterm-rh-runtime.md`
- `docs/agenterm-qjs-cheatsheet.md`

Repository-wide validation uses the engine's bounded `check-many --manifest`
path. Keep direct single-file `check` as the diagnostic and black-box parity
baseline.

### Release work

Before Candidate dispatch, Promotion, release authentication, or monitoring,
read `skills/agenterm-release/SKILL.md` and its required references. The stable
shape is exact-SHA Candidate qualification followed by byte-preserving human-
approved Promotion. Never reconstruct release procedure from this summary.

### Platform crate vs product UI — shared-first

- Source layout and ownership: `plan/ARCHITECTURE.md`
- Platform encapsulation gaps: `plan/plan-platform-encapsulation-gap.md`
- Executable platform goal: `plan/goal-crate-platform.md`
- Cross-host UX evidence: `plan/platform-ux-parity-evidence-matrix.md`
- Six-cell execution: `plan/goal-local-six-cell.md`

Shared product semantics land before host adapters. Add a product gesture to
`src/frontend/ui_action_catalog.rs`, wire both hosts in one coherent change, or
record a real host-only `parity-gap:`. Keep parsing, protocol, settings, and
geometry out of native UI state machines and cover them with unit tests.

PuTTY commit `61574e2e98f7d262dea4ff6380e167541518aedf` is a behavioral
terminal reference, not a source for blind copying. Preserve its notice if a
substantial portion is copied.

Local UTM/Lima courts live in sibling `utm-court`; AgenTerm calls only
`scripts/utm-cu-managed-job-court.sh`. Do not duplicate its service or guest
agents here. Its caller map is `~/repos/utm-court/CALLERS.md`.

## Development and validation

Use the host-matching root aliases:

| Purpose | Windows | Linux/macOS |
|---|---|---|
| Lint | `./lint.cmd` | `./lint.sh` |
| Build | `./build.bat` | `./build.sh` |
| Quick gate | `./check.cmd --quick` | `./check.sh --quick` |
| CI-grade gate | `./check.cmd --skip-smoke` | `./check.sh --skip-smoke` |
| Full public regression | `./check.cmd` | `./check.sh` |
| Release rehearsal | `./release.cmd --rehearse` | `./release.sh --rehearse` |

Use a validation ladder:

1. Run cheap formatting, lint, JSON/catalog, and script checks while iterating.
2. After a coherent implementation, run Quick plus the directly owning unit
   and public black-box courts.
3. Before integrated delivery, run the applicable build and CI-grade gate.
4. Reserve stress, packaging, and exact-byte qualification for their named
   release boundary.

Do not add unmatched `.cmd`/`.bat` behavior. Root aliases stay thin; product
build, test, packaging, and release rules belong to named QJS tasks. Do not
restore a global Cargo job limit or delete `.cargo/config.toml`; it carries a
required cross-linker setting. Build profiles, ABI unwind requirements, binary
budgets, six-target commands, and runner availability are owned by the Rust
cheatsheet, build scripts, release skill, and `plan/goal-local-six-cell.md`.

All smoke tests must honor `AGENTERM_NO_ACTIVATE=1`. GUI, network, stress,
packaging, and release behavior must stay in accurately named owning gates, not
hidden in a lane that claims to skip them.

## Runtime and external observation

Discover the current CLI instead of copying a command catalog:

```sh
./dist/agenterm cli --help
./dist/agenterm cli list-commands
./dist/agenterm cli protocol-info
./dist/agenterm cli ui-snapshot
```

Use distinct `AGENTERM_IPC_ADDRESS` and `AGENTERM_WORKSPACE_PATH` values for
isolated tests, stable tab IDs instead of mutable indexes, and public wait
operations instead of fixed sleeps. Rendering investigations capture both
structured state and PNG evidence.

GitHub Actions observation is bounded and read-only: resolve one run and
attempt, assign one observer, back off while state is unchanged, and fetch logs
or artifacts only when the owning job reaches a relevant state. Git transport
and GitHub API authentication are separate authorities; never extract or
repurpose credentials. If an authorized mutation is unavailable, stop and give
the human the immutable run identity and required action.
