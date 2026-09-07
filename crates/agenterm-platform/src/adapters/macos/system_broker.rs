//! macOS native carrier for the launchd-activated privilege broker.
//!
//! Production activation is reachable only through the fixed launchd socket
//! key. Both ends authenticate the peer from `LOCAL_PEERTOKEN`; request fields
//! are never process authority.

use std::{
    ffi::c_char,
    fs,
    io::{self, Read, Write},
    mem::{self, MaybeUninit},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use super::{
    SYSTEM_BROKER_LAUNCHD_SOCKET, SYSTEM_BROKER_SOCKET, SYSTEM_BROKER_SOCKET_MODE,
    SystemBrokerError, SystemBrokerErrorCode, SystemBrokerPeerFacts, SystemBrokerResult,
};

const SYSTEM_BROKER_DIRECTORY: &str = "/private/var/run/agenterm";

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AuditToken {
    value: [u32; 8],
}

unsafe extern "C" {
    static mach_task_self_: u32;
    fn launch_activate_socket(
        name: *const c_char,
        descriptors: *mut *mut libc::c_int,
        count: *mut usize,
    ) -> libc::c_int;
    fn task_name_for_pid(target_task: u32, pid: libc::c_int, task: *mut u32) -> libc::c_int;
    fn task_info(
        task: u32,
        flavor: u32,
        information: *mut libc::c_int,
        count: *mut u32,
    ) -> libc::c_int;
    fn mach_port_deallocate(task: u32, name: u32) -> libc::c_int;
}

const TASK_AUDIT_TOKEN: u32 = 15;
const TASK_AUDIT_TOKEN_COUNT: u32 = 8;

pub(super) struct SystemBrokerListener {
    listener: std::os::unix::net::UnixListener,
}

impl SystemBrokerListener {
    pub(super) fn from_launchd_activation() -> SystemBrokerResult<Self> {
        require_root("system broker listener")?;
        let mut descriptors = std::ptr::null_mut();
        let mut count = 0_usize;
        // SAFETY: the fixed name is NUL-terminated. launchd either leaves the
        // outputs empty or returns a malloc-owned descriptor array.
        let result = unsafe {
            launch_activate_socket(
                SYSTEM_BROKER_LAUNCHD_SOCKET.as_ptr(),
                &raw mut descriptors,
                &raw mut count,
            )
        };
        if result != 0 {
            return Err(io_error(
                SystemBrokerErrorCode::InvalidActivation,
                "launchd did not supply the fixed system broker socket",
                io::Error::from_raw_os_error(result),
            ));
        }
        if descriptors.is_null() || count != 1 {
            close_activation_descriptors(descriptors, count);
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::InvalidActivation,
                "launchd must supply exactly one system broker socket",
            ));
        }
        // SAFETY: success with count one initialized the first descriptor.
        let descriptor = unsafe { *descriptors };
        // SAFETY: launch_activate_socket documents this array as malloc-owned.
        unsafe { libc::free(descriptors.cast()) };
        if descriptor < 0 {
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::InvalidActivation,
                "launchd supplied an invalid system broker descriptor",
            ));
        }
        // SAFETY: the returned descriptor is transferred to the caller.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        Self::from_activation_fd(
            descriptor,
            Path::new(SYSTEM_BROKER_SOCKET),
            Path::new(SYSTEM_BROKER_DIRECTORY),
            0,
            SYSTEM_BROKER_SOCKET_MODE,
        )
    }

    fn from_activation_fd(
        descriptor: OwnedFd,
        endpoint: &Path,
        ancestry_anchor: &Path,
        expected_owner: u32,
        expected_mode: u32,
    ) -> SystemBrokerResult<Self> {
        let before = validate_endpoint(endpoint, ancestry_anchor, expected_owner, expected_mode)?;
        validate_listener_fd(descriptor.as_raw_fd(), endpoint, expected_owner)?;
        let after = validate_endpoint(endpoint, ancestry_anchor, expected_owner, expected_mode)?;
        if before != after {
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::InvalidActivation,
                "system broker socket identity changed during activation validation",
            ));
        }
        set_cloexec(descriptor.as_raw_fd())?;
        let listener = std::os::unix::net::UnixListener::from(descriptor);
        listener.set_nonblocking(true).map_err(|error| {
            io_error(
                SystemBrokerErrorCode::InvalidActivation,
                "cannot make the activated system broker listener nonblocking",
                error,
            )
        })?;
        Ok(Self { listener })
    }

    #[cfg(test)]
    fn from_activation_fd_for_test(
        descriptor: OwnedFd,
        endpoint: &Path,
        ancestry_anchor: &Path,
        expected_owner: u32,
        expected_mode: u32,
    ) -> SystemBrokerResult<Self> {
        Self::from_activation_fd(
            descriptor,
            endpoint,
            ancestry_anchor,
            expected_owner,
            expected_mode,
        )
    }

    pub(super) fn accept(&self, timeout: Duration) -> SystemBrokerResult<SystemBrokerStream> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    return authenticated_peer(&stream, PeerUidPolicy::Ordinary).map(|peer| {
                        SystemBrokerStream {
                            stream,
                            facts: peer.facts,
                            audit_token: peer.audit_token,
                        }
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(SystemBrokerError::new(
                            SystemBrokerErrorCode::AcceptTimeout,
                            "timed out waiting for a system broker peer",
                        ));
                    }
                    if !poll_fd(self.listener.as_raw_fd(), libc::POLLIN, remaining).map_err(
                        |error| {
                            io_error(
                                SystemBrokerErrorCode::Io,
                                "cannot wait for a system broker peer",
                                error,
                            )
                        },
                    )? {
                        return Err(SystemBrokerError::new(
                            SystemBrokerErrorCode::AcceptTimeout,
                            "timed out waiting for a system broker peer",
                        ));
                    }
                }
                Err(error) => {
                    return Err(io_error(
                        SystemBrokerErrorCode::Io,
                        "cannot accept a system broker peer",
                        error,
                    ));
                }
            }
        }
    }
}

