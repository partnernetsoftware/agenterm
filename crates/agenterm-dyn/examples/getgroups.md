# Read Unix supplementary groups with `getgroups`

On Linux and macOS, `SupplementaryGroups::acquire()` is the typed, bounded,
pointer-free API for reading the current process's supplementary group IDs.
It retries a changing two-call `getgroups` snapshot only within a fixed bound
and reports typed OS, size-limit, and instability failures.

The matching-host `dlcall` smoke remains separate evidence that the native
symbol is callable. On macOS, the embedding Rust host binds the integer capacity as
`ngroups_max` and aligned writable `gid_t` storage as `gids`, retaining both
through unsafe evaluation.

```lisp
(dlcall "libSystem.B.dylib" "getgroups" "i32" "i32" ngroups_max "ptr" gids)
```

A non-negative result is the number of groups written into the caller-owned
array; `-1` remains the native failure result. dyn does not allocate or retain
that array. The call opens no caller-owned file descriptor and returns no Mach
right. The typed API owns its `Vec<u32>` snapshot rather than exposing this
caller-owned buffer.
