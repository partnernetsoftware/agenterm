# Snapshot Unix filesystem facts with `statvfs`

On Linux and macOS, `StatVfsSnapshot::acquire(path)` is the typed entry point.
It accepts native Unix path bytes, rejects an interior NUL before the OS call,
and copies the fixed native output into an owned, pointer-free Rust value. No
`libc::statvfs` structure or raw pointer escapes the call.

The public contract test compares stable root-filesystem fields with an
independent direct `statvfs` call. Capacity counters can change between
observations, so the comparison does not require two complete structures to be
byte-identical. Missing paths preserve a typed OS error, and Windows reports
typed `Unsupported`.
