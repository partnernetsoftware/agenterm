# lab/mmap-ephemeral — short-lived process + mmap mailbox

Not product code. Not a PRD leaf. Standalone crate (not a workspace member).

## Question

Can an API-shaped local service avoid listen sockets / named pipes by using:

1. a file-backed mmap slot as the request/response mailbox, and
2. a short-lived `worker` process spawned per call,

and how does that latency compare to a resident process polling the same slot?

## Shape

| Mode | Role |
|------|------|
| `init` | create the 64 KiB slot file |
| `worker` | serve exactly one request, then exit |
| `resident` | loop on the slot until `shutdown` |
| `call [--ephemeral]` | client write/wait (optionally spawn worker) |
| `bench` | warm once, then time ephemeral vs resident |

Toy work in the worker: XOR payload with `0xA5`.

No socket. No named pipe. Coordination is atomics in the mapped header.

## Reproduce

From repository root. Build inside the lab crate so its `.cargo/config.toml`
pins `/usr/bin/clang` (a PATH `cc` shim breaks linking on some hosts):

```bash
cargo build --manifest-path lab/mmap-ephemeral/Cargo.toml --release

SLOT=lab/mmap-ephemeral/bench.slot
rm -f "$SLOT"
./lab/mmap-ephemeral/target/release/mmap-lab bench "$SLOT" 200
```

Optional single-shot:

```bash
./lab/mmap-ephemeral/target/release/mmap-lab init "$SLOT"
./lab/mmap-ephemeral/target/release/mmap-lab call --ephemeral "$SLOT" hello
```

## Non-goals

- Not a general RPC framework
- Not security / multi-writer locking beyond a single-client lab
- Not futex-optimized wait (uses `yield`); absolute numbers are indicative
- Does not touch product crates or CI gates
