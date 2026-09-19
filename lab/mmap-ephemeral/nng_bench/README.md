# Minimal nng PAIR ping-pong (ipc:// UDS only)

Lab-only. Not product code.

## Build

Requires Homebrew `nng` (`brew install nng`). Compile with system clang:

```bash
cd lab/mmap-ephemeral/nng_bench
make
```

## Run (20 B XOR · n=1000 · warm=1)

```bash
URL='ipc:///tmp/agenterm-mmap-nng-bench.ipc'
rm -f /tmp/agenterm-mmap-nng-bench.ipc
./nng_pair_bench server "$URL" xor &
SPID=$!
sleep 0.05
./nng_pair_bench client "$URL" 1000 20 xor
kill "$SPID" 2>/dev/null || true
rm -f /tmp/agenterm-mmap-nng-bench.ipc
```

Optional 64 B: last args `1000 64 xor`.
Plain echo (misaligned with mmap-lab): use `echo` instead of `xor`.

## RPS (wall-clock throughput)

```bash
URL='ipc:///tmp/agenterm-mmap-nng-rps.ipc'
rm -f /tmp/agenterm-mmap-nng-rps.ipc
./nng_pair_bench server "$URL" xor &
SPID=$!
sleep 0.05
./nng_pair_bench rps "$URL" 100000 1000 20 xor
kill "$SPID" 2>/dev/null || true
rm -f /tmp/agenterm-mmap-nng-rps.ipc
```

Or from parent: `../run_rps_court.sh` (mmap + nng).

Refuse any non-`ipc://` URL.