pub(super) struct SystemBrokerStream {
    stream: std::os::unix::net::UnixStream,
    facts: SystemBrokerPeerFacts,
    audit_token: AuditToken,
}

impl SystemBrokerStream {
    pub(super) fn connect(timeout: Duration) -> SystemBrokerResult<Self> {
        require_ordinary_user("system broker client")?;
        Self::connect_to(
            Path::new(SYSTEM_BROKER_SOCKET),
            Path::new(SYSTEM_BROKER_DIRECTORY),
            0,
            SYSTEM_BROKER_SOCKET_MODE,
            PeerUidPolicy::Exact(0),
            timeout,
        )
    }

    fn connect_to(
        endpoint: &Path,
        ancestry_anchor: &Path,
        endpoint_owner: u32,
        endpoint_mode: u32,
        peer_policy: PeerUidPolicy,
        timeout: Duration,
    ) -> SystemBrokerResult<Self> {
        let before = validate_endpoint(endpoint, ancestry_anchor, endpoint_owner, endpoint_mode)?;
        let stream = connect_bounded(endpoint, timeout)?;
        let after = validate_endpoint(endpoint, ancestry_anchor, endpoint_owner, endpoint_mode)?;
        if before != after {
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::UnsafeEndpoint,
                "system broker socket identity changed while connecting",
            ));
        }
        let peer = authenticated_peer(&stream, peer_policy)?;
        Ok(Self {
            stream,
            facts: peer.facts,
            audit_token: peer.audit_token,
        })
    }

    #[cfg(test)]
    fn connect_for_test(
        endpoint: &Path,
        ancestry_anchor: &Path,
        endpoint_owner: u32,
        endpoint_mode: u32,
        peer_uid: u32,
        timeout: Duration,
    ) -> SystemBrokerResult<Self> {
        Self::connect_to(
            endpoint,
            ancestry_anchor,
            endpoint_owner,
            endpoint_mode,
            PeerUidPolicy::Exact(peer_uid),
            timeout,
        )
    }

    pub(super) fn peer(&self) -> &SystemBrokerPeerFacts {
        &self.facts
    }

    pub(super) fn peer_is_alive(&self) -> SystemBrokerResult<bool> {
        exact_peer_is_alive(&self.audit_token)
    }

    pub(super) fn set_io_timeout(&self, timeout: Duration) -> SystemBrokerResult<()> {
        self.stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| self.stream.set_write_timeout(Some(timeout)))
            .map_err(|error| {
                io_error(
                    SystemBrokerErrorCode::Io,
                    "cannot set system broker stream timeout",
                    error,
                )
            })
    }

    pub(super) fn shutdown_write(&self) -> SystemBrokerResult<()> {
        self.stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|error| {
                io_error(
                    SystemBrokerErrorCode::Io,
                    "cannot half-close system broker request stream",
                    error,
                )
            })
    }
}

