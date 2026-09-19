# lab/mmap-ephemeral — layout

```
lab/mmap-ephemeral/
├── README.md / RESULTS.md / PRACTICAL.md / LAYOUT.md
├── Cargo.toml                 # lib shmbox + bin mmap-lab
├── .cargo/config.toml
├── run_rps_court.sh
├── run_court.sh               # IPC + RPC suite (cargo test --lib court)
├── src/
│   ├── lib.rs
│   ├── address.rs             # shmbox:file:… / shmbox:shm:…
│   ├── channel.rs             # Endpoint / Server / Client
│   ├── court.rs               # combined IPC/RPC tests
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
└── nng_bench/                 # opponent only (nng still uses ipc://)
```

## Public API

- Address: `Address` / `Endpoint::parse` (`shmbox:file:…`, `shmbox:shm:…`)
- Practical: `Server::bind` / `accept`+`reply` / `serve*`; `Client::connect` / `ask`+`await_reply` / `call`
- Low-level: `Slot`, `SlotLoc`, `WaitKind::{Yield,Native}`
