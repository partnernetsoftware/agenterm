# Rust condensed manual

Practical reference for Rust work in this repository. This is not a language
tutorial. It records local contracts and recurring failure modes that general
Rust knowledge does not reveal. Product ownership lives in `PRD.md` and its
module PRDs; source ownership lives in `plan/ARCHITECTURE.md`; agent workflow
lives in `AGENTS.md`. Those files remain authoritative when this manual and a
product decision differ.

Update this document when a Rust lesson is proven by code, target compilation,
tests, emitted assembly, a black-box journey, or a production failure. Do not
record guesses, one-off preferences, or a second living source map.

---

## 0. Before editing

1. Locate the owning PRD and architecture boundary.
2. Identify the exact package, features, targets, public black-box owner, and
   safe failure result.
3. Search every consumer of a changed geometry, protocol, feature, or native
   contract before the first build.
4. Locate the owning test target before a mutation run. A test inside
   `src/**` belongs to `cargo test --lib`; a similarly named integration test
   belongs to `cargo test --test <name>`. When a contract spans both, run both
   targets: a green command that never compiled the mutated assertion is not a
   successful red/green experiment.
5. Use a task-specific target directory when isolation matters:

```powershell
$env:CARGO_TARGET_DIR = 'target/my-leaf'
cargo clippy -p owning-package --all-targets -- -D warnings
```

Do not share that directory with another active Cargo process. Remove it after
the owning evidence, after resolving and checking that it is the intended
repo-local target. Never set `CARGO_TARGET_DIR` to `/tmp/claude-*`,
`/tmp/codex-*`, or any session `scratchpad/` — that is how a chat session
accumulates tens of gigabytes of leftover `target/` trees.

If `cargo` fails with `could not execute process .../build-script-build (never
executed)` / `No such file or directory` while compiling `libc` (or any crate
with a build-script), check whether `cc` on `PATH` is a non-compiler shim
(for example a Claude Code wrapper that prints `--provider required`). On
macOS pin the real linker for that crate/lane with
`[target.<triple>] linker = "/usr/bin/clang"` under a local `.cargo/config.toml`,
or fix `PATH` so `cc` is `/usr/bin/cc`. Symptom fingerprint: linker stderr
mentions `--provider` and linker stdout dumps an unrelated CLI help banner.

macOS same-machine wait/wake lab note (proven in `lab/mmap-ephemeral/`): Darwin
`os_sync_wait_on_address*` / `os_sync_wake_by_address_*` (macOS 14.4+) work
across `mmap` `MAP_SHARED` file slots when both sides pass the `*_SHARED`
flags; probe with `dlsym` before claiming availability. Hot ping-pong latency
still favors a yield spin — os_sync trades CPU for blocking wakes. POSIX
`shm_open` on current Darwin can create a name but return `EACCES` on any
reopen (same process or peer); do not plan multi-process mailboxes on shm_open
without a reopen probe — prefer a normal file + `mmap` `MAP_SHARED`.
Same lab: resident file-mmap beat nng pair `ipc://` (UDS) on p50 by ≥2× for
both waits; primary practical gate is os_sync/Native (idle does not spin). Receipt:
`lab/mmap-ephemeral/RESULTS.md`; opponent C: `lab/mmap-ephemeral/nng_bench/`.
Cross-OS wait map in the same lab: `WaitKind::Native` → macOS `os_sync_*_SHARED`,
Linux shared `futex`, Windows `WaitOnAddress`; practical bind/connect API is
`Endpoint`/`Server`/`Client` in `lab/mmap-ephemeral/src/channel.rs` (see
`PRACTICAL.md`). Prefer file-backed slots on Darwin (`shm_open` reopen often
`EACCES`). `kill(pid, 0)` only proves a pid exists; that lab stores the process
start-time beside `owner_pid` and the in-flight caller so a recycled pid does
not keep the mailbox and a dead caller does not leave it `Busy`. Addresses are
`shmbox:file:…` / `shmbox:shm:…` (`src/address.rs`); peers are `Server`/`Client`
with optional split flight `ask`/`await_reply` and `accept`/`reply`.

The repository is pinned by `rust-toolchain.toml`. Do not solve a compiler
failure by silently changing the toolchain, edition, target, linker, or global
Cargo jobs.

Treat generated-code cache identity as one atomic contract. Any transpiler
emission change that bumps `RH_CODEGEN_REVISION` must update the public-contract
and native-pack fixture pins in the same commit; run both owning tests before
the broad AOT pipeline. A stale pin is a delivery failure, not evidence that
the new emitter should reuse an old cache revision.

---

## 1. Pick the correct layer

| Concern | Owner | Must not own |
|---------|-------|--------------|
| OS-neutral mechanism contract | `crates/agenterm-platform/src/contract/*` or a narrow facade | AgenTerm product names, Fleet, scripts, navigation policy |
| Native mechanism | `crates/agenterm-platform/src/adapters/{windows,unix,linux,macos}/**` selected by `selected.rs` | Product gesture meaning |
| Shared product semantics | `src/frontend/*`, `src/ui_*.rs`, `src/ui_geometry.rs` | `windows_sys`, winit, X11, libc |
| Small host-neutral UI kernels | `crates/agenterm-ui-core` | Window handles, PTYs, product authority |
| Product-specific state | owning binary/module | Raw OS APIs or duplicated native adapters |

`agenterm-con` is intentionally a separate small package, but separation is
not permission to duplicate mechanisms. It may differ from `agenterm` in UI and
authority while reusing PTY, pixel, font, clipboard, filesystem, input, and
failure contracts.

If product code needs `windows_sys`, `libc`, `core::arch`, a raw handle, or an
`unsafe` block, stop and move the mechanism boundary first. Boundary tests are
expected to reject such leakage.

---

## 2. Cargo features are contracts

A build with many enabled features is weak evidence. Cargo unifies features, so
another dependency can accidentally make an undeclared module, OS import, or
optional crate available.

For a new or changed facade, run its isolated graph:

```powershell
cargo check -p agenterm-platform --no-default-features --features filesystem-publish
cargo test  -p agenterm-platform --lib --no-default-features --features filesystem-publish
```

Rules:

- A narrow feature lists every optional dependency and `windows-sys` API module
  needed by its selected adapter.
- A broader feature may depend on a narrow one; do not make the narrow feature
  depend on the full product surface merely to make compilation pass.
- Gate the facade, selected adapter, imports, and tests consistently.
- Dev dependencies can hide production graph mistakes. Inspect and test the
  normal graph separately when size or isolation matters.
- `cargo test FILTER` reporting `0 tests` is not success. List tests or correct
  the feature/filter until the owning tests actually run.
- Cross-platform process tests must select the host shell and its argument and
  environment-expansion syntax together; a hard-coded `cmd /c` is not a Unix
  test. Do not hide the mismatch with a host `cfg` skip.
- When product code shells out to enumerate host state, check the child's exit
  status before parsing stdout. A failed `ps`, `tasklist`, or equivalent command
  must become a typed failure, never a valid empty inventory.

Package boundaries improve cold-build isolation and make feature leakage
visible. They do not by themselves reduce linked size; the linker may already
discard unused code.

---

## 3. FFI and native adapters

Keep the mechanism support matrix separate from an upper-layer exposure
catalog. The former records ABI shapes for which the native layer has a real
calling implementation; the latter records which of those shapes a product
chooses to expose. If the two sets currently happen to be equal, there is no
hidden "supported but unexposed" test case: prove the boundary by narrowing the
upper catalog while the raw mechanism test remains green, or add a real native
calling implementation before claiming a larger mechanism set.

An exposure cardinality is upper-layer schema data, not mechanism metadata. Do
not let the mechanism crate export a count of the shapes a product chooses to
expose: when the exposure owner already derives its catalog from its own type
list, that count has two authorities and drifts silently. Prove the migration by
deleting the mechanism-side helper and keeping the owning cardinality/parity
test green; `agenterm-dyn` carried such a public exact-family stub count only
until `agenterm-qjswasm` derived `7 x (arity + 1)` from its own scalar list.

Native calls belong behind typed platform contracts. A sound adapter states:

- which thread may call it;
- who owns every handle, pointer, allocation, and callback lifetime;
- which inputs are validated before FFI;
- how absence differs from `Unsupported` and operational `Failed`;
- what cleanup runs on every partial-failure path;
- whether success means visibility, atomic replacement, or durable storage.

Do not cast `libc::c_char` directly to `u8` in target-portable adapters. Its
signedness differs by target (notably x86_64 versus aarch64), so a cast that is
needed on one ISA becomes a denied redundant-cast lint on another. Recover the
stored byte with `c_char::to_ne_bytes()[0]`, then validate the resulting byte
sequence. Cross-target Clippy is part of the proof, not merely host tests.