impl Read for SystemBrokerStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buffer)
    }
}

impl Write for SystemBrokerStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

struct AuthenticatedPeer {
    facts: SystemBrokerPeerFacts,
    audit_token: AuditToken,
}

#[derive(Clone, Copy)]
enum PeerUidPolicy {
    Ordinary,
    Exact(u32),
}

fn authenticated_peer(
    stream: &std::os::unix::net::UnixStream,
    policy: PeerUidPolicy,
) -> SystemBrokerResult<AuthenticatedPeer> {
    let token = socket_peer_audit_token(stream.as_raw_fd())?;
    let pid = token.value[5];
    let uid = token.value[1];
    let gid = token.value[2];
    let pid_version = token.value[7];
    if pid == 0 || pid_version == 0 {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "macOS system broker peer audit token has no process generation",
        ));
    }
    let (peer_uid, peer_gid) = socket_peer_effective_ids(stream.as_raw_fd())?;
    if uid != peer_uid || gid != peer_gid {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::StalePeer,
            "macOS system broker peer credentials disagree with its audit token",
        ));
    }
    let accepted = match policy {
        PeerUidPolicy::Ordinary => uid != 0,
        PeerUidPolicy::Exact(expected) => uid == expected,
    };
    if !accepted {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafePeer,
            match policy {
                PeerUidPolicy::Ordinary => "system broker client peer must be an ordinary user",
                PeerUidPolicy::Exact(0) => "system broker server peer is not root",
                PeerUidPolicy::Exact(_) => "system broker peer UID does not match test authority",
            },
        ));
    }
    Ok(AuthenticatedPeer {
        facts: SystemBrokerPeerFacts {
            process_id: pid,
            effective_user_id: uid,
            effective_group_id: gid,
            start_ticks: u64::from(pid_version),
        },
        audit_token: token,
    })
}

fn socket_peer_audit_token(fd: RawFd) -> SystemBrokerResult<AuditToken> {
    let mut token = MaybeUninit::<AuditToken>::uninit();
    let mut length = mem::size_of::<AuditToken>() as libc::socklen_t;
    // SAFETY: the kernel writes at most `length` bytes into aligned storage.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read macOS system broker peer audit token",
            io::Error::last_os_error(),
        ));
    }
    if length as usize != mem::size_of::<AuditToken>() {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "macOS system broker peer audit token has an invalid kernel length",
        ));
    }
    // SAFETY: successful getsockopt returned one complete audit token.
    Ok(unsafe { token.assume_init() })
}

fn socket_peer_effective_ids(fd: RawFd) -> SystemBrokerResult<(u32, u32)> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: getpeereid writes the two initialized scalar outputs only.
    if unsafe { libc::getpeereid(fd, &raw mut uid, &raw mut gid) } != 0 {
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read macOS system broker peer effective credentials",
            io::Error::last_os_error(),
        ));
    }
    Ok((uid, gid))
}

