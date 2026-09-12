# Read the Darwin login name with a typed snapshot

Use `LoginNameSnapshot::acquire()` on macOS. It owns a bounded, pointer-free
copy of the native bytes without assuming UTF-8, requires a real NUL
terminator, and preserves a nonzero `getlogin_r` status as a typed OS failure.

The typed court compares success bytes and failure status with an independent
direct native buffer. Other hosts return typed `Unsupported`; the public API
does not expose a caller-owned buffer or arbitrary native invocation.
