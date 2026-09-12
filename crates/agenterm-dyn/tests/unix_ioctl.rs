use agenterm_dyn::{UnixIoctlRequest, invoke_unix_ioctl};

#[cfg(not(unix))]
use agenterm_dyn::UnixIoctlError;
#[cfg(not(unix))]
use std::ffi::c_void;

#[test]
fn signed_i32_requests_zero_extend_their_low_bits() {
    assert_eq!(UnixIoctlRequest::I32Bits(-1).as_u64(), u64::from(u32::MAX));
    assert_eq!(UnixIoctlRequest::I32Bits(i32::MIN).as_u64(), 0x8000_0000);
    assert_eq!(UnixIoctlRequest::U64(u64::MAX).as_u64(), u64::MAX);
}

#[cfg(unix)]
mod unix {
    use super::*;

    struct Fd(libc::c_int);

    impl Drop for Fd {
        fn drop(&mut self) {
            if self.0 >= 0 {
                // SAFETY: this test owns descriptors initialized by `openpty`.
                unsafe { libc::close(self.0) };
            }
        }
    }

    #[test]
    fn owned_pty_winsize_crosses_the_variadic_abi() {
        let mut master = -1;
        let mut slave = -1;
        let mut requested = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: every out-pointer is valid and the two returned descriptors
        // are immediately placed under Drop ownership before any assertion.
        let status = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut requested,
            )
        };
        let _master = Fd(master);
        let slave = Fd(slave);
        assert_eq!(status, 0, "openpty must create an owned ioctl fixture");

        let mut observed: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: `observed` is the writable `winsize` required by
        // `TIOCGWINSZ` and remains live until the call returns.
        let result = unsafe {
            invoke_unix_ioctl(
                slave.0,
                UnixIoctlRequest::U64(libc::TIOCGWINSZ),
                (&mut observed as *mut libc::winsize).cast(),
            )
        }
        .expect("Unix target supports its variadic ioctl ABI");

        assert_eq!(result, 0);
        assert_eq!((observed.ws_row, observed.ws_col), (24, 80));
    }
}

#[cfg(not(unix))]
#[test]
fn non_unix_targets_return_typed_unsupported_without_touching_the_pointer() {
    // SAFETY: the non-Unix implementation never dereferences the deliberately
    // dangling pointer and returns before any native call.
    let result = unsafe {
        invoke_unix_ioctl(
            0,
            UnixIoctlRequest::U64(0),
            std::ptr::dangling_mut::<c_void>(),
        )
    };
    assert_eq!(result, Err(UnixIoctlError::Unsupported));
}
