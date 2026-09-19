# lab/mmap-ephemeral — bench receipt

## Host

| Field | Value |
|-------|-------|
| CPU | Apple M4 Pro |
| OS | macOS 26.5.1 (25F80) |
| Arch | arm64 |
| SDK | Xcode MacOSX.sdk (via `xcrun --show-sdk-path`) |
| nng | Homebrew 1.12.4 (`brew install nng`) |

## Build

```bash
cargo build --manifest-path lab/mmap-ephemeral/Cargo.toml --release
# from lab/mmap-ephemeral/nng_bench:
make
```

- Profile: `[profile.release]` in `lab/mmap-ephemeral/Cargo.toml`
  (`opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = true`)
- Linker: `lab/mmap-ephemeral/.cargo/config.toml` → `/usr/bin/clang`
- Artifact: `lab/mmap-ephemeral/target/release/mmap-lab`
- nng C: `/usr/bin/clang -O3` + Homebrew `libnng`
- Zero crates.io deps in mmap-lab (raw libc FFI)

## Probes

```bash
./lab/mmap-ephemeral/target/release/mmap-lab probe-os-sync
# -> os_sync: available (dlsym resolved wait/wake symbols)

./lab/mmap-ephemeral/target/release/mmap-lab probe-shm
# -> shm_open: reopen FAILED after create (errno=13) on this host
```

`os_sync_*` APIs are present (macOS 14.4+ / `__API_AVAILABLE`). Cross-process
waits use `OS_SYNC_WAIT_ON_ADDRESS_SHARED` / `OS_SYNC_WAKE_BY_ADDRESS_SHARED`.

POSIX `shm_open`: create succeeds; any reopen of the same name returns
`EACCES` (same process or peer). Not usable here as a multi-process mailbox.
File-backed `mmap` `MAP_SHARED` remains the working path.

## Bench command (reproducible)

### mmap (resident · both waits)

```bash
SLOT=lab/mmap-ephemeral/bench.slot
rm -f "$SLOT"
./lab/mmap-ephemeral/target/release/mmap-lab bench --skip-ephemeral "$SLOT" 1000
```

### nng (pair · ipc:// · XOR)

```bash
URL='ipc:///tmp/agenterm-mmap-nng-bench.ipc'
rm -f /tmp/agenterm-mmap-nng-bench.ipc
./lab/mmap-ephemeral/nng_bench/nng_pair_bench server "$URL" xor &
SPID=$!
sleep 0.1
./lab/mmap-ephemeral/nng_bench/nng_pair_bench client "$URL" 1000 20 xor
kill "$SPID" 2>/dev/null || true
rm -f /tmp/agenterm-mmap-nng-bench.ipc
```

### Caliber (win-nng series)

| Item | Value |
|------|-------|
| n | 1000 timed samples per series |
| warm | 1 untimed iteration before timed samples |
| payload | 20 bytes (`hello-mmap-ephemeral`); nng also 64 B |
| mmap slot | file, 64 KiB, header magic `MMAPEPH\x02` |
| toy work | XOR each payload byte with `0xA5` (mmap + nng both) |
| mmap waits | `yield` + `os_sync` (resident only for gate) |
| nng | pair0, `ipc://` (UDS only; tcp refused by bench) |
| build | release / `-O3` |

## Numbers (file slot · nng ipc · 2026-09-18)

### mmap resident (n=1000 · 20 B)

```
bench: slot=lab/mmap-ephemeral/bench.slot n=1000 payload=20B warm=1 release strip waits=[Yield, OsSync]
resident(mmap,Yield)     : n=1000 min=125ns p50=292ns p95=417ns max=54.542µs mean=457ns
resident(mmap,OsSync)     : n=1000 min=4.834µs p50=7.375µs p95=9.041µs max=15.208µs mean=7.511µs
```

### nng pair ipc (n=1000 · XOR aligned)

```
nng(pair,ipc,xor=1): n=1000 payload=20B warm=1 min=29.000µs p50=49.000µs p95=75.000µs max=202.000µs mean=50.604µs
nng(pair,ipc,xor=1): n=1000 payload=64B warm=1 min=21.000µs p50=30.000µs p95=40.000µs max=90.000µs mean=31.031µs
```

### Compact (20 B · gate)

