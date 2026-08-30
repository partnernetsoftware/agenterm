# Structural review: `.qjs` → `.wasm` / tinyvm (2026-08-30)

Reviewer: grok. Read-only. Pin in tree: `crates/agenterm-qjswasm/Cargo.toml:40-41` `rev = "c7b6004"`.
`plan/design-value-representation-experiment.md` **does not exist**; V1 evidence is README + A9.

Verdict: the three-layer arrow is real and worth pushing. The engine is already the product’s only script line (qjs/wasmcore/rh archived). The next 10× is **prelude + host-op accounting**, not NaN-boxing, not asm, not a typed door.

## 1. Layering

**Compiler API is clean.** `~/repos/tinyvm/prd/PRD.md:758-763` and `758-787`: door stays raw `(ptr,len)`; compiler “names nobody’s host function”; vocabulary is the embedder’s. `tinyvm-qjs` has no `agenterm`/`tool` rust identifiers as product ops.

**Compiler *design* is not clean.** Object layout is justified by one embedder file:

- `~/repos/tinyvm/crates/tinyvm-qjs/src/runtime.rs:1620-1632` — “The binding library this targets (`agenterm/scripts/qjs/lib/fleet.js`)”
- same crate `src/parse.rs:452`, `src/parse.rs:929`, `src/emit.rs:1405+` — `fleet.js` as speed/JSON rationale

That is product vocabulary in comments and in the **shape bet** (flat k/v, no hidden class). Fine while 71 scripts look like fleet.js. False if agents grow long-lived objects.

**Door vs platform.** Window ops are declared as a map (`Cargo.toml:19-22`; `tool.rs:131-137`). Filesystem is **not**: `tool.rs:499-523` calls `std::fs::exists` / `read_to_string` in the engine crate. Process spawn/kill/read live here too. That is embedder policy (who may open `tool.*`, `tool.rs:21-33`) mixed with OS adapters that `agenterm-platform` already owns for the GUI. Keep the *permission* in qjswasm; push the *syscalls* down or the platform crate becomes optional fiction.

`host.rs:10-23` two-pass park is a **tinyvm borrow** constraint (`&mut` memory vs `invoke`), not compiler logic. Correct layer.

## 2. Value representation

V1 is two wasm values: `(tag: i32, payload: i64)` — `~/repos/tinyvm/crates/tinyvm-qjs/README.md:44-47`. No NaN-box experiment on disk.

A9 prices (`~/repos/tinyvm/prd/PRD.md:236`), after 2026-08-30 prelude work:

| op | now (order) | what moved it |
|---|---|---|
| loop | 146 steps | interpreter + boxing |
| concat 1000+1 | 2569 steps (`<3000` pin) | 8-byte copy, not tags |
| `.length` 6000 ASCII | 19854 / call (~3.3/char) | 8-byte `__len` |
| `includes`/`indexOf` | 7.2 steps/char | 4-byte window skip |
| `split` | 26 steps/char | same skip |
| `toLowerCase` | 38 steps/char | ASCII fast path |
| `JSON.parse` | ~29/byte | nibble loops |
| `JSON.stringify` | leftover ~4500/object + ~700/property walk | quote run done; **walk remains** |

Empty program 101 steps; CLI default subtracts that (`script_protocol.rs:100-112`).

**Next 10× is the prelude’s remaining linear walks and the step model, not V1 vs NaN-box.** Unboxing every opcode might shave the 146-loop constant; it will not turn a 108M-step GUI journey (`PRD_02_36` A1.8 / theme-smoke) into 10M. NaN-box fights wasm’s typed vals (you still return two results or you smuggle through i64 and re-box at every call). Do not reopen representation until a payload is *compute-bound past the 1500-round JIT crossover* (`~/repos/tinyvm/prd/PRD.md:187-210`: 535× vs wasmtime on 2e7 loops; agent scripts measured I/O-shaped, “三次假设有需求、实测为零”).

JSON.stringify’s ~4500/object is the honest next prelude target (hidden class / shape) — but that is exactly the bet `runtime.rs:1620` refused. Re-measure object cardinality on *agent* scripts before paying it.

## 3. Memory

Bump, no GC. Product cap **1024 pages = 64 MiB** (`src/script_engine.rs:336-355`), overriding tinyvm default 256 pages / 16 MiB (`~/repos/tinyvm/crates/tinyvm/src/wasm.rs:50-59, 1429-1434`). unix-frontend died at 16 MiB after two clipboard answers; at 64 MiB the same step later died on `max_steps`; at 256 MiB 1G steps still died — **wait-loop accounting, not leaks** (`PRD_02_36` A1.8).