fn exact_peer_is_alive(token: &AuditToken) -> SystemBrokerResult<bool> {
    let pid = libc::c_int::try_from(token.value[5]).map_err(|_| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "macOS system broker peer PID is out of range",
        )
    })?;
    // A task-name port alone is not authority. It is used only to ask XNU for
    // the current audit token and compare its pidversion to the token retained
    // from this socket. Failure is not guessed as exit: task visibility can be
    // denied, so that case remains typed unavailable.
    let self_task = unsafe { mach_task_self_ };
    let mut task = 0_u32;
    let result = unsafe { task_name_for_pid(self_task, pid, &raw mut task) };
    if result != 0 {
        // SAFETY: signal zero performs no mutation and distinguishes a missing
        // numeric PID from a live peer whose task token is not inspectable.
        let alive_by_pid = unsafe { libc::kill(pid, 0) };
        if alive_by_pid != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot obtain the current macOS system broker peer audit token",
            io::Error::from_raw_os_error(result),
        ));
    }
    let mut current = AuditToken::default();
    let mut count = TASK_AUDIT_TOKEN_COUNT;
    let info_result = unsafe {
        task_info(
            task,
            TASK_AUDIT_TOKEN,
            current.value.as_mut_ptr().cast(),
            &raw mut count,
        )
    };
    // SAFETY: task_name_for_pid returned this send right to the current task.
    let _ = unsafe { mach_port_deallocate(self_task, task) };
    if info_result != 0 {
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read the current macOS system broker peer audit token",
            io::Error::from_raw_os_error(info_result),
        ));
    }
    if count != TASK_AUDIT_TOKEN_COUNT {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "current macOS system broker peer audit token has an invalid length",
        ));
    }
    Ok(current == *token)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

fn validate_endpoint(
    path: &Path,
    ancestry_anchor: &Path,
    expected_owner: u32,
    expected_mode: u32,
) -> SystemBrokerResult<SocketIdentity> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || !path.starts_with(ancestry_anchor)
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "system broker socket path is not inside its fixed normalized ancestry",
        ));
    }
    validate_ancestry(
        path.parent().ok_or_else(|| {
            SystemBrokerError::new(
                SystemBrokerErrorCode::UnsafeEndpoint,
                "system broker socket has no parent directory",
            )
        })?,
        ancestry_anchor,
        expected_owner,
    )?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        io_error(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "cannot inspect fixed system broker socket",
            error,
        )
    })?;
    if metadata.file_type().is_symlink()
        || !std::os::unix::fs::FileTypeExt::is_socket(&metadata.file_type())
        || metadata.uid() != expected_owner
        || metadata.mode() & 0o777 != expected_mode
        || metadata.nlink() != 1
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "fixed system broker endpoint is not a protected owned Unix socket",
        ));
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn validate_ancestry(
    directory: &Path,
    ancestry_anchor: &Path,
    expected_owner: u32,
) -> SystemBrokerResult<()> {
    let descendants = directory.strip_prefix(ancestry_anchor).map_err(|_| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "system broker socket escapes its ancestry anchor",
        )
    })?;
    let mut current = ancestry_anchor.to_path_buf();
    for component in std::iter::once(Component::CurDir).chain(descendants.components()) {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => current.push(name),
            _ => {
                return Err(SystemBrokerError::new(
                    SystemBrokerErrorCode::UnsafeEndpoint,
                    "system broker socket ancestry is not normalized",
                ));
            }
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            io_error(
                SystemBrokerErrorCode::UnsafeEndpoint,
                "cannot inspect system broker socket ancestry",
                error,
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != expected_owner
            || metadata.mode() & 0o022 != 0
        {
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::UnsafeEndpoint,
                "system broker socket ancestry is not an owned non-writable real directory",
            ));
        }
    }
    Ok(())
}

fn validate_listener_fd(fd: RawFd, endpoint: &Path, expected_owner: u32) -> SystemBrokerResult<()> {
    let status = fstat(fd)?;
    if status.st_uid != expected_owner || (status.st_mode & libc::S_IFMT) != libc::S_IFSOCK {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "launchd activation descriptor is not the expected owned socket",
        ));
    }
    // Darwin AF_UNIX returns ENOPROTOOPT for SO_ACCEPTCONN. The descriptor's
    // listener provenance is instead the fixed `launch_activate_socket` key;
    // here we independently bind its type, owner and pathname.
    if socket_option(
        fd,
        libc::SO_TYPE,
        SystemBrokerErrorCode::InvalidActivation,
        "cannot inspect system broker socket type",
    )? != libc::SOCK_STREAM
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "launchd activation descriptor is not a Unix stream socket",
        ));
    }
    if socket_path(fd)? != endpoint {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "launchd activation descriptor is bound to the wrong socket path",
        ));
    }
    Ok(())
}

