# Production contract on the same shm mailbox

Landed in `src/slot.rs` (`MMAPEPH\x04`), `src/rpc.rs`, `src/channel.rs`, `src/error.rs`.
Public calls return [`shmbox::Error`](src/error.rs) (`Busy` / `Timeout` / `Rejected` / `Dead` / `AlreadyBound` / `NoOwner`).
`owner_pid` is paired with that process's start-time. A recycled pid does not keep the slot.
A dead caller's in-flight state is reclaimed, not left `Busy`.
`Server` drop clears `owner_pid` and bumps `generation` so an old client sees `Dead`.
`bind` tightens a group/world-writable file to `0o600`, and recreates a short or bad-magic file.

Transport stays: one file-backed slot, `WaitKind::Native`, synchronous `call`.
No second queue, no listen socket. A slow handler blocking `call` is the
contract (backpressure), not a defect.

## Header (`MMAPEPH\x04`, still 64 bytes)

| Field | Role |
|-------|------|
| `magic` `seq` `state` `shutdown` `req_len` `resp_len` | mailbox |
| `owner_pid` `generation` `status` | who serves, and the reply status |
| `owner_birth` | start-time of `owner_pid` (two `u32`) |
| `client_pid` `client_birth` | who holds the in-flight call |

`status`: `Ok` / `Busy` / `Timeout` / `Rejected` / `Dead`.

`seq` is the in-flight id. Client CAS-es `IDLE → CLAIM` (`state` 3), writes
the payload, then publishes `REQ`. Server may publish `RESP` only if `state`
is still `REQ` and `seq` still matches (compare-and-swap). A late handler
must drop its reply. Timeout CAS ignores a newer `seq`.

## Rules

1. **Single flight.** `call` CAS-es `IDLE → CLAIM`, writes the payload, then
   publishes `REQ`. A second `call` sees non-`IDLE` and returns `Busy`.
   Do not wait behind another request.
2. **Deadline.** `call` waits only until its timeout. On expiry, CAS
   `REQ → IDLE` with `status=Timeout` only if `seq` is still this call's id.
   If the server already won the CAS, return the reply (`Ok`), not a timeout.
3. **Dead owner.** `bind` writes `owner_pid` and that process's start-time.
   If the file exists and that pid is dead, or the pid was recycled (start-time
   does not match), bump `generation`, zero `state`/`status`, take the slot.
   If the same process is still alive, `bind` fails. `connect` requires that
   pair to be alive and remembers `generation`; if it changes, `call` returns
   `Dead`.
4. **Handler failure is a reply.** `Ok` or `Rejected` is stored in `status`
   before `state=RESP`. Empty payload is not an error by itself.
5. **File mode.** Create `0o600` (owner read/write only). `bind` tightens a
   looser mode when it can, and replaces a short or bad-magic file. macOS
   rendezvous stays a normal file. `shm_open` stays a probe, not the production path.
6. **Dead caller.** `call` stamps `client_pid` plus start-time after `CLAIM`.
   A later `call` or `serve` that sees that pid gone resets the slot to `IDLE`.
   A panic before `REQ` is published also drops `CLAIM`.

## Address

| Form | Loc |
|------|-----|
| `shmbox:file:/path` / `shmbox:file:rel.slot` | file-backed mmap |
| `shmbox:/abs/path` | shorthand file |
| `shmbox:shm:name` | named shm (probe; Darwin reopen often fails) |

Not used: `ipc://`, `tcp://`, `shm://`. Production on Darwin is `file`.

## Public API

`Endpoint::parse` / `Server::bind` / `Client::connect`.
One-shot: `call` / `serve_one`.
Split flight: client `ask` → `await_reply` / `try_reply`; server `accept` → `reply` / `try_accept`.
Wrong phase → `State`. After `close` → `Closed`. Busy slot → `Busy` (no queue).

## Explicitly not in this step

- Request queue / multi-client fan-in
- A faster transport than this shm slot
- Credentials beyond the slot file mode and `owner_pid`