| Series | p50 | p95 |
|--------|-----|-----|
| resident mmap + yield | 292 ns | 417 ns |
| resident mmap + os_sync | 7.375 µs | 9.041 µs |
| nng pair ipc + xor | 49.000 µs | 75.000 µs |

### Ratio vs nng (20 B p50)

| Mine | mine_p50 | nng_p50 / mine_p50 | Gate (≥2×) |
|------|----------|--------------------|------------|
| os_sync (primary) | 7.375 µs | **6.64×** | **PASS** |
| yield (reference) | 292 ns | **168×** | PASS |

## Verdict

**PASS — 赢 nng (os_sync)**

Primary gate: resident file-mmap + `os_sync` is ≥2× faster than nng pair
`ipc://` on p50 (measured 6.64×). Yield also clears the same numeric gate
(168×) but spins while idle; practical path is os_sync.

Alignment: both sides XOR payload with `0xA5`. No echo-only misalignment in
this receipt.

## C-S RPS court (2026-09-18)

Wall-clock requests/sec: one resident server, one client, sequential RPC
(same XOR toy work). Not multi-connection fan-out.

```bash
# mmap
./lab/mmap-ephemeral/target/release/mmap-lab --wait both rps \
  lab/mmap-ephemeral/rps.slot 100000 1000

# nng
cd lab/mmap-ephemeral/nng_bench && make
URL='ipc:///tmp/agenterm-mmap-nng-rps.ipc'
./nng_pair_bench server "$URL" xor &
./nng_pair_bench rps "$URL" 100000 1000 20 xor
```

Or: `lab/mmap-ephemeral/run_rps_court.sh`

### Numbers (n=100000 · warm=1000 · 20 B)

```
mmap-rps(resident,Yield):  n=100000 warm=1000 payload=20B elapsed=37.45075ms  rps=2670173
mmap-rps(resident,OsSync): n=100000 warm=1000 payload=20B elapsed=748.025083ms rps=133685
nng-rps(pair,ipc,xor=1):   n=100000 warm=1000 payload=20B elapsed=2.986927s   rps=33479
```

| Path | RPS | vs nng |
|------|-----|--------|
| mmap + yield | 2.67M | ~80× |
| mmap + os_sync (primary) | **134k** | **~4.0×** |
| nng pair ipc | 33.5k | 1× |

**PASS — 赢 nng RPS (os_sync)** (≥2× gate; measured ~4×).

## Earlier receipt (n=200 · mmap only · kept for history)

```
bench: slot=bench.slot n=200 payload=20B warm=1 release strip waits=[Yield, OsSync]
ephemeral(spawn+mmap,Yield): n=200 min=1.152167ms p50=1.236ms p95=1.435666ms max=1.617709ms mean=1.261923ms
ephemeral(spawn+mmap,OsSync): n=200 min=1.131292ms p50=1.209417ms p95=1.409041ms max=1.909375ms mean=1.237633ms
resident(mmap,Yield)     : n=200 min=166ns p50=292ns p95=417ns max=958ns mean=283ns
resident(mmap,OsSync)     : n=200 min=5.75µs p50=7.292µs p95=8.083µs max=12.667µs mean=7.359µs
```

## Interpretation (lab only)

- Ephemeral latency is dominated by process spawn (~1.2 ms); wait strategy is
  noise at that scale.
- Resident + yield is a hot spin: sub-µs RT, high CPU.
- Resident + os_sync is ~25× slower than yield here but does not burn a core
  while idle; still clears a comfortable margin over nng pair UDS.
- nng numbers are indicative same-host ping-pong, not a product SLO.
- Header ABI bumped to magic `\x02` when wait/wake + shm probe landed.

## Not done / assumptions

- Library split (`shmbox` + `mmap-lab`) is structural only; numbers above were
  taken before/after with the same ABI (`MMAPEPH\x02`).
- No listen-socket / named-pipe opponent beyond this nng pair lane.
- No anonymous `MAP_ANON` cross-process path (needs fd inheritance / Mach
  memory entry; out of scope).
- shm backend kept as probe + explicit failure, not a passing bench lane.
- Single client; no multi-writer locking.
- Linux `futex` / Windows `WaitOnAddress` backends not implemented yet.