fn fstat(fd: RawFd) -> SystemBrokerResult<libc::stat> {
    let mut status = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: status is aligned writable storage and fd remains borrowed.
    if unsafe { libc::fstat(fd, status.as_mut_ptr()) } != 0 {
        return Err(io_error(
            SystemBrokerErrorCode::InvalidActivation,
            "cannot inspect system broker descriptor",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful fstat initialized the complete stat value.
    Ok(unsafe { status.assume_init() })
}

fn socket_option(
    fd: RawFd,
    option: libc::c_int,
    error_code: SystemBrokerErrorCode,
    error_message: &'static str,
) -> SystemBrokerResult<libc::c_int> {
    let mut value = 0;
    let mut length = mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt receives an initialized length and writable integer.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&raw mut value).cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(io_error(
            error_code,
            error_message,
            io::Error::last_os_error(),
        ));
    }
    if length as usize != mem::size_of::<libc::c_int>() {
        return Err(SystemBrokerError::new(
            error_code,
            format!("{error_message}: kernel returned an invalid option length"),
        ));
    }
    Ok(value)
}

fn socket_path(fd: RawFd) -> SystemBrokerResult<PathBuf> {
    let mut address = MaybeUninit::<libc::sockaddr_un>::zeroed();
    let mut length = mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    // SAFETY: getsockname receives aligned sockaddr storage with its full size.
    if unsafe { libc::getsockname(fd, address.as_mut_ptr().cast(), &raw mut length) } != 0 {
        return Err(io_error(
            SystemBrokerErrorCode::InvalidActivation,
            "cannot inspect activated system broker socket name",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful getsockname initialized the returned address bytes.
    let address = unsafe { address.assume_init() };
    let path_offset = mem::offset_of!(libc::sockaddr_un, sun_path);
    let path_length = (length as usize)
        .checked_sub(path_offset)
        .filter(|length| *length <= address.sun_path.len())
        .ok_or_else(|| {
            SystemBrokerError::new(
                SystemBrokerErrorCode::InvalidActivation,
                "activated system broker socket name has an invalid length",
            )
        })?;
    let raw_path = &address.sun_path[..path_length];
    if address.sun_family != libc::AF_UNIX as libc::sa_family_t
        || raw_path.first().is_none_or(|byte| *byte == 0)
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "activated system broker socket is not a filesystem Unix socket",
        ));
    }
    let end = raw_path
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(raw_path.len());
    let bytes: Vec<u8> = raw_path[..end]
        .iter()
        .map(|byte| byte.to_ne_bytes()[0])
        .collect();
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

fn set_cloexec(fd: RawFd) -> SystemBrokerResult<()> {
    // SAFETY: both fcntl operations inspect/update flags on a borrowed fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::InvalidActivation,
            "cannot set close-on-exec on system broker descriptor",
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn connect_bounded(
    path: &Path,
    timeout: Duration,
) -> SystemBrokerResult<std::os::unix::net::UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() >= mem::size_of::<libc::sockaddr_un>() - 2 {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "fixed system broker socket path exceeds the platform limit",
        ));
    }
    // SAFETY: socket has no pointers and returns a fresh descriptor on success.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::Io,
            "cannot create system broker socket",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful socket returned a fresh descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(fd) };
    set_cloexec(fd)?;
    set_nonblocking(fd, true)?;
    // SAFETY: all-zero is a valid initial sockaddr_un representation.
    let mut address = unsafe { mem::zeroed::<libc::sockaddr_un>() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    address.sun_len = u8::try_from(mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
        .map_err(|_| {
            SystemBrokerError::new(
                SystemBrokerErrorCode::UnsafeEndpoint,
                "fixed system broker socket address exceeds the platform limit",
            )
        })?;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *destination = source as libc::c_char;
    }
    let address_length = libc::socklen_t::from(address.sun_len);
    // SAFETY: address_length covers the initialized family, path and NUL only.
    let connected = unsafe { libc::connect(fd, (&raw const address).cast(), address_length) };
    if connected != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(io_error(
                SystemBrokerErrorCode::Io,
                "cannot connect to fixed system broker socket",
                error,
            ));
        }
        if !poll_fd(fd, libc::POLLOUT, timeout).map_err(|error| {
            io_error(
                SystemBrokerErrorCode::Io,
                "cannot wait for system broker connection",
                error,
            )
        })? {
            return Err(SystemBrokerError::new(
                SystemBrokerErrorCode::ConnectTimeout,
                "timed out connecting to fixed system broker socket",
            ));
        }
        let socket_error = socket_option(
            fd,
            libc::SO_ERROR,
            SystemBrokerErrorCode::Io,
            "cannot inspect system broker connection result",
        )?;
        if socket_error != 0 {
            return Err(io_error(
                SystemBrokerErrorCode::Io,
                "cannot connect to fixed system broker socket",
                io::Error::from_raw_os_error(socket_error),
            ));
        }
    }
    set_nonblocking(fd, false)?;
    Ok(std::os::unix::net::UnixStream::from(descriptor))
}

