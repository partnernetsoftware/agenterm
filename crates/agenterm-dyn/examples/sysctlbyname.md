# Read the typed `hw.ncpu` snapshot

On macOS, `CpuCountSnapshot` owns the bounded two-stage `sysctlbyname` query,
copies the result into a pointer-free `u32`, and returns typed failures instead
of exposing caller buffers. This API deliberately represents only `hw.ncpu`;
it is not a general `sysctlbyname` interface.

```rust
use agenterm_dyn::CpuCountSnapshot;

let cpu_count = CpuCountSnapshot::acquire()?;
assert!(cpu_count.logical_cpus() > 0);
# Ok::<(), agenterm_dyn::CpuCountError>(())
```