For a host callback table borrowed during construction but invoked later,
separate the two lifetimes explicitly. Copy or own descriptor names before the
constructor returns, retain callback contexts until the native handle is closed,
and destroy the handle before releasing those contexts. Null the output handle
before validating the table so every failure path has the same postcondition.
Treat parameter, result and guest-memory pointers as call-scoped borrows; bound
arity before allocation, require the exact result count, and translate callback
failure into typed fail-closed state without unwinding across C. A Swift wrapper
should own stable pointer storage rather than rely on `Data.withUnsafeBytes`
beyond its closure.

When one synchronous native call accepts multiple spans into guest linear
memory, treat aliasing between those spans and the result slot as valid unless
the public wire contract explicitly forbids it. Validate every range first,
take the allocation base pointer once, and derive raw call-scoped pointers from
that base; do not construct overlapping `&mut` slices merely to pass their
addresses to FFI. Keep the backing allocation fixed for the whole call and
publish the result only after the foreign function returns.

For native APIs that write a bounded C string, prefill the output with a
non-NUL sentinel and require an actual NUL after a successful call. Zero-filled
buffers can fabricate termination when the native function truncates or writes
nothing. The sentinel is evidence of untouched storage, never the terminator:
search only for byte zero so valid non-UTF-8 native bytes remain intact.

When an FFI entry point holds `&mut Runtime` while invoking a synchronous host
callback, a documentation-only ban on reentry does not satisfy Rust's aliasing
rules. Reject every callback-time API that takes a runtime handle before turning
the raw pointer into a reference. Use a scoped thread-local guard so normal
return and unwinding both restore the boundary, and prove a rejected nested call
cannot corrupt outputs, latch an otherwise successful outer lifecycle, or leave
its diagnostic behind after that outer call succeeds.

Do not describe elapsed-time checking after a synchronous native callback as a
timeout: it cannot prevent a hang, and latching based on device speed makes a
deterministic guest nondeterministic. Preemption requires a different ownership
model (worker execution, copied/transactional memory and cancellable work), not
a stopwatch around borrowed memory on the owner thread. For trusted app-compiled
callbacks, require each implementation to be finite and nonblocking, then bound
untrusted guest amplification with a per-lifecycle quota charged before dispatch.

`catch_unwind` returning a status is not enough after a mutable runtime operation.
The handle-aware panic branch must mark the instance permanently failed, restore
any lifecycle/phase guard and invalidate cached outputs that could represent
partially mutated state; only inspection and close remain valid. Prove that
transition under the exact unwind-enabled delivery profile, because the ordinary
test profile does not establish that a release artifact can catch at all.

Cross-machine Rust test harnesses must not treat `env!("CARGO_MANIFEST_DIR")`
as a runtime repository locator. That value embeds the build host path in the
test executable and fails when exact linked harnesses run in a clean VM. If a
test owns repository metadata, accept an explicit runtime evidence root and
copy only the bounded contract/fixture bundle into the target court; ordinary
in-checkout execution may retain the manifest-directory fallback.

The current `agenterm --all-features` Windows test target cannot be produced
by `cargo xwin test --no-run` on macOS: enabling optional `script-lua` selects
vendored LuaJIT, whose `luajit-src` MSVC build invokes `msvcbuild.bat` and
requires `cl.exe` from the Visual Studio registry. The attempt fails at
`mlua-sys` with `failed to find cl` before test executables exist. A green
cross-build of the default QJS feature set does not prove the optional Lua
feature: `cargo xwin test --locked --target x86_64-pc-windows-msvc -p agenterm
-p agenterm-ui-core --no-run` did produce the default-feature Windows test
executables. Keep the optional Lua Windows evidence separately named until
that toolchain boundary is solved. Do not remove `--all-features` from an
existing gate and reuse its evidence ID as if the same assertion had run.

A wrapper crate's `env!("CARGO_PKG_VERSION")` names the wrapper, not the product
whose output it presents. When a monolith and a dynamic provider must emit the
same product identity, keep the formatted text in the product library and have
both presenters call it. Equal package versions at one revision are not a
cross-crate invariant and cannot substitute for one owner.

A dynamic provider mode may own process stdin/stdout only when the mode is
already an isolated child whose parent checks exit status before accepting its
bounded protocol. Make fd ownership an explicit per-mode contract, require the
ABI output lengths to remain zero so the launcher cannot duplicate frames, and
never re-enter the public command dispatcher from that handler: doing so can
self-spawn the same launcher recursively and escape the parent's kill/reap
containment. A nonzero child exit makes any bytes already in the private pipe
non-authoritative; this rule does not justify direct stdio for an ordinary
in-process request.

A handle that separates a mutating operation from a later two-stage copy must
clear its previous retained output before starting every new operation. Clear
on typed failure and panic as well as success replacement; otherwise a caller
can observe a failed trust/storage refresh, then accidentally copy stale bytes
from an earlier success. Prove the failure-then-copy sequence at the public FFI
boundary, not only the internal operation result.

Security.framework may return a partial signing-information dictionary for a
damaged or unsigned code object. When a caller explicitly requests validity,
run `SecStaticCodeCheckValidity` before publishing identifier, cdhash, Team ID,
or entitlement facts; only known integrity failures are a completed `false`,
while unreadable, unsupported, permission, and unknown statuses remain typed
unavailable. A missing signing identifier means unsigned, and a missing or
mistyped cdhash must never be serialized as a present `none` value. Keep every
Create/Copy-rule CoreFoundation object under one RAII owner and type-check all
borrowed dictionary values before decoding them.

FSEvents file-level streams are coalesced hints, not one record per filesystem
syscall. Keep a single-directory contract by filtering raw callback paths
lexically against a canonical root; never canonicalize a removed or rename-old
path. Disambiguate cumulative Created/Removed/Renamed flags with delivery-time
existence plus invocation-local seen state, and fail typed on dropped-event or
must-rescan flags. Without `UseCFTypes`, callback paths are `char **`, while
CoreFoundation `Boolean` remains `u8`. Catch callback unwind, keep the context
alive through Stop, and clean up in Start/Stop/Invalidate/Release order. Do not
use `FSEventStreamFlushSync` in a duration-bounded observer: it has no timeout
and can silently defeat the public deadline.

`ReadDirectoryChangesW` overlapped reads borrow their notification buffer and
`OVERLAPPED` until final completion. Use DWORD-aligned storage, share the
directory for read/write/delete, and on every timeout or abnormal wait path run
`CancelIoEx` followed by a blocking `GetOverlappedResult` drain before any
borrowed storage or event handle can drop. Treat a zero-byte completion as lost
directory truth, validate the entire offset chain before applying a public event
cap, and keep `bWatchSubtree` false plus a separator check for a non-recursive
single-directory contract. Rename-old and rename-new are independent removed and
created facts; do not require them to arrive in one batch.

When an event filter depends on mutable node facts, project those facts from the
same before/after snapshots that produced the event. Do not issue a later tree
query from a reply projector: the node may already have changed or disappeared,
turning a post-capture filter into a different-time guess. Removed events read
their facts from the previous snapshot; other events prefer the current one.
Sparse native notifications must mark unavailable facts explicitly, and an
unknown boolean fact must match neither `true` nor `false`.

Windows application facts need three independent lifetime and honesty brackets.
Enumerate both 32-bit and 64-bit Uninstall views under HKLM and HKCU, then
canonicalize and deduplicate executable paths before deciding uniqueness. For a
negative running fact, use the ToolHelp executable basename to narrow candidates
before opening process handles; an inaccessible same-name candidate makes the
fact unavailable, while unrelated protected processes do not. Embedded
Authenticode inspection owns the CryptQuery store, message and certificate with
RAII, and every `WinVerifyTrust` VERIFY call must receive a matching CLOSE. If
the public contract says embedded signatures only, do not publish a successful
catalog-signature fallback as verification of an absent embedded signature.

When a binary format has both converter inspection and runtime loading, keep
one structural validator and split capability *description* from capability
*availability*. Static inspection may parse manifests, imports and export
signatures, but it must not instantiate the module, run start/init or silently
bind a host function. Runtime open reuses that structure result and applies the
host registry/trust/resource gates afterward. If FFI publishes a descriptor,
make it bounded, versioned and explicit that it is metadata—not a replacement
wrapper for the original standard executable bytes. Prove that a native-import
descriptor succeeds without a registry while an unauthorized runtime open still
fails closed.

Deterministic host input belongs at the shared runtime boundary, not only in a
replay recorder or one UI wrapper. Validate known bit masks and monotonic clocks
before entering guest lifecycle code. A rejected host argument that executed no
guest instruction must not latch the cartridge or advance its remembered
clock; prove a corrected next call still runs. When portable snapshots exclude
an app-owned clock, successful resume starts a new validation epoch and the app
persistence envelope must restore that clock alongside the snapshot.

