# Never debug in the release pipeline

The Candidate's Windows cell takes about 45 minutes and stops at its first
failing gate. Used as a debugging loop it answers one question per 45 minutes,
and answers it with a single exception string.

On 2026-09-20/21 that cost more than ten hours across thirteen rounds. The
same class of defects — four separate layers in one suite — then fell in about
twenty minutes once the loop moved to a real Windows machine under our own
control. Three minutes a round, with the whole log in hand.

**The release pipeline proves a release. It is not an instrument for finding
out why something failed.** Anything that reaches CI should already have been
proved on Windows here.

## The loop

```bash
# Build all three artifacts, push them, and prove the guest holds exactly
# those bytes before anything runs.
./scripts/windows-court-sync.sh

# Then run whatever gate you are working on, in the court:
utm-court exec win-aarch64-desktop -- cmd.exe /c \
  "cd /d C:\minicon-six\repo && C:\minicon-six\run2\agenterm.exe cli script run \
   --profile tool --timeout-ms 900000 --max-operations 1000000000 \
   --project-root . scripts\qjs\<gate>.qjs -- . \
   C:\minicon-six\run2\agenterm.exe <extra args> > C:\minicon-six\out.log 2>&1"

utm-court pull win-aarch64-desktop 'C:\minicon-six\out.log' -
```

An `x86_64-pc-windows-msvc` build runs on the `win-aarch64-desktop` court
under emulation, which costs nothing extra and keeps the warm cross-build
cache. See `docs/macos-local-build.md` for the cross-compile itself.

## What this loop caught that CI could not have, cheaply

Four layers in one suite, each of which would have been one CI round:

1. the ACU provider library was simply absent from the environment;
2. so was the resident owner executable;
3. a 3-second deadline was asserted with a 150 ms margin, and the court
   measured 3152 ms — two milliseconds over;
4. `MoveFileEx` refuses to replace a file that is open even when every handle
   granted delete-sharing, which needed `ReplaceFileW` instead.

Only the fourth was a product defect. The first two were environment and the
third was a bound sized for an idle machine — and all three would have read
as "the gate failed again" in CI.

## Rules that keep the loop honest

- **Keep the three artifacts in lockstep.** `agenterm.exe`, `agenterm-cu.exe`
  and the ACU provider must come from the same build. Two rounds went into
  chasing a failure that was only version skew, and one more into a stale
  `agenterm-cu-provider.dll` left behind because a `copy` was chained after an
  extraction that had not finished. `windows-court-sync.sh` prints host and
  guest sizes side by side for exactly this reason — read them.
- **Kill the guest's processes before replacing anything**, in a separate
  command, and confirm it landed. A DLL still loaded cannot be overwritten,
  and the failure is silent.
- **Push one artifact per transfer.** A combined ~11 MB archive times out; the
  individual ~3-4 MB ones land in seconds.
- **Clear `target/smoke` between runs** when a run was interrupted. Leftover
  managed-job state surfaces as `managed_job_outcome_unknown`, which looks
  like a product failure and is not.

## What the 2026-09-21 round added

Two more traps, both belonging to the court rather than the product:

- **`exec` returns when the guest agent accepts a command, not when the guest
  has run it.** Three cell steps issued back to back all start at once, so the
  cell reads a receipt its own predecessor has not written yet and reports
  `..._receipt_missing` — a missing file, not the race that caused it. A push
  issued straight after a `mkdir` loses the same race and reports "cannot find
  the path". `scripts/windows-court-runtime-cell.sh` puts a marker in the log
  at the end of each step and waits for it.
- **A guest DLL stays locked after every visible process is gone.** Pushing
  over it fails with "being used by another process", which reads like a
  transport fault. Each run gets its own timestamped guest directory instead.

And one that belongs to this Mac: **overwriting a running or previously-run
Mach-O in place gets the next execution SIGKILLed (exit 137)**, because the
code signature no longer matches the file. `rm` first, then copy.

## The single most expensive diagnostic gap

Candidate 35590507738 failed **all six** runtime cells with the same string,
`cu_retirement_cell_mcp_provider_exit`, and that string named a step rather
than a cause. Six cells, six platforms, one message, nothing to act on.

The cause was one line in `candidate.yml`: the four smoke scripts were passed
in the wrong order, so `acu-mcp-provider-smoke.qjs` received the position of
`native-acu-composition-smoke.qjs` and refused with "expected: no arguments".
An assertion that carried the child's own stderr would have said so on the
first round. That is now a repository-wide invariant: no gate script may judge
a child by its exit code without carrying that child's output.
