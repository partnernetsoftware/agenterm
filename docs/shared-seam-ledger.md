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
| `agenterm-platform` | AgenTerm lane | `5eda1de74` | 13 base features, plus `input-inject` in dev builds, `native-pixel-window` on Windows and `portable-pixel-window` on Unix |
| `agenterm-ui-core` | AgenTerm lane | `5eda1de74` | `terminal-selection` |

The observed pin predates current AgenTerm `main`; it does not claim that the
consumer has qualified intervening commits. The feature list above is an
inventory from MiniCon's `Cargo.toml`, not yet an automated gate result.

## Claims and decisions

| Area | Agent | Exclusive files | Purpose and date | State |
|---|---|---|---|---|
| `agenterm-ui-core/click` | `cc-minicon` | None | Compare real click grouping callers, time windows, position keys and fourth-click behavior; 2026-09-22 | Paused: blank-space third click, composer fourth click, Windows click events and time-window policy differ between products |
| `agenterm-ui-core/scrollbar` | `cc-minicon` | `crates/agenterm-ui-core/src/lib.rs` test module only | Add MiniCon's ten geometry vectors and five mutation controls to the shared crate; 2026-09-22 | Active claim; production code unchanged until tests and both consumers are reviewed |
| `agenterm-platform/consumer-matrix` | `cdx-agenterm` | None yet | Specify and add target-aware MiniCon consumer feature checks; 2026-09-22 | Planned; no build result claimed |

The composer candidate remains an investigation item, not a file claim. It
needs separate behavior evidence. Any active shared-file claim must be added
here before editing and cleared or handed off when the change lands.
