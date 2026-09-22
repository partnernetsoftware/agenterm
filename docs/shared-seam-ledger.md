# Shared seams with MiniCon

This is a coordination ledger, not a second source map. AgenTerm's current
code ownership and boundaries remain in [`plan/ARCHITECTURE.md`](../plan/ARCHITECTURE.md).
MiniCon's working proposal is `minicon/plan/plan-cross-project-reuse.md` in its
own repository. A pin listed here is an observation; MiniCon's `Cargo.toml` is
authoritative for the version it builds.

## Contract

- AgenTerm owns `agenterm-platform` and `agenterm-ui-core`. A MiniCon
  contribution lands and passes the gates here before MiniCon updates its pin.
- Share host-neutral behavior only after both products' callers and behavior
  have been measured. Record differences before choosing a common rule.
- Treat MiniCon's feature combinations as consumer builds, including target
  dependencies. A green default-feature build is not consumer evidence.
- Use envelope titles `seam: <crate>/<area>` for claims and handoffs. A claim
  names exclusive files, an agent, the purpose and the date. Update this ledger
  with the shared change; MiniCon records its pin bump in its own commit.

## Current observation — 2026-09-22

| Shared crate | Owner | MiniCon observed pin | Consumer surface |
|---|---|---|---|
| `agenterm-platform` | AgenTerm lane | `6ae0f9bb6` | 13 base features, plus `input-inject` in dev builds, `native-pixel-window` on Windows and `portable-pixel-window` on Unix |
| `agenterm-ui-core` | AgenTerm lane | `6ae0f9bb6` | `terminal-selection` and shared scrollbar geometry |

MiniCon moved its pin after adopting the shared scrollbar in its 0.1.22 train;
its local fmt, host/Windows Clippy and 360 tests passed. The observed pin
predates current AgenTerm `main` and does not qualify later commits. The
feature list above is an inventory from MiniCon's `Cargo.toml`, not yet an
automated gate result in AgenTerm.

## Claims and decisions

| Area | Agent | Exclusive files | Purpose and date | State |
|---|---|---|---|---|
| `agenterm-ui-core/click` | `cc-minicon` | None | Compare real click grouping callers, time windows, position keys and fourth-click behavior; 2026-09-22 | Paused: blank-space third click, composer fourth click, Windows click events and time-window policy differ between products |
| `agenterm-ui-core/scrollbar` | `cc-minicon` | None | Shared geometry vectors landed in `2e77d3195` (12 added tests, production code unchanged); 2026-09-22 | Both consumers now use shared geometry; MiniCon pin is `6ae0f9bb6`. Four mutation controls went red; one hit-test mutation was equivalent. A possible one-row drag shift when travel is sparse remains a separate product decision. |
| `agenterm-platform/consumer-matrix` | `cdx-agenterm` | None yet | Specify and add target-aware MiniCon consumer feature checks; 2026-09-22 | Planned; no build result claimed |

The composer candidate remains an investigation item, not a file claim. It
needs separate behavior evidence. Any active shared-file claim must be added
here before editing and cleared or handed off when the change lands.

## Consumer feature gate to add

MiniCon's base `agenterm-platform` request is `clipboard`, `entropy`,
`filesystem-publish`, `filesystem-read`, `font`, `ime`, `input`, `ipc`,
`parent-console`, `pty`, `runtime`, `screenshot`, `window`, with default
features disabled. Cargo also unifies `input-inject` for dev builds,
`native-pixel-window` on Windows, and `portable-pixel-window` on Unix.
The AgenTerm gate must compile that union per target, then run tests where a
native host or leased court can execute them. A cross-target check proves
compilation only. MiniCon's own Cargo manifest remains the source for this
list; its consumer-side policy test should flag drift before either lane calls
the gate current. Keep the target checks separate from AgenTerm's default or
all-feature platform builds so feature unification cannot mask a missing
consumer module.