For converter-authored WASM metadata, keep the standard module as the source of
truth. Derive capability namespaces from its function import table, sort and
deduplicate them canonically, then append a standard custom section while
preserving all producer bytes. Do not ask a CLI caller to maintain a duplicate
capability list. Reject an existing manifest instead of rewriting identity,
run the complete static descriptor before publication, and create output once
through an atomic no-overwrite path. Attach after producer optimization so a
strip pass cannot discard the custom section. Test reproducible bytes, native-import
derivation, duplicate-manifest refusal and absence of output after preflight
failure.

Prove that a supposedly standard cartridge ABI has an independent producer,
not only multiple guests built by the runtime's own language/toolchain. A
freestanding C fixture is a useful minimum: compile with an ordinary wasm32
Clang backend and no libc, JS glue, WASI or tinyvm library; attach metadata only
after linking; then run the exact resulting bytes through static inspection,
snapshot/restore and tinyvm/JSC/browser replay comparison. Keep the compiler as
development tooling rather than a runtime dependency. This catches accidental
Rust ABI assumptions and private executable conventions that WAT fixtures alone
cannot expose.

When a PRD uses checked tree leaves as executable claims, every new `[x]` leaf
must be added to the owning integration suite's leaf-to-test map in the same
change. Map it to a test that actually executes the relevant product boundary;
documentation presence alone is not evidence. This keeps planning prose from
silently getting ahead of the suite as new non-Rust SDK behavior is added.

A converter-facing host profile must be callback-free and content-addressable.
Encode exact ABI/media versions, resource ceilings and versioned native import
signatures, but never serialize function pointers, native implementations or
trust authority. Keep static compatibility honest: declared memory/table and
exact imports can be proven without execution, while fuel, output volume and
native semantics still require dynamic conformance. Use one strict canonical
decoder across CLI and FFI so reordered, duplicate or trailing fields cannot
produce multiple identities for the same claimed app build.

A catalog's self-reported profile digest is useful for discovery and converter
content addressing, but it cannot define an App build's authority: an attacker
can replace both bytes and digest. Generate the expected canonical profile from
the app-compiled configuration/native registry and require exact byte equality
after a bounded same-origin download. Keep older catalogs readable by making
discovery metadata optional; never make compatibility optional once it is
present.

Do not place a filesystem helper behind optional-feature `cfg` when an
unconditionally compiled CLI subcommand calls it. An all-features suite hides
that mistake. In addition to feature-rich tests, compile the default binary and
execute at least one real default-feature command that crosses the helper.

Windows checklist:

- Convert paths/text to bounded NUL-terminated UTF-16 at the adapter edge.
- A fixed Win32 UTF-16 array is an encoded-unit contract, not a UTF-8 byte
  contract. Reserve its final unit for NUL and reject an over-capacity value
  before the first authority call; copying a prefix can split a surrogate pair
  and turns an accepted receipt into a false claim about the delivered text.
- A Cargo build script is compiled for the host. Never guard Windows resource
  generation with build-script `#[cfg(windows)]`: Linux/macOS cross-builds then
  silently emit PE files without VERSIONINFO. Branch on
  `CARGO_CFG_TARGET_OS` / `CARGO_CFG_TARGET_ENV` at runtime, pin the resource
  compiler in every owning target lane, and inspect each separately packaged
  EXE/DLL. A root package resource does not reach a binary or DLL owned by
  another crate.
- Read `GetLastError` through `last_os_error()` immediately after the failing
  call; another FFI or allocation can overwrite thread-local error state.
- Use RAII for GDI objects, handles, clipboard allocations, capture ownership,
  and process resources. Transfer ownership only after native success.
- A Windows `PROCESS_INFORMATION.hThread` is required through suspended setup
  and `ResumeThread`, but not for ordinary process wait/termination afterward.
  Once resume and PID validation succeed, close that `OwnedHandle` immediately;
  keep only `hProcess` unless a typed runtime contract actually operates on the
  primary thread. Never close it earlier than the armed partial-process owner
  can still terminate a failed suspended launch.
- Distinguish GUI-thread-only APIs from worker-safe I/O. Do not block the event
  thread on clipboard reads, PTY waits, filesystem retries, or IPC round trips.
- Model native clipboard reads as bounded one-shot work: the platform worker
  owns native retry/blocking mechanics and wakes on success, typed failure,
  panic, and disconnect. The product retains stable target identity and
  revalidates target plus focus before committing text; dropping the receiver
  on tab/window close must never strand or block the worker.
- Treat a human-editable paste review as a second asynchronous identity
  boundary even when its native modal runs a nested GUI message loop. Keep raw
  handles and dialog callbacks in the platform adapter; after confirmation,
  re-normalize and re-bound edited text, then revalidate stable target, focus,
  epoch and terminal mode before the only PTY write. Carry an explicit origin
  bit so CLI/control paste never inherits an interactive modal accidentally.
- Retry only documented transient errors, with a strict attempt/deadline bound.
- Never guard `total - started.elapsed()` with an earlier `elapsed() < total`
  sample: pre-emption can cross the deadline between the two reads and
  `Duration` subtraction panics. Use `saturating_sub`/`checked_sub`, stop on
  zero, and test the already-expired case. Under panic-abort this otherwise
  appears on Windows as worker exit `0xc0000409`, not as a Rust assertion.
- Do not use the whole Windows desktop (`HWND = 0`) as deterministic UIA success
  evidence: unrelated providers can recycle, reject calls, or exceed the bounded
  deadline. Let desktop-wide ABI probes typed-fail when a provider is unavailable,
  and own success semantics with a child-owned native window fixture plus the
  public black-box journey. `InvokePattern::Invoke` returning does not guarantee
  the target GUI thread has processed `WM_COMMAND`; wait on the owned observable
  effect with a deadline instead of asserting immediately or sleeping blindly.
  A newly visible native proxy can also briefly return a successful but empty
  RuntimeId SAFEARRAY: bounded re-read is valid, but a synthetic id is not,
  because later node resolution must compare the provider's opaque value. A
  window-scoped tree already has a stronger root identity in its caller-supplied
  HWND: encode and validate that root anchor directly, then use opaque RuntimeIds
  only for descendants that must be rediscovered through the tree walker. If a
  descendant still has no RuntimeId, omit that unaddressable branch; never fail
  the whole snapshot or invent an action target that could resolve to a sibling.
  Cross-platform command tests that pass a synthetic HWND must allow the native
  adapter to reject that window before name lookup; prove exact not-found matcher
  semantics with pure data and prove the integrated path with an owned window.
- A composed desktop observation is not one instant merely because it has one
  JSON envelope. Freeze the target from the first bounded window inventory,
  perform the slower tree/pointer reads, then re-enumerate and require the full
  target identity row to match before publishing. Refuse ambiguous focus,
  disappearance, drift and an over-budget inventory; expose tree truncation
  instead of selecting the first window or calling a partial desktop complete.
- A process-global native resource needs one lock and one RAII owner across every
  adapter path. In particular, Windows console attach/detach cannot be split
  between a dependency helper and a platform guard: serialize the whole
  `FreeConsole` / `AttachConsole` / `CONIN$` / `WriteConsoleInputW` transaction,
  and verify exact record counts rather than treating a nonzero call as complete.
- A native dependency replacement earns its complexity only when it removes the
  complete production edge and preserves hidden behavior, not when it merely
  rewrites visible calls. Direct ConPTY must retain cancellable overlapped input,
  output draining during pre-24H2 `ClosePseudoConsole`, DSR fragments, build-gated
  flags, suspended Job assignment, quoting/environment lookup and exact wait
  semantics. Start the child suspended, assign its kill-on-close Job, then resume;
  a failed first child needs a fresh ConPTY because its output pump may reach EOF.

Unix checklist:

- Retry `EINTR` where the syscall contract requires it, not indiscriminately.
- Treat file replacement and durability as separate phases: same-directory
  rename gives name atomicity; syncing the parent owns directory durability.
- Keep fd ownership explicit across fork/exec and close-on-exec boundaries.
- Never substitute a symlink-following path convenience API when the contract
  promises a real entry or protected ancestry.

Use official OS documentation for exact flags and ownership semantics. Record
the stable conclusion in code comments or this manual, not a copied article.

