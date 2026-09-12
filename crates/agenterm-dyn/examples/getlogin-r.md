# Read the login name with `getlogin_r`

macOS retains two distinct facts for `getlogin_r`. The legacy native-call court
binds a caller-owned writable byte buffer and passes its address and capacity.
A zero status means the buffer contains a NUL-terminated login name; `ERANGE`
means retry with a larger buffer.

```lisp
(dlcall "libSystem.B.dylib" "getlogin_r" "i32" "ptr" name "u64" 256)
```

This is a process-session fact probe. The caller owns and bounds its buffer.
New Rust consumers should use `LoginNameSnapshot::acquire()`. It owns a fixed,
bounded, pointer-free copy of the native bytes without assuming UTF-8, rejects
successful output without a real NUL terminator, and returns the nonzero
`getlogin_r` status directly as a typed OS failure. On non-Darwin hosts it
returns typed `Unsupported`.
