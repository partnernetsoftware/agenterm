# Snapshot loaded-image facts with `dladdr`

On macOS, `DlAddressSnapshot::current_image()` is the typed default. It queries
only an internal function address and immediately copies the containing image
path and optional symbol name into owned native bytes. It does not expose raw
addresses, borrowed loader pointers, or require those bytes to be UTF-8.

The legacy `dlcall` court remains useful as independent ABI evidence. Its
embedding host binds the address to query as `addr` and writable `Dl_info`
storage as `info` before evaluation.

```lisp
(dlcall "libSystem.B.dylib" "dladdr" "i32" "ptr" addr "ptr" info)
```

A nonzero result means `info` holds borrowed symbol metadata. This low-level
court returns only the status; the host must treat filename and symbol pointers
as borrowed loader storage. Product callers should use the typed snapshot.