**Never take a `#[repr(C)]` enum by value on the C boundary** (proven at
milestone 53). The C side can pass any `int`, and constructing a Rust enum
from an out-of-range integer is *immediate* UB — it happens at function entry,
before the `match` runs — so a `_ =>` wildcard arm only catches
legal-but-unhandled variants; against garbage input it is false comfort.
Machine-code-identical fix that does not move the ABI: take an integer
(`u32` / `c_int`; a `repr(C)` enum is passed as `int`), keep the C header
declaring the enum, `match` on the integer, and map unknown values explicitly.
Derive the discriminants as `Enum::Variant as u32` constants inside the
function (or a macro), never copy the magic numbers — then a rename/reorder/
revalue of the enum follows through at compile time. When the same numbering
is duplicated across several sources (Rust enum, C header, test-side
constants), gate it with a test that parses every source and compares name
sequence AND values in declaration order; comparing only the name *sets*
misses an insert/swap that shifts every later value.

---

## 4. `unsafe` discipline

Rust 2024 requires unsafe operations inside explicit `unsafe {}` blocks even
within an `unsafe fn`. Keep those blocks as small as possible.

Every unsafe mechanism needs:

1. A safe public caller that checks lengths, geometry, alignment assumptions,
   integer overflow, target capability, and lifetime.
2. A local safety explanation tied to those checks.
3. A scalar or safe semantic reference where one exists.
4. Boundary and adversarial tests: zero length, short buffers, tails, overflow,
   clipping, close/cancel, and partial native failure.
5. Target-specific compile evidence for every `cfg` implementation.

Do not use `unsafe` to avoid a borrow-checker design problem, to share mutable
GUI state across threads, or to skip a bounded copy without measurement. A
small allocation is preferable to an unowned pointer; a reusable bounded buffer
is preferable once profiling proves the allocation is hot.

---

## 5. SIMD, intrinsics, and assembly

### Measure before specialising: a word-wise swizzle beat hand-written NEON

Converting a BGRA framebuffer row to RGBA looks like a textbook SIMD kernel:
compact, fixed lane layout, clear pixel contract. Measured on aarch64 over a
3456x2234 surface (release, `black_box` on both ends so the loop is not
eliminated):

| implementation | per full surface | throughput |
| --- | --- | --- |
| byte-at-a-time (`dst[0] = src[2]` ...) | 2.12 ms | 14.6 GB/s |
| whole `u32` load, shift, store | **1.33 ms** | **23.2 GB/s** |
| NEON `vld4q_u8` / `vst4q_u8` | 1.49 ms | 20.8 GB/s |

The word-wise version in plain safe Rust won. At ~23 GB/s the kernel is
memory-bandwidth bound, so wider vectors buy nothing and the deinterleaving
`vld4q` costs more than it saves. Specialising would have added an ISA path,
a feature-detection branch and a parity obligation for a measured *loss*.

Take the cheap representation change first — byte moves to word moves is often
most of the available win — and only reach for intrinsics once a measurement
shows the scalar form is not already saturating memory. Record the rejected
optimisation, as here, so nobody re-derives it.



ISA specialization is justified for compact, stable kernels with a clear byte
or pixel contract. Current good examples are alpha-mask XRGB composition and
XRGB-to-RGB8 packing. VT parsing, JSON, Unicode width, tree state, and other
branch-heavy policy are poor assembly targets.

Required pattern:

```rust
pub fn safe_kernel(input: &[u8], output: &mut [u8]) {
    let length = checked_common_length(input, output);
    unsafe { selected_kernel()(input.as_ptr(), output.as_mut_ptr(), length) }
}
```

- Keep one scalar truth implementation.
- On x86_64, select optional SSSE3/AVX2 with
  `is_x86_feature_detected!` and cache the function pointer with `OnceLock`.
  SSE2 is baseline for x86_64, not for every x86 target.
- On aarch64, NEON is baseline for repository targets, but still compile the
  target-specific implementation.
- Process vector bodies plus exact scalar tails. Test lengths around every lane
  boundary and compare every output bit.
- Keep CPU detection outside inner loops.
- Do not use a similarly named instruction without checking its polynomial or
  semantics. SSE4.2/Arm CRC instructions compute CRC32C, not PNG's IEEE CRC-32.
- Prefer intrinsics first. Inline assembly is reserved for evidence that the
  compiler cannot retain the required instruction sequence or ABI.
- Native FFI is a mechanism boundary, not automatic size evidence. Compare the
  final PE and raw sections against the implementation it replaces. A system
  codec can still add wrapper/control-flow code or cross one file-alignment
  block; keep it when the measured trade buys shared semantics, less protocol
  code, or stronger output, and record the honest delta instead of claiming a
  size win.
- Do not select an assembly/FFI target from one `cargo bloat` top-symbol row.
  ICF, cold blocks, unwind ranges, and adjacent symbol intervals can charge
  unrelated code to a small leaf. Filter the exact symbol, inspect emitted code
  when needed, and compare total `.text` plus final artifact bytes. A measured
  Windows PTY case showed 7.0 KiB in the top list, while the filtered native
  wait leaf was only 105 B; changing its typed boundary moved the 7.0 KiB label
  to process creation and changed neither `.text` nor PE size.
- Audit const generics and iterator adapters whose type records a container's
  shape. A fixed-schema helper taking `[T; N]` can emit its collection path once
  per used `N` even when the operation is cold and identical. In `agenterm-con`,
  replacing the JSON `object<const N>` helper with one owning `Vec` boundary
  collapsed about 2,445 B of measured specializations to 727 B and reduced the
  same-profile PE by 3,072 B. Apply this only where the saved code outweighs
  allocation/runtime cost; it is not a blanket rule against generics.
- For branch-heavy dispatch, share repeated lookup and validation through plain
  non-generic helpers, but keep fallible command work inside its existing local
  `Result` boundary. A helper taking a closure recreates one monomorph per call;
  flattening `?` into a surrounding function that returns `()` changes the
  error-propagation contract. The measured con control refactor used ordinary
  session/cell helpers, retained per-command `map`/`and_then` boundaries, and
  reduced the final PE by 512 B with `.text` down 720 B.
- Protocol enum tags need one encode/decode authority. Plain non-generic
  enum-owned conversion methods can remove parallel match tables without a
  trait or allocation. Measure sections as well as final bytes: con's compact
  mouse tag unification removed 32 B of `.text`, but PE file alignment kept the
  artifact byte count unchanged, so it is a consistency win rather than a PE
  size claim.
- A fixed CLI schema does not require one generic integer parser instance per
  target type. A single non-generic checked ASCII-to-`u64` kernel can feed
  `TryFrom` for unsigned widths while callers retain their exact error text. It
  must preserve details such as one leading `+`, leading zeroes, empty input,
  non-ASCII rejection, and overflow. In con this kernel emitted as 93 B,
  reduced `.text` by 224 B, and crossed one PE alignment block for a 512 B
  artifact reduction. Keep signed parsing on `FromStr` until its separate
  grammar and range semantics have matching evidence.
- Format diagnostics through the narrowest type that owns their public range.
  `Duration::as_millis()` returns `u128`; formatting it directly can retain the
  complete `u128` decimal formatter even when the native timeout boundary is
  only `u32` or `u64`. Preserve semantics with an explicit checked or saturating
  conversion and test the extreme value before claiming a size win. In the
  Windows PTY adapter this removed the 1,043-byte linked formatter and moved the
  exact custom-std PE from 533,504 to 531,968 bytes. This rule applies to cold
  diagnostics, not to values whose real contract requires 128-bit precision.
- Do not change an unwind-enabled native host to `panic = "abort"` merely to
  remove runtime bytes. `agenterm-con` catches panics at WNDPROC, deferred-work,
  and native-thread FFI boundaries; abort changes that containment contract
  rather than optimizing its implementation. First prove no public robustness
  invariant depends on unwind, or retain the exact-profile unwind graph.
- A stripped-core measurement root should call the narrow interpreter entry it
  actually needs. Do not retain an optional export-name map facade merely to
  make one size selftest convenient; keep export parsing in the core and prove
  public name lookup in its own black box. This can recover a file-alignment
  boundary without deleting product semantics or raising the size gate.

Always inspect emitted release code:

```powershell
$env:CARGO_TARGET_DIR = 'target/isa-check'
cargo rustc -p agenterm-ui-core --release -- --emit=asm
rg 'pshufb|vpmullw|packuswb' target/isa-check/release/deps -g '*.s'
```

Writing intrinsics is not evidence that the instruction survived optimization.
Conversely, a visible Rust loop is not evidence that specialization is needed.
`slice::fill`, copies, iterator reductions, and other mature primitives often
already lower to vectorized runtime/compiler code. Inspect and benchmark first.
Keep a shared safe geometry wrapper when it removes duplicated clipping, but do
not maintain ISA forks without a measured gain.

