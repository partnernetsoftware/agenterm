# agenterm-ape.com — experimental six-cell APE launcher

This directory reproduces MiniCon's proven delivery outcome inside agenterm:
one unsigned `.com` APE that embeds all six native cells and dispatches the
correct payload at runtime.

**Naming:** Windows release archives already ship `agenterm.com` as the CUI
forwarder. The experimental cross-platform launcher is `agenterm-ape.com` so
the two artifacts do not collide.

## Layout

```
agenterm-ape.com   ← cosmocc fat APE (when cosmocc is installed)
cells/
  osx-aarch64/agenterm
  osx-x86_64/agenterm
  lnx-aarch64/agenterm
  lnx-x86_64/agenterm
  win-aarch64/agenterm.exe
  win-x86_64/agenterm.exe
```

Without cosmocc, `pack.sh` emits `dist/agenterm-ape-loader` for host testing;
set `AGENTERM_APE_CELLS` to `dist/cells`.

## Pack

From repository root, after `client-build-all`:

```bash
AGENTERM_BOOTSTRAP_TASK=client-build-all ./scripts/bootstrap.sh release-fast
bash research/agenterm-com-loader/pack.sh
```

Optional: `COSMOCC_DIR=~/cosmocc` when cosmocc is not in the default location.

## Qualify

Six-cell archives and checksum sidecars:

```bash
AGENTERM_BOOTSTRAP_TASK=package-six-cell-delivery ./scripts/bootstrap.sh release-fast
```

Receipt: `target/qualification/six-cell/delivery-manifest.json`
