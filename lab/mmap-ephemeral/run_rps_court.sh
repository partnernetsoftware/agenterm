#!/usr/bin/env bash
# C-S RPS court: resident mmap (yield + os_sync) vs nng pair ipc://
# Lab-only. From repository root or this directory.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
N=100000
WARM=1000
PAYLOAD=20

cd "$ROOT"
cargo build --release --manifest-path "$ROOT/Cargo.toml" >/dev/null
make -C "$ROOT/nng_bench" >/dev/null

LAB="$ROOT/target/release/mmap-lab"
NNG="$ROOT/nng_bench/nng_pair_bench"
SLOT="$ROOT/rps.slot"
URL='ipc:///tmp/agenterm-mmap-nng-rps.ipc'

echo "=== mmap RPS (n=$N warm=$WARM payload=${PAYLOAD}B) ==="
rm -f "$SLOT"
"$LAB" --wait both rps "$SLOT" "$N" "$WARM"

echo "=== nng RPS (n=$N warm=$WARM payload=${PAYLOAD}B) ==="
rm -f /tmp/agenterm-mmap-nng-rps.ipc
"$NNG" server "$URL" xor >/dev/null &
SPID=$!
sleep 0.05
"$NNG" rps "$URL" "$N" "$WARM" "$PAYLOAD" xor
kill "$SPID" 2>/dev/null || true
wait "$SPID" 2>/dev/null || true
rm -f /tmp/agenterm-mmap-nng-rps.ipc

echo "=== done (see RESULTS.md to record) ==="
# silence unused
: "$REPO"
