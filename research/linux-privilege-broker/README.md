# Linux privilege broker court

This directory records the executable evidence for
`plan/design-linux-privilege-broker-experiment.md`. The production path is the
typed `agenterm-cu privilege plan` / `privilege apply` interface; no separate
research broker is allowed to become a second implementation.

## Court boundary

```text
ordinary graphical user
└─ agenterm-cu privilege apply
   └─ fixed /run/agenterm/cu-privilege.sock
      └─ systemd socket activation as root
         ├─ SO_PEERCRED + start identity + retained pidfd
         ├─ root-only replay ledger lookup
         ├─ polkit exact unix-process authorization for Missing only
         └─ retained exact process effect + verified receipt
```

The Linux package consists of exactly four sealed artifacts under
`packaging/privilege/linux/`: the `agenterm-cu` provider binary, polkit policy,
socket unit and service unit. `install-provider.sh` publishes and rolls back
those four artifacts as one recoverable generation.

## Reproduction shape

From repository root, build the exact Linux artifact, lease one declared court,
install the four sealed artifacts through QGA root authority, and execute the
public client through the court's graphical-session bridge:

```text
cargo zigbuild -p agenterm-cu --release --target <LINUX_TARGET>
../utm-court/bin/utm-court lease <LINUX_COURT>
../utm-court/bin/utm-court interactive-ready <LINUX_COURT> 300
../utm-court/bin/utm-court push <LINUX_COURT> <ARTIFACT> <GUEST_STAGE>
../utm-court/bin/utm-court exec <LINUX_COURT> -- <INSTALLER> install <SEALED_DIGESTS>
../utm-court/bin/utm-court interactive-exec <LINUX_COURT> -- \
  <INSTALLED_AGENTERM_CU> --target current --grant observe privilege plan \
  process.signal <FIXTURE_PID> TERM
```

The real apply call also carries the public `--request-id`, `--session` and
`--session-lease` triple plus the plan's exact `request` and
`approval_digest`. Those values are per-run evidence and are not copied into
documentation. A native password, authentication response or token is never a
fixture, log field or repository input.

The court must release a leased VM after evidence. A missing active graphical
session, unavailable native agent, mixed package generation or unbound target
is a failure, never a skipped success.
