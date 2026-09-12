# Read the Mach timebase ratio with `mach_timebase_info`

On macOS, `MachTimebaseSnapshot::acquire()` is the pointer-free typed API. It
validates that both ratio fields are nonzero and exposes no raw output pointer.

The legacy `dlcall` court independently binds caller-owned storage matching two adjacent `u32` fields
(`numer`, then `denom`) and pass its pointer. A zero status means both fields
now describe the conversion from Mach ticks to nanoseconds.

```lisp
(dlcall "libSystem.B.dylib" "mach_timebase_info" "i32" "ptr" ratio)
```

The ratio is a host fact. It is not a timing policy or a portable duration.
Both paths are retained so the raw ABI layout and typed snapshot remain comparable.
