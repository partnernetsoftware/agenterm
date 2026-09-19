# lab/mmap-ephemeral — layout

```
lab/mmap-ephemeral/
├── README.md / RESULTS.md / PRACTICAL.md / LAYOUT.md
├── Cargo.toml                 # lib shmbox + bin mmap-lab
├── .cargo/config.toml
├── run_rps_court.sh
├── src/
│   ├── lib.rs
│   ├── channel.rs             # Endpoint / Server / Client
│   ├── slot.rs                # mapping (unix mmap / win CreateFileMapping)
│   ├── wait.rs                # Yield + Native (os_sync|futex|WaitOnAddress)
│   ├── rpc.rs                 # call / serve_one / run_resident
│   ├── ffi/
│   │   ├── mod.rs
│   │   ├── unix.rs            # mmap, shm_open, open flags
│   │   ├── darwin.rs          # os_sync_*
│   │   ├── linux.rs           # futex syscall
│   │   └── windows.rs         # WaitOnAddress + mapping APIs
│   └── bin/mmap_lab.rs
└── nng_bench/
```

## Public API

- Low-level: `Slot`, `SlotLoc`, `WaitKind::{Yield,Native}`
- Practical: `Endpoint`, `Server::bind/serve`, `Client::connect/call`
