# Build and install AgenTerm locally on macOS

繁體中文：[在 macOS 本機建置與安裝 AgenTerm](macos-local-build.zh-Hant.md).

Use this path when running AgenTerm from a source checkout. It creates a real
application bundle at `~/Applications/AgenTerm.app`; do not pin the bare
`target/debug/agenterm` executable in the Dock.

```bash
./build.sh
./install.sh --local-build target/debug
open ~/Applications/AgenTerm.app
```

After the app opens, keep **AgenTerm.app** in the Dock. The installer also
copies the local build into a versioned directory under
`~/.local/share/agenterm` and refreshes commands under `~/.local/bin`.

## How the Release installer handles a missing signed asset

Running `./install.sh` without `--local-build` selects the Release installer.
On macOS, when the signed release archive is not available it will
automatically fall back to the `-unsigned-preview` package and prints a trust
warning that cannot be skipped. If Gatekeeper blocks launch, open
System Settings → Privacy & Security and choose **Open Anyway** for
`~/Applications/AgenTerm.app`.

Only an explicit HTTP 404 or 410 for the signed asset permits this fallback.
Transport, authentication, rate-limit, and server failures stop the install
instead of silently downgrading it.

For a source checkout, use `--local-build target/debug`. Your local build is
unsigned-but-local and still does not change release-channel trust decisions.

Older commands may still set `AGENTERM_ALLOW_UNSIGNED_PREVIEW=1`. It is now a
compatibility acknowledgment only: it does not force the preview, suppress the
warning, skip signed-asset verification, or alter the install record.

For an optimized local build, use:

```bash
./build.sh release-fast
./install.sh --local-build target/release-fast
open ~/Applications/AgenTerm.app
```

Local builds are unsigned bytes produced on your machine. Release checksum,
signature, notarization, Candidate, and Promotion rules remain unchanged.

## Cross-compiling for Windows from this Mac

Building a Windows target from macOS needs the MSVC CRT and the Windows SDK.
`cargo xwin` fetches them once into a user-global cache shared by every
project on the machine, so the step is normally invisible — you only meet it
on a cold cache.

**Use the local proxy.** The direct route to Microsoft's download host does
not complete from here: measured on 2026-09-21 it timed out at 12 s with no
response, while the same request through the proxy answered in about 1 s. On
the direct route a cold cache crawled from 58 MB to 73 MB in roughly fifteen
minutes; through the proxy it reached 1.1 GB and started compiling inside one
minute.

```bash
export HTTPS_PROXY=http://127.0.0.1:8888
export HTTP_PROXY=http://127.0.0.1:8888
export ALL_PROXY=http://127.0.0.1:8888

cargo xwin build --locked --profile release-fast \
  --target x86_64-pc-windows-msvc -p agenterm
```

Prefer the proxy for `cargo build` and `cargo fetch` here too: the crate
registry and the MSVC packages take the same route out.

Two ways to turn a slow download into a stuck one, both avoidable:

- **Do not interrupt `cargo xwin` mid-download.** A partial cache makes the
  next run re-verify everything from the start.
- **Do not run two `cargo xwin` builds at once.** They deadlock on the cache
  lock; three concurrent runs held the cache frozen until all but one were
  killed.

Asking for a target whose packages are not cached starts a fresh download, so
prefer the warm one. When the destination is the `win-aarch64-desktop` court,
an `x86_64-pc-windows-msvc` build runs there under emulation and costs no new
download.

## Before pushing anything a Windows gate will judge

```bash
./scripts/pre-push-check.sh
```

That runs, in the configuration the gate uses, the checks the gate runs first:
`cargo fmt --check`, the Windows-target release Clippy, the library tests, the
release-policy tests and the redaction check. The gate stops at its first
failure, so a formatting slip and a lint slip are two separate ninety-minute
rounds of it; both are minutes here. On 2026-09-21 they were exactly that --
two rounds, one for each.

The lint step on its own, for reference — the Windows target,
the release profile, all targets, warnings denied:

```bash
export HTTPS_PROXY=http://127.0.0.1:8888 ALL_PROXY=http://127.0.0.1:8888
cargo xwin clippy --locked --profile release-fast \
  --target x86_64-pc-windows-msvc --all-targets -- -D warnings
```

Two minutes here against ninety in the Candidate's Windows leg. A host-target
debug Clippy is not the same check and will not tell you the same things: on
2026-09-21 it was silent about two `let ... else` clauses that became
irrefutable after an enum lost a variant, and the Candidate's quality gate
rejected the lib tests for exactly that.
