# Invoke the Unix variadic `ioctl` ABI

`ioctl` is a dedicated mechanism because its second argument's ABI width varies
by platform and it is variadic. The caller still owns the request semantics and
the complete pointer contract.

```rust
# #[cfg(unix)] {
use agenterm_dyn::{UnixIoctlRequest, invoke_unix_ioctl};

let mut size: libc::winsize = unsafe { std::mem::zeroed() };
// SAFETY: `size` is writable, aligned storage for the selected `TIOCGWINSZ`
// request and remains live for the synchronous call.
let status = unsafe {
    invoke_unix_ioctl(
        libc::STDIN_FILENO,
        UnixIoctlRequest::U64(libc::TIOCGWINSZ),
        (&mut size as *mut libc::winsize).cast(),
    )?
};
# let _ = status;
# }
# Ok::<(), agenterm_dyn::UnixIoctlError>(())
```

On non-Unix targets this entry returns `UnixIoctlError::Unsupported` before
touching the pointer.