That inspection can also disprove the optimistic case. In the staged con PE,
`fill_xrgb_rect` emitted two scalar per-pixel store/branch loops rather than
`rep stosd`, SIMD, or a runtime call. A bounded x86-64 `rep stosd` span leaf and
an AArch64 NEON peer retain the same safe clipping facade; spans below 64 pixels
stay scalar to avoid setup cost. Paired exact-state con PEs remained the same
size while `.text` grew 48 bytes and the ISA PE gained exactly one `F3 AB`
signature. A release-mode 200-frame 1920x1080 A/B measured 102.3 ms versus
210.0 ms (2.05x), with final buffers compared bit-for-bit. This is sufficient
evidence to retain a compact ISA fork; GUI frame timing alone was too noisy.

---

## 6. Bounded concurrency and shutdown

Terminal and GUI code must remain bounded under slow consumers and abnormal
children.

A dual-stream event facade and a one-stream output facade are different public
contracts even when both read the same retained rings. Do not implement the
one-stream form by calling the dual-stream form: splitting the aggregate budget
silently halves the requested stream and can advance an unrequested peer
cursor. Share the bounded page collector beneath both facades, keep each
stream's absolute cursor and truncation state independent, and prove identical
bytes for the same stream/cursor at the public black-box boundary.

Regex search over paged retained bytes must not treat a transport page edge as
an input boundary: `$`, `\z`, word boundaries, and matches longer than a carry
window can otherwise become page-alignment dependent. Either preserve the
matcher state and its required context or retain one explicitly bounded logical
scan window and search only after catching up to its current cursor. Bound
pattern compilation, total retained scan bytes, match bytes, and elapsed time
separately; a linear-time matcher does not make repeated whole-window searches
strictly linear across the complete wait.

A bounded command buffer does not necessarily bound the native work it
schedules. For duration-bearing media or timer protocols, validate record
count and aggregate scheduled duration before dispatch, in addition to byte
length and each record's local range. TinyArcade tone batches, for example,
bound both event count and total sequential duration so a tiny payload cannot
retain the audio owner indefinitely.

- A queue needs a byte/item capacity, explicit backpressure, and a per-GUI-turn
  drain budget.
- Closing must wake blocked producers/consumers and define whether committed
  tail data remains drainable.
- Dropping the product owner must not strand a worker on a full queue.
- Coalesce wakeups and latest-only resize requests; pointer frequency must not
  become PTY resize frequency.
- One tab's malformed output, exited child, failed screenshot, or bad request
  must remain local to that tab/request.
- Do not hold a lock while invoking unknown product callbacks unless the
  contract explicitly owns that serialization and tests shutdown/backpressure.
- Use stable IDs across asynchronous work; reject stale tab/epoch completions.

Prefer a small state machine over booleans that permit impossible combinations.
Selection, mouse capture, process lifecycle, publication, and native resource
transfer all benefit from explicit states.

---

## 7. Rendering and performance evidence

Optimize work removed, not just instructions made clever.

1. Separate input/PTY drain, geometry, raster, chrome, screenshot, and present
   timing where possible.
2. Use public `perf-stats`/snapshot/PNG evidence for `agenterm-con`; use the
   owning public UI journey for the main app.
3. Reduce redundant frames, unchanged rows, allocations, resize calls, and
   lock contention before specializing arithmetic.
4. Keep resize chrome responsive while coalescing PTY/VT geometry at the
   trailing edge.
5. Measure cold build, warm build, binary size, frame latency, and throughput
   separately. Improving one does not prove the others.

For resize profiling, drive the real host through `agenterm-con cli
resize-window --width N --height N`, use a blocking screenshot as the render
fence, then read `perf-stats`. A successful command reply only proves that the
native size request was accepted; PNG IHDR dimensions prove that the backing
surface actually changed. End automated journeys with `close-window` so the GUI
event loop, PTYs, listener, and native handles follow the production shutdown
path instead of being abandoned by a test-process kill.

When extracting retained framebuffer storage, migrate its failure contract as
well as its `Vec`. A host that deliberately omits the terminal from its logical
frame cannot silently ignore retained-layer allocation failure: that would
present stale pixels or a hole. Unix HiDPI therefore maps shared
`RetainedFrameError` into its pixel-window/screenshot result, marks storage
valid only after raster succeeds, and commits its product key afterward. Keep
exact dirty-row masks and invalidation keys product-owned when replacing them
with a conservative shared interval would increase repaint work.

Never infer performance from source appearance or binary size. A package split
may improve compilation without changing PE bytes; a typed correctness state
machine may add bytes while removing a data-loss or deadlock path. Report both
cost and benefit truthfully.

Release, release-fast, and debug are different artifacts. Do not compare their
sizes as if profile policy were implementation growth.

---

## 8. Cross-platform evidence

Repository delivery spans `{x86_64,aarch64} x {win,lnx,osx}`. One host build
cannot prove code hidden behind another target's `cfg`.

- A successful Linux compile does not prove dynamically loaded GUI runtime
  libraries exist in the test image. X11 journeys using winit under Xvfb need
  either the host `libxkbcommon-x11` runtime (`libxkbcommon-x11-0` on Ubuntu)
  or the product's bundled copy: the selected Linux adapter
  `crates/agenterm-platform/src/adapters/linux/linux_xkb_startup.rs` embeds
  `libxkbcommon-x11.so.0` and `libxcb-xkb.so.1` (see `vendor/linux/`) and
  re-execs once with `LD_LIBRARY_PATH` when the host omits them. Keep probing,
  Unix permission changes, dynamic loading and re-exec inside the platform
  adapter; product frontends call the neutral facade on every host, whose
  non-Linux adapter is an explicit no-op. Without either library source,
  source, event-loop creation can abort before the product exposes its control
  endpoint. Preserve child stderr in black-box launch harnesses so a missing
  `dlopen` dependency is reported as the first failure instead of a generic
  startup timeout or signal.
- Unix IPC black-box fixtures must create their runtime parent first and set it
  to mode `0700` before launching the product. A default temp directory created
  under umask `022` is commonly `0755`; the product must reject that endpoint as
  unsafe rather than weakening ownership checks for CI.
- A resident child must bind its current-user native control endpoint before it
  claims or publishes a live durable state. Derive a short opaque endpoint name
  from sealed generation identity, never accept TCP or a caller-supplied path,
  and finish each bounded server reply before dropping the one-request stream.
  Otherwise a crash or bind conflict can publish a live process that no client
  can control, or Windows can discard a buffered named-pipe reply on close.

- Put shared semantics outside target modules.
- Keep selected adapter APIs type-identical across hosts.
- Compile aarch64 when adding NEON or pointer-width-sensitive FFI.
- Compile a native/cross Unix consumer when changing a file imported only by
  Unix frontend modules. If the local host lacks its C compiler/sysroot, report
  the missing evidence and leave it to the owning CI/native cell; do not call a
  Windows-only build cross-platform proof.
- Test endian/channel assumptions through semantic bytes. Repository x86_64 and
  aarch64 hosts are little-endian, but scalar code should avoid accidental
  native-byte-order coupling when simple shifts express the contract.

- **Never put `test` in a host-family `cfg` for a host-implemented module.**
  Declaring a module as `any(target_os = "linux", target_os = "macos", test)`
  compiles it into `--all-targets` runs of every *other* host as well, where its
  only consumers are themselves host-gated: the module and its `test`-only helpers
  then read as dead code (38 diagnostics on Windows for
  `crates/agenterm-cu/src/privilege_broker.rs` together with
  `crates/agenterm-cu/src/privilege_provider.rs`) although nothing on that host
  could ever call them. Name exactly the hosts that own a consumer —
  `any(target_os = "linux", target_os = "macos")` for the broker, wire and provider
  modules, and `any(target_os = "linux", all(target_os = "macos", test))` for
  `privilege_broker_metrics`, whose only consumer is
  `crates/agenterm-cu/src/privilege_system_broker_linux.rs`; that keeps both Unix
  hosts' tests while never opening Windows. Hold every item of such a module to the
  same boundary: the native-consent constructors in
  `crates/agenterm-cu/src/privilege_apply.rs` are
  `any(target_os = "linux", target_os = "macos")` because only the Unix launchers
  consume them, while the structs stay host-neutral. A host `cargo clippy` cannot
  see any of this, so the gate must include
  `cargo xwin clippy -p agenterm-cu --all-targets --no-deps -- -D warnings` with
  per-package `--message-format json` attribution. Fix the boundary, never the lint:
  `allow` masks it, and inventing a consumer on a host without the implementation
  fabricates behaviour that host does not have — it keeps its typed
  unsupported/refusal surface instead.