fn set_nonblocking(fd: RawFd, enabled: bool) -> SystemBrokerResult<()> {
    // SAFETY: fcntl only inspects/updates flags on an owned descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    let selected = if enabled {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, selected) } < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::Io,
            "cannot configure system broker socket blocking mode",
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn poll_fd(fd: RawFd, events: libc::c_short, timeout: Duration) -> io::Result<bool> {
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(false);
        }
        let timeout_ms = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: descriptor is one initialized writable pollfd.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
        match result {
            0 => return Ok(false),
            1 if descriptor.revents & (events | libc::POLLERR | libc::POLLHUP) != 0 => {
                return Ok(true);
            }
            1 => return Err(io::Error::other("unexpected system broker socket event")),
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            _ => unreachable!("poll returns at most the descriptor count"),
        }
    }
}

fn close_activation_descriptors(descriptors: *mut libc::c_int, count: usize) {
    if descriptors.is_null() {
        return;
    }
    // SAFETY: launchd returned an array with `count` descriptor entries.
    let descriptors_slice = unsafe { std::slice::from_raw_parts(descriptors, count) };
    for descriptor in descriptors_slice.iter().copied().filter(|fd| *fd >= 0) {
        // SAFETY: rejection owns every returned activation descriptor.
        let _ = unsafe { libc::close(descriptor) };
    }
    // SAFETY: launch_activate_socket documents this array as malloc-owned.
    unsafe { libc::free(descriptors.cast()) };
}

fn require_root(context: &str) -> SystemBrokerResult<()> {
    // SAFETY: geteuid takes no arguments and has no failure contract.
    if unsafe { libc::geteuid() } == 0 {
        Ok(())
    } else {
        Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafePeer,
            format!("{context} must run as root"),
        ))
    }
}

fn require_ordinary_user(context: &str) -> SystemBrokerResult<()> {
    // SAFETY: geteuid takes no arguments and has no failure contract.
    if unsafe { libc::geteuid() } != 0 {
        Ok(())
    } else {
        Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafePeer,
            format!("{context} must run as an ordinary user"),
        ))
    }
}