Smallest honest collector for *this* design: **watermark reset at instance or top-level call**, which you already have (`Limits` reset steps per top-level call; pages live for the `Instance`, `wasm.rs:1407-1418`). Next: **region bump** (restore watermark after `JSON.parse` / stringify / one tool result is projected). Cost to zero-cost gating: a prelude that runs only if the program calls those ops — same discipline as per-method prefabs.

A tracing GC is in the tree as `[ ]` / P2 (`PRD.md:263, 1014-1015`) and would emit a visitor into every module, killing “unused method = unemitted”. Do not buy Wasm GC proposal to save journeys that are actually polling.

## 4. Budget model

tinyvm unit = **wasm instructions** (`wasm.rs:1418-1420`). Product default **128M**, hard **1G** (`script_protocol.rs:112, 148`). Wall clock is a sibling field (`wall_time_ms`, default 2000 in `Default` — too small for 3.5s journeys; hard 3.6e6). Host time is **explicitly outside steps**: `tool.rs:61-68` (`process.command` timeout because “the core’s step budget does not measure time spent in the host”).

Raising 1M → 16M → 64M → 128M in one weekend because journaled CLI + `JSON.stringify` + 25ms polls burned the ceiling is evidence the **unit is wrong for agent scripts**. A wait that must fail on macOS still consumes steps (`PRD_02_36` A1.8 unix-frontend).

Right model: **keep steps as runaway-CPU guard** (do not delete); **add host-op and wait-tick counters** (broker_requests already exists in `ScriptBudgets`). Charge `fleet_call` / `tool.*` 1 + bytes, charge `time.sleep_ms` wall, do not charge 1M wasm steps per idle poll. 128M as a silent default will keep lying.

## 5. Legibility

Named: heap, throw, missing string method, host-argument, property-of-non-object (`PRD.md:376-381`). **Open unnamed:**

- **A10** call of non-function → `unreachable executed` (`PRD.md:237`). Same `require_tag(TAG_FUNCTION)` as the old property trap. Highest agent pain (`[].concat` is missing method, not a type error in the author’s head).
- **`split("")` isolated surrogate** — tree says should be 4th fault class; today bare unreachable (`PRD.md:361`).
- Any remaining `require_tag` / `unbox_*` not on the unwind channel.
- Compiler/validation failures (`A7` nested closure + import) still `type mismatch` without a qjs span.

Do A10 with the same `__call_check` recipe as A8. Gate bytes on “program has indirect call”.

## 6. Door surface

42 named `HostFn`s, JSON in/out, two-copy park (`tool.rs:1-71`, `host.rs:10-23`). **Keep the raw door.** A typed `(tag,payload)` ABI is forbidden by `PRD.md:758-763` (breaks hand-written wasm; leaks JS into the host). JSON is the tax for “wasm has no record type” (`Cargo.toml:42-44`).

Do not grow the door by smuggling network/clipboard into every sandbox. Grow **profiles**: sandbox = `agenterm.*` only; tool = today’s 42; a future `net` profile is a third import module, load-time refuse by name (same as `host::check_declarations`).

Two-copy is correct until tinyvm can yield memory during host calls. Don’t “fix” it with guest alloc (`host.rs:25-28` rejected wasmcore’s `wasmcore_alloc`).

## 7. Missing from the tree (agent script engine)

Not listed, or listed as “demand zero” while journeys pay for them:

1. **Host-op / wait budget** (see §4) — tree tracks `--max-operations` only (`PRD_02_36` ~866).
2. **Cancellation** of `time.sleep_ms` / child wait when the GUI dies.
3. **A10 / Array.prototype.concat** (missing method vs call-undefined).
4. **Secrets**: `env.get` is a raw door; no redaction class.
5. **Deterministic time** for replay (only `time.now_ms`, `tool.rs:231-238`).
6. **Slot lifetime across agent turns** (one call vs persistent instance; 64 MiB bump makes persistence a leak).
7. **Network** — absent on purpose; say so on the product tree so it is not an accident.
8. **Concurrent guests** — one interpreter, no shared-memory story (P2 threads `[ ]`).
9. **Span-bearing compile errors** for A7-class validation.

## What to push (core-capability, not MiniCon)

1. A10 named call-of-non-function (legibility, unblocks `concat` misreads).
2. Host-op-aware budget so 128M stops being a diary of JSON walks.
3. Keep V1; next prelude win = stringify property walk **if** object cardinality changes; else leave it.
4. Region bump if a *single* invocation must retain a slot; not Wasm GC.
5. Move `std::fs` / process guts behind platform; keep `tool.*` as the permission module.

Do not: NaN-box, typed door, JIT (until a compute payload >1500 rounds exists), MiniCon embed.