- **A resident owner's program is not `current_exe()`.** `agenterm-cu` is a formally
  distributed sibling of the product executable: the release zip ships it beside
  `agenterm`, `install.sh` requires both executables colocated and verifies the
  `agenterm-cu` / `libagenterm` ABI pair, and the macOS bundle validator requires
  `Contents/MacOS/agenterm-cu`. An in-process embedder (`agenterm:acu` inside the
  product binary) therefore cannot start a resident owner through its own image:
  the product binary has no owner mode to dispatch to. Resolve the fixed sibling
  beside `current_exe()` in one place — fixed basename plus the platform
  executable suffix, final component not a symlink and regular, Unix execute bit
  (Windows has no execute bit to check) — and let every owner launcher
  (managed job, browser session, device lease) start *that* program with its one
  internal argument. The same rule owns any resident protocol registration:
  a Native Messaging manifest must name the resolved sibling `agenterm-cu`, not
  the embedding product's `current_exe()`, because only the sibling dispatches
  the native-host invocation. Keep the refusals typed and distinct: absence, a
  non-regular shape, and an inspection failure are three facts, so never collapse
  a permission or I/O failure into "missing". Do not call the refusal a
  zero-effect failure either — the durable start intent or claim already exists
  and is closed by the owning failure path, which may honestly report
  cleanup-uncertain. Path inspection followed by a spawn is still a TOCTOU window
  and is not an atomic identity-bound exec; do not claim the reverse.
- **A test whose subject borrows a record cannot prove the record is unchanged.**
  When the unit under test takes `&Record`, an assertion that the record's state
  and terminal trigger are unchanged is tautological: the signature already
  forbids mutation, so it passes even when the code that could relabel the record
  lives somewhere else. Put that claim where the mutation is possible (the
  owner-side single `finished` gate) or prove it at the integration boundary.
  The same asymmetry ruins a cleanup sweep: a terminal record that still holds a
  live resident owner needs an explicit release over the owner's own endpoint,
  and counting it as "already terminal" is a silent leak, because an unreachable
  owner is not evidence that a release happened. Conversely, mapping every
  owner-unavailable result to "already terminal" is honest for a record that is
  genuinely terminal and misleading for one that still claims to be running;
  prefer a typed failure there rather than a bucket that reads as success.
- **A gate env variable is declared, not exported.** A task's `env` list and a
  contract's `env_allow` are a declaration and a pass-through allowlist; neither
  exports anything. The script must fail closed when a required name is absent
  (name it, plus any repo-location or distinctness checks the court needs), and
  the caller exports it. That is the existing `ACU_MCU_REPOSITORY` shape, and a
  court that fails closed is stronger than one that reads an ambient default.
  Record the exact export block where the gate can be reproduced, and do not
  invent an exporter to paper over it. For the embedded job-wait cancellation
  court the reproducible block is one repo-local root with a private runtime
  directory (mode `0700`) and the nine names it declares:

  ```sh
  R=$PWD/target/<gate-lane>; mkdir -p "$R"/{runtime,config,data,cache,state}; chmod 700 "$R/runtime"
  ( HOME="$R" XDG_RUNTIME_DIR="$R/runtime" XDG_CONFIG_HOME="$R/config" XDG_DATA_HOME="$R/data" \
    XDG_CACHE_HOME="$R/cache" AGENTERM_CU_MANAGED_JOB_PATH="$R/state/managed.json" \
    AGENTERM_CU_RUNTIME_PATH="$R/state/runtime.json" AGENTERM_CU_IDEMPOTENCY_PATH="$R/state/requests.json" \
    AGENTERM_CU_AUDIT_PATH="$R/state/audit.jsonl" \
    bin/agenterm cli script task run acu-provider-job-wait-cancel-smoke )
  ```

  A court that instead needs the real layout (for example
  `cu-managed-job-smoke`) runs with the caller's own environment; see the next
  rule for why the two must never share one shell.
- **Two courts with opposite `HOME` requirements need two environments.** A gate
  that wants the real layout and a gate that requires its state under a
  repo-local `target/` lane are mutually exclusive by design. Run each with a
  single-command env prefix in a subshell (`( VAR=... VAR=... cmd )`) and never
  export those names into a shell that also runs the other court; a shared
  `export` silently breaks whichever court runs second.
- **Find the guard before you build a court around the hazard.** Session-end
  releases a resident owner over `StopAndRelease`, which the owner serves with
  `stop_and_release` — and that method branches on `on_expiry` *before* it would
  reach `stop_with`: a detach-policy record is finished through
  `finish_adopted_detached` (publish `detached` plus a bounded `detach_liveness`
  observation) and never through the adopted-group `terminate()` inside
  `stop_adopted`. So "session-end might kill an adopted, still-live process" is
  excluded by a structural branch, not by one line. That distinction decides
  whether an end-to-end court is worth its cost. Keep two mutations apart when
  reading such a result. Deleting the branch itself (`stop_and_release` taking
  the `stop_with` path for a detach policy) is the one that exposes the hazard:
  the adopted process comes back `Signaled(9)`, killed by `stop_adopted`.
  Suppressing the detach `terminal_report` publication instead reddens a court
  for an unrelated reason — the owner never returns from `run_to_completion`,
  so cleanup reports uncertain — and says nothing about whether the adopted
  process would have been killed. A court that only watches the child survive a
  real session-end is also weak on its own, because the owner exits as soon as a
  terminal report exists and the sweep then takes the
  `managed_job_owner_unavailable` path without exercising this code at all.
  When the guard is a branch rather than a line, a Rust test at that branch is
  the honest owner of the claim:
  `managed_job_owner::tests::session_end_releases_a_detach_policy_owner_without_killing_the_adopted_process`
  adopts a real group-leader fixture under a detach policy, calls
  `stop_and_release`, and asserts detached truth plus the fixture still live
  under its exact start identity. An e2e court buys cost, not evidence.
- **Count the guards before you read a mutation result.** The neighbouring claim
  — session end must not relabel a detach that lease expiry already published —
  is held in three places: the `terminal_report` early return in
  `stop_and_release`, the identical one at the top of `try_finish`, and the
  store's refusal to take a second terminal transition. Suppressing any single
  one leaves the test green, which is evidence about the depth of the guard and
  not about the test. Remove both owner-side returns and the test does redden,
  and even then the durable record is never rewritten: the failure arrives as
  `managed_job_terminal_publish_failed` from the store. So state a mutation
  result as "reddens when N of these M sites fall", and treat a single-site
  mutation that stays green as a finding to explain rather than a test to
  delete.
