# lab/mmap-ephemeral — shmbox + courts

Not product code. Standalone package. See [`LAYOUT.md`](LAYOUT.md), [`PRACTICAL.md`](PRACTICAL.md), [`RESULTS.md`](RESULTS.md).

## Layers

| Layer | Role |
|-------|------|
| **`shmbox` lib** | Slot ABI + `WaitKind` + `Endpoint`/`Server`/`Client` |
| **`mmap-lab` bin** | probes / latency bench / RPS / ephemeral spawn |
| **`nng_bench/`** | nng pair `ipc://` opponent |

## Wait backends (`WaitKind::Native`)

| OS | Implementation |
|----|----------------|
| macOS | `os_sync_*_SHARED` |
| Linux | `futex` WAIT/WAKE |
| Windows | `WaitOnAddress` / `WakeByAddressSingle` |

CLI: `--wait yield|native` (`os_sync` is an alias for `native`).

## Build / smoke (macOS)

```bash
cargo build --manifest-path lab/mmap-ephemeral/Cargo.toml --release
./lab/mmap-ephemeral/target/release/mmap-lab probe-native
./lab/mmap-ephemeral/target/release/mmap-lab --wait native rps \
  lab/mmap-ephemeral/rps.slot 20000 500
```

## Court headline

| Gate | Result |
|------|--------|
| Latency p50 vs nng (native/os_sync) | **PASS ~6.6×** |
| RPS vs nng (native/os_sync) | **PASS ~4×** |
