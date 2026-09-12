# Read the host name with `gethostname`

On Linux and macOS, `HostnameSnapshot::acquire()` returns a bounded,
pointer-free copy of the native host-name bytes without requiring UTF-8. It
rejects a successful native call that did not NUL-terminate its buffer and
returns typed unsupported or OS failures instead of exposing partial bytes.

The public contract test compares the snapshot with an independent direct
`gethostname` call on the running Unix host. Windows reports typed
`Unsupported`; no Windows host-name capability is claimed here.
