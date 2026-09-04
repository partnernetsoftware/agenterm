#!/bin/bash
# Reproduce one exact Windows x86_64 agenterm-cu release-size point.

set -euo pipefail

variant="${1:-}"
case "$variant" in
  thin|fat) ;;
  *) echo "usage: research/cu-size-budget/measure.sh thin|fat" >&2; exit 2 ;;
esac

repo="$(git rev-parse --show-toplevel)"
cd "$repo"
if [ -n "$(git status --porcelain=v1)" ]; then
  echo "size court requires an exact clean source tree" >&2
  exit 3
fi

target="x86_64-pc-windows-msvc"
binary="target/$target/release/agenterm-cu.exe"
build=(cargo xwin build --locked -p agenterm-cu --bin agenterm-cu --release --target "$target")
if [ "$variant" = fat ]; then
  build+=(--config 'profile.release.lto="fat"')
fi

# Keep dependency caches but force the owning package through codegen/link so
# elapsed time is not Cargo's no-op freshness check.
cargo clean -p agenterm-cu --release --target "$target" >/dev/null
started="$(date +%s)"
"${build[@]}" >&2
finished="$(date +%s)"

[ -f "$binary" ] || { echo "measured binary is missing" >&2; exit 4; }
bytes="$(wc -c <"$binary" | tr -d ' ')"
digest="$(shasum -a 256 "$binary" | awk '{print $1}')"
sections="$(objdump -h "$binary" | awk '$2 == ".text" || $2 == ".rdata" || $2 == ".pdata" {printf "%s=%d ", substr($2,2), ("0x" $3)+0}')"

python3 - "$variant" "$(git rev-parse HEAD)" "$bytes" "$((finished-started))" "$digest" "$sections" <<'PY'
import json, sys
sections = {}
for item in sys.argv[6].split():
    key, value = item.split("=", 1)
    sections[key] = int(value)
print(json.dumps({
    "schema": 1,
    "variant": sys.argv[1],
    "source_sha": sys.argv[2],
    "source_dirty": False,
    "boundary": "L1 whole agenterm-cu.exe",
    "tool": "wc-c plus objdump-h",
    "build": "release opt-level=z codegen-units=1 panic=abort strip=true",
    "target": "x86_64-pc-windows-msvc",
    "execution": "byte-measurement-only",
    "bytes": int(sys.argv[3]),
    "budget_bytes": 2_097_152,
    "budget_delta_bytes": int(sys.argv[3]) - 2_097_152,
    "package_rebuild_seconds": int(sys.argv[4]),
    "sha256": sys.argv[5],
    "sections": sections,
}, sort_keys=True))
PY
