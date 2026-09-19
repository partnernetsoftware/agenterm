# Production contract on the same shm mailbox

Landed in `src/slot.rs` (`MMAPEPH\x03`), `src/rpc.rs`, `src/channel.rs`.
`cargo test --manifest-path lab/mmap-ephemeral/Cargo.toml` covers xor, timeout-then-retry, rejected, and dead-owner reclaim.

Transport stays: one file-backed slot, `WaitKind::Native`, synchronous `call`.
No second queue, no listen socket. A slow handler blocking `call` is the
contract (backpressure), not a defect.

## Header (`MMAPEPH\x03`, still 64 bytes)

Reuse the current 32-byte pad. Do not grow the slot.

| Field | Now (`\x02`) | Production (`\x03`) |
|-------|----------------|---------------------|
| `magic` `seq` `state` `shutdown` `req_len` `resp_len` | kept | kept |
| pad | unused | `owner_pid` `generation` `status` + reserved |

`status`: `Ok` / `Busy` / `Timeout` / `Rejected` / `Dead`.

`seq` is the in-flight id. Client stores it before `state=REQ`. Server may
publish `RESP` only if `state` is still `REQ` and `seq` still matches
(compare-and-swap). A late handler must drop its reply.

## Rules

1. **Single flight.** `call` when `state != IDLE` returns `Busy` immediately.
   Do not wait behind another request.
2. **Deadline.** `call` waits only until its timeout. On expiry, CAS
   `REQ → IDLE` with `status=Timeout` if `seq` still matches. If the server
   already won the CAS, return the reply (`Ok`), not a timeout.
3. **Dead owner.** `bind` writes `owner_pid`. If the file exists and that pid
   is not alive, bump `generation`, zero `state`/`status`, take the slot.
   If the pid is alive, `bind` fails. `connect` remembers `generation`; if it
   changes, `call` returns `Dead` (caller reconnects).
4. **Handler failure is a reply.** `Ok` or `Rejected` is stored in `status`
   before `state=RESP`. Empty payload is not an error by itself.
5. **File mode.** Create `0o600` (owner read/write only). macOS rendezvous
   stays a normal file. `shm_open` stays a probe, not the production path.

## Explicitly not in this step

- Request queue / multi-client fan-in
- A faster transport than this shm slot
- Credentials beyond the slot file mode and `owner_pid`
