# PTY process-control decision court

This directory owns the reproducible fixtures and result for
[`plan/design-pty-process-control-experiment.md`](../../plan/design-pty-process-control-experiment.md).

Status: **POSIX forced-cleanup floor green**. `RESULTS.md` preserves the
endpoint-only failure baseline and the post-fix public qjswasm result.
Foreground-signal judgment and Windows runtime evidence remain open, so no
whole-capability promotion is claimed yet.

The court must use owned throwaway processes only. It must never inspect,
signal, activate, or terminate an unrelated user process; its unrelated
control is another fixture child created by the court itself.
