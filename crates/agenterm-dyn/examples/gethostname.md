# Read the host name with `gethostname`

Linux and macOS retain two distinct facts for `gethostname`. The legacy
native-call court binds a writable byte buffer as `name` and its capacity as
`namelen` before evaluation (use `libc.so.6` on Linux):

```lisp
(dlcall "libSystem.B.dylib" "gethostname" "i32" "ptr" name "u64" namelen)
```

A zero result means `name` holds a NUL-terminated host name. That low-level
call returns the status and leaves buffer ownership with its caller.

New Rust consumers should use `HostnameSnapshot::acquire()`. It owns a bounded,
pointer-free copy of the native bytes without assuming UTF-8, rejects a
successful call that did not NUL-terminate its buffer, and returns typed
unsupported or OS failures.
