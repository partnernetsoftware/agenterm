# Practical IPC shape (macOS first)

## Recommended product-facing API

Use **`Endpoint` + `Server` / `Client`**, not raw `Slot` in app code:

```rust
use shmbox::{xor_a5, Client, Endpoint, Server, WaitKind};

let ep = Endpoint::file("/path/to/app.slot").with_wait(WaitKind::Native);
// process A
let mut srv = Server::bind(&ep)?;
srv.serve(|buf| { /* mutate in place */ })?;
// process B
let mut cli = Client::connect(&ep)?;
let reply = cli.call(b"...")?;
```

| Choice | Recommendation (macOS) |
|--------|-------------------------|
| Rendezvous | **File** path under app data (not `shm_open`) |
| Wait | **`WaitKind::Native`** (`os_sync`) for idle-friendly; `Yield` only if you accept 100% core |
| Concurrency | **1 client × 1 server** per slot (current ABI) |
| Lifecycle | Server `bind` creates; clients `connect`; shutdown via `request_shutdown` |

## Cross-platform wait map

| `WaitKind::Native` | Mechanism |
|--------------------|-----------|
| macOS | `os_sync_wait_on_address*_SHARED` |
| Linux | `futex(FUTEX_WAIT/WAKE)` shared |
| Windows | `WaitOnAddress` / `WakeByAddressSingle` |

Same mailbox ABI (`MMAPEPH\x02`); only the wait backend switches.

## Not yet (next increments)

- Multi-client queue / ring of slots
- Credential / peer identity
- Automatic reconnect after server restart
- Linux/Windows runtime court numbers on this Mac (compile-check only unless cross-run)
