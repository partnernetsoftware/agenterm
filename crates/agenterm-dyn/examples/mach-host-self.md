<!-- CU hand: none — a Mach host port is a kernel send right, not a window fact. -->

# Own one `mach_host_self` send-right reference

`mach_host_self()` returns a process-level Mach host-port send right and adds one
user reference on every call. Use the typed owner instead of `Dyn::eval_native`:

```rust
use agenterm_dyn::MachHostPort;

let host = MachHostPort::acquire()?;
let refs = host.send_right_refs()?;
assert!(refs >= 1);
# Ok::<(), agenterm_dyn::MachHostPortError>(())
```

`Drop` pairs this acquisition with exactly one `mach_port_deallocate`. It
releases this owner's send-right reference; it does not close or destroy the
host. The raw Mach port name is never exposed, and non-macOS acquisition returns
`MachHostPortError::Unsupported`.