- **Mutate in place and prove the revert; never restore from a copy.** A
  mutation check has to put the tree back exactly, and a file under active work
  usually carries uncommitted changes, so the two obvious restores are both
  wrong: `git checkout -- <file>` discards that work, and a `cp` snapshot
  restored later silently overwrites anything written to the file in between.
  Do the mutation as an exact, reversible in-place substitution, then reverse
  the same substitution and prove it byte for byte — record the file's hash
  before and after and require equality (`git diff --stat` showing insertions
  and **zero deletions** is the second reading). That makes the check safe
  without needing to establish exclusivity first, which matters because the
  "concurrent writer" a status line reveals is often another agent legitimately
  working the same lane. A test written to be mutated also has to survive its
  own red path: the failing run is the common run, so anything it spawned must
  be removed by `Drop` rather than by cleanup statements after the assertions,
  which a panic skips. Prove that by checking for strays after a deliberate red
  run, not by reading the code.
  Two facts that constrain any such court: `job-spawn --expiry detach` is
  refused with the typed `managed_job_detach_retired` before every side effect
  (a spawned child is its managed owner's to clean up), so only `job-adopt`
  carries a detach policy — it is the default there; and `cu-managed-job-smoke`
  already builds the adopt/detach/live fixture, then kills it by exact identity
  before session-end, which is why no existing court observes this shape.

See `AGENTS.md` for current commands and CI cells; do not duplicate that living
matrix here.

---

## 9. Validation ladder

Use the smallest authoritative evidence first:

1. `cargo fmt` for the touched crates (`cargo fmt -p PACKAGE`), never a bare
   `rustfmt <file>`: a bare invocation assumes style edition 2015 while these
   manifests declare edition 2024, and the two editions disagree on nested `use`
   ordering (`src/platform/adapters/*/contract_manifest.rs` is the measured
   case), so a "formatted" file turns a clean tree red under
   `cargo fmt --all -- --check`. To format one file alone, pass the manifest's
   edition explicitly: `rustfmt --edition 2024 <file>`. Edition 2024 also reserves
   `gen`; a local with that name is a parse error, not a warning.
2. Package Clippy with `--all-targets -- -D warnings`.
3. Pure scalar/contract tests.
4. ISA parity and target compilation.
5. Owning binary tests.
6. Direct public black-box journey.
7. Release artifact and size.
8. Integrated repository/release gate only at the proper boundary.

`cargo test --bin NAME` builds a test harness; it does not prove that
`target/debug/NAME` was refreshed. Before a black-box script or generated-doc
step invokes that path, run an explicit `cargo build -p PACKAGE --bin NAME` and
record the product binary as the evidence subject.

A paired black-box parity court must prove more than `A == B`: give at least
one case an absolute success or typed-refusal expectation, and inject one known
bad side as a negative control that must diverge. Drain child stdout and stderr
concurrently behind explicit byte bounds, and treat timeout as failure after
kill plus reap; otherwise two equally broken artifacts or a full output pipe can
produce a false green or hang the court.

An unwind-only dynamic artifact needs its own lint/test lane. Linting
`agenterm-cu-provider` under the default profile stops in `build.rs` with
`must be built with panic=unwind: use --profile abi-release (or abi-dev)`; the
owning evidence is `cargo clippy -p agenterm-cu-provider --profile abi-dev
--all-targets -- -D warnings`, not a dev/release invocation.

A script that imports the native door cannot pass a check-only lane.
`agenterm:native` is enabled only for production execution inside the supervised
worker, so the repository check corpus (`cli script check-many`) refuses such a
file before any of its assertions can be read. Exempt it in
`scripts/qjs/lint.qjs` by naming the court that actually runs it, keep the
exemption fail-closed against a stale path, and never enable the native door for
checking to make the gate green.

Hand-authored Wasm has two argument channels; never overload one as the other.
Trailing `script run ... -- ARGS` are strings read through `tool.arg(n)`, while
repeatable `--wasm-entry-arg i32:...|i64:...|f32:...|f64:...` values bind the
exported `main` parameters in order. Carry floating-point entry arguments across
the JSON worker protocol as raw IEEE bits so NaN payloads, infinities, and signed
zero survive exactly; parsing them into a JSON number silently changes or loses
valid Wasm values.

Artifact provenance also has two input identities; make the distinction typed
before reading. `script hash FILE.qjs` compiles source and hashes the resulting
module, while `script hash FILE.wasm` hashes the exact file bytes that the
loader consumes. Never route an artifact through `read_to_string`: ordinary
modules fail UTF-8 decoding, and text-looking bytes can be recompiled into a
different program. A qualification receipt's `artifact_sha256` is the black-box
oracle for the artifact path. Source hashing is still a real compilation: carry
the entry directory, project root, and requested local/tool door into the same
resolver/compiler used by `check`; a context-free hash rejects an import-bearing
program that the runtime can load and fingerprints no runnable artifact. Normalize
the entry identity before deriving either resolver root: a bare filename has an
empty `Path::parent`, so deriving roots from the raw CLI spelling can erase the
entire import search path even though `./name.qjs` names the same file.
Identity inspection and artifact production are not exempt from input budgets.
Hash, pack build/load, smoke and qualification verbs must reuse the run path's
take-limited readers, reading at most the effective source/artifact ceiling plus
one byte before a typed refusal. A bare
`std::fs::read` allocates in proportion to an attacker-controlled or sparse file
before it can decide that the runtime would never admit those bytes.

A module resolver's canonical cache and the compiler's module identity are two
different contracts. The pinned qjs compiler callback accepts only a specifier
and returns source text, so a canonical `PathBuf` cache can deduplicate reads,
byte charges, and module-count charges but cannot tell the compiler that
`lib/x` and `lib/x.qjs` are one evaluation identity. Do not claim single
evaluation without an identity-bearing upstream interface, and do not recreate
the import/export system locally to simulate it. For imported files, metadata
is only an early refusal: perform a source-ceiling-plus-one bounded read and
sample cancellation and deadline again after that read before accepting or
charging the bytes.

When an engine adapter returns a typed failure category, preserve it through
every operation arm in the shared dispatcher. Rewrapping the check arm with a
default configuration constructor while the run arm projects the typed error
creates two public `exit_class` values for the same compile failure. A shared
dispatcher should own transport spelling and backend code, not overwrite the
engine's diagnosis.

The same stale-artifact trap applies to `libagenterm`: an `abi-dev` build
refreshes `target/abi-dev/libagenterm.*`, while an integration-test executable
may still open `target/debug/libagenterm.*`. If a new export is present in the
`abi-dev` symbol table but a dynamic sweep reports it missing, inspect the
actually loaded library path, then copy the just-built `abi-dev` library into
the test profile directory before rerunning. A source-compiled test harness is
not evidence that the sibling dynamic library was refreshed.

When a Script task summarizes a large filesystem tree, do not issue one host
call per entry or return a whole directory listing through a bounded bridge.
Put the neutral bounded walk in `agenterm-platform`, require an explicit entry
ceiling, fail without partial truth when the ceiling is crossed, and return a
fixed-size aggregate through one Script host operation. The v0.1.16 Candidate
proved both failure modes in sequence: 4,096 host operations were insufficient,
then one `deps/` listing exceeded the bridge result cap. Native aggregation
measured 204,283 files / 44,738,264,418 logical bytes in 1.88 seconds without
raising Script compute or host-operation budgets.

When a Script consumer asks about one native identity, prefer one bounded
exact-key platform query over transporting a complete inventory for guest-side
filtering. Price the bridge bytes and guest parse steps as well as the host
lookup: an O(n) inventory can dominate even when it counts as one host
operation.

GUI tests inherit `AGENTERM_NO_ACTIVATE=1`. Use public `wait-*` commands instead
of fixed sleeps. A test that launches a GUI must own endpoint/workspace
isolation and process cleanup.

Keep direct terminal automation distinct from focus-routed UI automation.
`send-keys` deliberately targets a terminal, while `send-ui-keys` must traverse
workspace shortcuts and the current composer/terminal focus owner exactly like
a keyboard event. For composer black-box evidence, query current physical input
bounds through `ui-snapshot`, click those native client coordinates, and prove
both the pre-submit PTY absence and post-submit terminal result.

An IME automation hook must inject the platform-neutral `ImeEvent` after native
decoding, not manufacture Win32 messages or bypass product focus routing. Keep
preedit and commit distinct, bound text and cursor indices at CLI plus wire
decode, expose both composer and active-terminal preedit in structured state,
and report terminal commit success only after the complete PTY write. Such a
hook proves product routing and failure transactions; it does not prove a real
installed IME's IMM32/TSF keyboard behavior.

Capability facts belong to the selected platform adapter. Once an adapter ships
the normalized mechanism, remove product-side OS exceptions that duplicate an
older unsupported state; otherwise the implementation can work while public
discovery still forces callers to degrade. Keep human acceptance as a separate
product-evidence status rather than misreporting the mechanism capability.

Native IME status is event-driven observation, not a render-loop query. Cache
the typed status on open, focus, keyboard/IME transitions and explicit
structured observation; invalidate only the owning chrome when it changes.
Publish stable typed defaults for unknown state, because background/no-activate
windows legitimately lack a thread-local focused input context.

Native IME acceptance must use a real focused desktop window plus physical
virtual-key or scan-code `SendInput`; `KEYEVENTF_UNICODE` bypasses conversion,
and synthetic `WM_IME_*` can manufacture false preedit/commit evidence. A Rust
`char` value is not a Windows virtual-key code: map ASCII letters to uppercase
`VK_A..VK_Z`, keep digits in `VK_0..VK_9`, reject unsupported characters, and
pair every key down with key up. Final CJK text and observed native preedit are
the behavior evidence; status labels and screenshots are supporting evidence.

Feature-gate an adapter facade on both its contract prerequisites and at least
one concrete provider feature. `input + ime` can expose IME facts without
selecting a pixel host; compiling `run_pixel_window` in that graph leaves its
native/portable implementation module absent. Prove narrow capability features
independently instead of relying on a product's larger unified graph.

Process completion and host lifetime are separate states. A terminal waiter
must publish any exit code before its completion flag with release ordering;
the GUI consumes the flag and status with acquire ordering, retains the final
screen, and closes only on an explicit product action. Do not make a one-shot
`-e` child silently redefine remain-on-exit behavior for the whole host.

State waits belong at the owning event loop, not in client polling loops. Keep
them bounded by count and deadline, key them by stable IDs, wake them from the
same transition that updates observable state, and return the completed typed
state (`child_exit_code` included) rather than only a boolean receipt.

A GUI-lifetime control listener must not spawn one detached thread per accepted
connection. Use a fixed worker count, a bounded connection queue, a separate
bounded GUI-request queue, and a short request-read deadline so incomplete
clients cannot create unbounded threads or retain every worker indefinitely.
Queue saturation may return a bounded busy response; shutdown must clear queues
and wake all blocked workers.

A successful named-pipe request write does not prove that its reply will remain
readable after the server releases the pipe instance. On Windows, complete the
server response according to the native pipe lifecycle before releasing the
handle. If the transport can still lose a request or reply, never retry a
mutation under a fresh identity: wrap each request in a CSPRNG identity, claim
that identity before GUI dispatch, and retain bounded pending, completed, and
tombstone states. A reconnect may use only the same identity, so it either
retrieves the exact cached result or fails closed without dispatching twice.
Bound identity count, result bytes, and retention time; reject new work when
those bounds are full rather than evicting a still-replayable mutation. Treat
Windows pipe errors 109/233 and EOF during request write or response read as an
unknown transport outcome, not proof that product work did or did not run.

A startup readiness probe is not automatically a stable handoff. A Windows
named-pipe server can accept one query while rotating into its steady accept
loop, then return error 233 to the next connection. Before the first mutation,
require at least two consecutive independent read-only readiness transactions;
reset the streak on any transport failure and keep the whole handshake under one
wall deadline. Do not retry the mutation itself under a fresh identity.

Every deferred control reply must have an owning cancellation path. Before a
tab is removed, fail its waits and pending screenshot reply with the stable tab
ID; before window shutdown, fail all remaining replies. Expose pending counts
for black-box sequencing, and keep Drop cancellation as a fallback rather than
the only path, because dropping the sender otherwise collapses a typed close
into a misleading generic timeout.

An enqueue API that may reject work must not take an owned reply sender before
capacity/busy validation succeeds. Borrow `&mut Option<ReplySender>` (or return
the sender with the error), and transfer it only on acceptance. Otherwise the
caller loses the only typed-error path, sender drop wakes the receiver without
a value, and a deterministic busy rejection is observed as a generic timeout.
Cover both sides: acceptance consumes the sender; rejection preserves it for
the caller to answer exactly once.

Closing a bounded PTY output queue only releases a producer blocked on that
queue; it does not by itself release an OS read or process wait. Teardown must
close product backpressure synchronously, then transfer master/child ownership
to a platform-managed background owner that terminates the child tree, closes
the native pseudoconsole and drops both halves. Never call potentially blocking
`ClosePseudoConsole` on a GUI/event thread. On Unix, a detached reader's
duplicated master fd prevents a mere product-master drop from delivering HUP
and can otherwise strand both reader and waiter.

A PTY root PID is not its containment. POSIX shells can create several process
groups inside one PTY session, and descendants may ignore HUP, TERM and INT.
Retain an exact observer for the session leader, enumerate the bounded session,
open and recheck exact process references, freeze all members until a second
scan is stable, then terminate and observe every retained reference exited.
Windows must terminate its retained Job Object and query
`ActiveProcesses == 0`. A missing tab, closed endpoint, or exited root is not a
cleanup receipt. Publish containment kind, member counts, empty postcondition
and worker completion through the public control response.

Foreground signaling is narrower than owned-session cleanup. On POSIX, derive
the target process group from `tcgetpgrp` on the retained PTY master, recheck
that group belongs to the retained session immediately before `killpg`, retain
bounded member observers, and verify STOP/CONT scheduler state or TERM exit.
INT may prove native delivery but must remain unverified without application
acknowledgement. Direct ConPTY exposes no equivalent foreground-process-set
handle; `GenerateConsoleCtrlEvent(..., 0)` reaches every attached process and
therefore fails foreground isolation. Return a typed limitation before mutation
instead of substituting input bytes, PID/name scans, foreground activation, or
whole-Job termination. Keep `pty-stop` as the separate portable full-session
safety contract.

On macOS, a termination audit token captured between `fork` and `exec` becomes
stale when the child image changes. A close-on-exec status pipe proves exec
success, but does not make a pre-exec mutation token durable. Retain an
observe-only kqueue reference across the transition; when cleanup begins, open
the mutation reference while that exact observer brackets the numeric PID as
still live, then recheck session membership before signaling. Never hide this
identity race with a fixed sleep.

Kill a frozen POSIX session leader last. If it exits first, the kernel can mark
a stopped child process group orphaned and deliver `SIGHUP` plus `SIGCONT`;
that child may resume and `exec`, preserving its PID while invalidating the
retained Darwin audit-token pidversion. When an exact signal reports `ESRCH`
but the retained observer says the same process object is still alive, reopen
the mutation reference, recheck the session, freeze again and retry within the
original deadline. Do not reinterpret that disagreement as proof of exit.

Do not create one teardown thread per closed PTY. Transfer sessions to one
platform reaper with bounded queueing and per-item panic containment; use an
isolated overflow teardown only when that queue cannot accept ownership. This
bounds normal close-storm concurrency without moving native handles back onto
the GUI thread.

Initialize the PTY reaper before acquiring a native PTY. A lazy first close can
discover thread-creation failure only after ownership has moved into teardown;
dropping that failed task may synchronously run the same blocking native close
on the event thread. Startup failure before acquisition is the bounded result.

On the pinned Rust 1.97 toolchain, `OnceLock::get_or_try_init` is still unstable.
For fallible process-wide initialization without feature gates, store a
`Result<T, (io::ErrorKind, String)>` inside `OnceLock` and reconstruct an
`io::Error` for callers.

When bounded producers and control IPC share one native wake event, service the
bounded control queue on every wake before consuming producer work. A producer
that reposts wakes for its remaining backlog can otherwise prevent the event
loop from reaching `about_to_wait` and starve the very command intended to stop
that producer.

For multi-session frontends, make producer work a global per-event budget, not
a full budget independently granted to every session. Divide the fixed byte
budget across live sessions (while preserving progress for each) so tab count
cannot multiply GUI-thread latency.


A bounded request queue does not by itself bound event-loop latency: one GUI
callback can still drain every queued heavy request. Set the callback batch
below the maximum simultaneous worker count, atomically take that fixed batch
and report whether backlog remains, then repost the native wake when needed.
Coalesce producer wakes on the empty-to-nonempty transition. Keep wait/deadline
evaluation outside that dispatch budget so timeout progress is never deferred
by load.

Latest-only command coalescing must preserve both queue order and reply
ownership. Concurrent IPC workers may not have enqueued a logical burst when
the GUI drains its first bounded batch, so inspecting only the current queue
tail is timing-sensitive. Use a short fixed deadline measured from the first
supersedable request, make every other command an ordering barrier, submit the
last value once, and complete every absorbed reply. Never reset the deadline on
later arrivals or leave those replies owned only by the normal pending-wait
registry; window teardown must fail them explicitly.

Deferred frame operations need one global owner when they change active-session
state. A per-tab pending screenshot slot is insufficient: draining several
requests can switch active repeatedly and render only the last target, stranding
the others. Cover pending-render and background-encode as one bounded state,
reject overlap explicitly, and move PNG encoding/publication off the GUI thread
while retaining a shared one-shot reply slot for immediate close cancellation.
Keep the pending operation at frontend scope, not inside the target session: a
later tab selection must not make progress depend on that target remaining
active. At render entry, capture the latest desired active tab, render the
target, copy the frame for background work, then restore and invalidate the
visible owner.

Do not use an omitted frame commit to suppress presentation: the compatibility
contract treats an unspecified write as a full write. Capture-only rendering
needs an explicit discard receipt that is accepted for retained and transient
backings, invalidates backing content, and makes every host skip native present.

Feature-isolate diagnostics too. An optional adapter trace must not call an
unrelated feature-gated module merely to choose a log directory; use the native
temporary-directory contract when that is the documented destination. Full
product feature union can otherwise hide an undeclared dependency indefinitely.
A dependency feature enabled only in one target-specific Cargo table likewise
does not exist on peer targets: cfg-gate the adapter import and implementation,
and keep the peer-target public command alive through an explicit typed refusal.

Do not rerun a large gate to compensate for not knowing which smaller test owns
the behavior. Add or identify the owner.

When a parity test invokes a live inventory twice, normalize only the fields
that are defined to advance between observations, such as wall-clock text and
uptime. Keep stable boot, machine, provider, and schema identities in the
comparison. Replacing the complete live-facts object with a fixture can make a
real adapter or routing mismatch invisible.

---

## 10. Review checklist

- Does the code live in the owning layer?
- Does a narrow feature compile in isolation?
- Are all queues, inputs, outputs, retries, and waits bounded?
- Does shutdown wake blocked work and clean owned resources?
- Are native ownership and thread-affinity rules explicit?
- Is every unsafe precondition enforced by a safe caller?
- Does ISA code have scalar parity, tail tests, target compile, and emitted-code
  evidence?
- Did measurement prove specialization rather than source aesthetics?
- Do Windows, Linux, and macOS adapters expose the same neutral contract or an
  explicit parity gap?
- Does a public black-box test prove user-visible behavior?
- Are generated artifacts outside Git and isolated targets cleaned?
- Were changed docs checked with `scripts/doc-redact-check.sh`?

If a recurring answer was hard to discover, add the proven rule here.

## Proven casebook

The detailed, measured cases are in [the Rust casebook](agenterm-rust-casebook.md).
Read the relevant entry when changing that boundary; this condensed manual remains
the required first read before Rust, Cargo, FFI, PTY, IPC, rendering, platform,
or build work.