fn io_error(
    code: SystemBrokerErrorCode,
    message: &'static str,
    source: io::Error,
) -> SystemBrokerError {
    SystemBrokerError::io(code, format!("{message}: {source}"), source)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read as _, Write as _},
        os::{fd::OwnedFd, unix::fs::PermissionsExt},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        SystemBrokerListener, SystemBrokerStream, exact_peer_is_alive, socket_peer_audit_token,
        validate_endpoint,
    };

    fn private_directory(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos();
        let directory = std::path::PathBuf::from("/private/tmp").join(format!(
            "agt-{}-{:x}-{}",
            std::process::id(),
            nonce,
            &label[..label.len().min(8)]
        ));
        fs::create_dir(&directory).expect("create private fixture directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("make fixture directory private");
        directory
    }

    fn listener_fixture(
        label: &str,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::os::unix::net::UnixListener,
    ) {
        let directory = private_directory(label);
        let endpoint = directory.join("broker.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&endpoint).expect("bind private broker fixture");
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))
            .expect("protect broker fixture socket");
        (directory, endpoint, listener)
    }

    #[test]
    fn activation_rejects_a_world_writable_socket() {
        let owner = unsafe { libc::geteuid() };
        let (directory, endpoint, listener) = listener_fixture("system-broker-mode");
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o622))
            .expect("weaken fixture socket mode");
        let descriptor = OwnedFd::from(listener);
        let error = match SystemBrokerListener::from_activation_fd_for_test(
            descriptor, &endpoint, &directory, owner, 0o600,
        ) {
            Ok(_) => panic!("world-writable socket must be refused"),
            Err(error) => error,
        };
        assert_eq!(error.code(), super::SystemBrokerErrorCode::UnsafeEndpoint);
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn endpoint_rejects_a_symlink_leaf() {
        let owner = unsafe { libc::geteuid() };
        let directory = private_directory("system-broker-symlink");
        let target = directory.join("target.sock");
        let _listener =
            std::os::unix::net::UnixListener::bind(&target).expect("bind private target socket");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("protect target socket");
        let endpoint = directory.join("broker.sock");
        std::os::unix::fs::symlink(&target, &endpoint).expect("create socket symlink");
        let error = validate_endpoint(&endpoint, &directory, owner, 0o600)
            .expect_err("symlink endpoint must be refused");
        assert_eq!(error.code(), super::SystemBrokerErrorCode::UnsafeEndpoint);
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_file(target);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn exact_liveness_rejects_a_different_pid_generation() {
        use std::os::fd::AsRawFd as _;

        let (peer, _other) = std::os::unix::net::UnixStream::pair().expect("create socket pair");
        let mut token = socket_peer_audit_token(peer.as_raw_fd()).expect("read peer audit token");
        token.value[7] = token.value[7].wrapping_add(1);
        assert!(!exact_peer_is_alive(&token).expect("compare exact peer generation"));
    }

    #[test]
    fn authenticated_stream_uses_kernel_peer_token_and_round_trips() {
        let owner = unsafe { libc::geteuid() };
        assert_ne!(owner, 0, "ordinary-user test must not run as root");
        let (directory, endpoint, listener) = listener_fixture("system-broker-peer");
        let descriptor = OwnedFd::from(listener);
        let listener = SystemBrokerListener::from_activation_fd_for_test(
            descriptor, &endpoint, &directory, owner, 0o600,
        )
        .expect("adopt private fixture listener");
        let client_endpoint = endpoint.clone();
        let client_directory = directory.clone();
        let client = std::thread::spawn(move || {
            let mut stream = SystemBrokerStream::connect_for_test(
                &client_endpoint,
                &client_directory,
                owner,
                0o600,
                owner,
                Duration::from_secs(2),
            )
            .expect("connect authenticated fixture client");
            assert_eq!(stream.peer().effective_user_id, owner);
            assert!(stream.peer().process_id > 0);
            assert!(stream.peer().start_ticks > 0);
            assert!(stream.peer_is_alive().expect("inspect exact server peer"));
            stream.write_all(b"ping").expect("write request");
            stream.shutdown_write().expect("half-close request");
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).expect("read response");
            assert_eq!(reply, b"pong");
        });
        let mut stream = listener
            .accept(Duration::from_secs(2))
            .expect("accept authenticated fixture peer");
        assert_eq!(stream.peer().effective_user_id, owner);
        assert!(stream.peer_is_alive().expect("inspect exact client peer"));
        let mut request = Vec::new();
        stream.read_to_end(&mut request).expect("read request");
        assert_eq!(request, b"ping");
        stream.write_all(b"pong").expect("write response");
        stream.shutdown_write().expect("half-close response");
        client.join().expect("join fixture client");
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(directory);
    }
}
