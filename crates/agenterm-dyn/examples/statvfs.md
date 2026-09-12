# Snapshot Unix filesystem facts with `statvfs`

On Linux and macOS, `StatVfsSnapshot::acquire(path)` is the typed entry point.
It accepts native Unix path bytes, rejects an interior NUL before the OS call,
and copies the fixed native output into an owned, pointer-free Rust value. No
`libc::statvfs` structure or raw pointer escapes the call.

The matching-host native-call courts also retain the lower-level `dlcall`
fact. The embedding Rust host binds a NUL-terminated path as `root` and aligned
writable `libc::statvfs` storage as `info`, retaining both through the call:

```lisp
(dlcall "<matching Unix libc>" "statvfs" "i32"
  "ptr" root
  "ptr" info)
```

A zero status initializes the caller-owned structure. Linux uses `libc.so.6`;
macOS uses `libSystem.B.dylib`. Block availability and other capacity counters
can change between observations, so the courts compare stable fields such as
block size, fragment size, and name limit instead of requiring two complete
structures to be byte-identical. The call opens no caller-owned file descriptor
and returns no Mach right. Windows reports typed `Unsupported`.
