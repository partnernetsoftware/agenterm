//! Linux native carrier for the system-activated privilege broker.
//!
//! This module must not be merged into current-user IPC. Its fixed root-owned
//! endpoint and cross-uid peer checks are the mechanism boundary that lets the
//! root broker consult private replay state before asking polkit for consent.

use std::{
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
    SYSTEM_BROKER_SOCKET, SystemBrokerError, SystemBrokerErrorCode, SystemBrokerPeerFacts,
    SystemBrokerResult,
};

const SYSTEMD_FIRST_FD: RawFd = 3;

pub(super) struct SystemBrokerListener {
    listener: std::os::unix::net::UnixListener,
}

impl SystemBrokerListener {
    pub(super) fn from_systemd_activation() -> SystemBrokerResult<Self> {
        require_root("system broker listener")?;
        validate_activation_environment()?;

        // SAFETY: `fcntl(F_GETFD)` only inspects the numeric descriptor.
        if unsafe { libc::fcntl(SYSTEMD_FIRST_FD, libc::F_GETFD) } < 0 {
            return Err(io_error(
                SystemBrokerErrorCode::InvalidActivation,
                "systemd activation descriptor 3 is not open",
                io::Error::last_os_error(),
            ));
        }

        // SAFETY: systemd transfers descriptor 3 to this process when
        // LISTEN_PID/FDS name exactly one descriptor. This function is the
        // only production adopter and therefore becomes its sole Rust owner.
        let descriptor = unsafe { OwnedFd::from_raw_fd(SYSTEMD_FIRST_FD) };
        Self::from_activation_fd(
            descriptor,
            Path::new(SYSTEM_BROKER_SOCKET),
            Path::new("/"),
            0,
        )
    }

    fn from_activation_fd(
        descriptor: OwnedFd,
        endpoint: &Path,
        ancestry_anchor: &Path,
        expected_owner: u32,
    ) -> SystemBrokerResult<Self> {
        let before = validate_endpoint(endpoint, ancestry_anchor, expected_owner)?;
        validate_listener_fd(descriptor.as_raw_fd(), endpoint, expected_owner)?;
        let after = validate_endpoint(endpoint, ancestry_anchor, expected_owner)?;
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
    ) -> SystemBrokerResult<Self> {
        Self::from_activation_fd(descriptor, endpoint, ancestry_anchor, expected_owner)
    }

    pub(super) fn accept(&self, timeout: Duration) -> SystemBrokerResult<SystemBrokerStream> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let peer = authenticated_peer(&stream, PeerUidPolicy::Ordinary)?;
                    return Ok(SystemBrokerStream {
                        stream,
                        facts: peer.facts,
                        pidfd: peer.pidfd,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(SystemBrokerError::new(
                            SystemBrokerErrorCode::AcceptTimeout,
                            "timed out waiting for a system broker peer",
                        ));
                    }
                    poll_fd(self.listener.as_raw_fd(), libc::POLLIN, remaining).map_err(
                        |error| {
                            io_error(
                                SystemBrokerErrorCode::Io,
                                "cannot wait for a system broker peer",
                                error,
                            )
                        },
                    )?;
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
    pidfd: OwnedFd,
}

impl SystemBrokerStream {
    pub(super) fn connect(timeout: Duration) -> SystemBrokerResult<Self> {
        require_ordinary_user("system broker client")?;
        let endpoint = Path::new(SYSTEM_BROKER_SOCKET);
        Self::connect_to(
            endpoint,
            Path::new("/"),
            0,
            PeerUidPolicy::Exact(0),
            timeout,
        )
    }

    fn connect_to(
        endpoint: &Path,
        ancestry_anchor: &Path,
        endpoint_owner: u32,
        peer_policy: PeerUidPolicy,
        timeout: Duration,
    ) -> SystemBrokerResult<Self> {
        let before = validate_endpoint(endpoint, ancestry_anchor, endpoint_owner)?;
        let stream = connect_bounded(endpoint, timeout)?;
        let after = validate_endpoint(endpoint, ancestry_anchor, endpoint_owner)?;
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
            pidfd: peer.pidfd,
        })
    }

    #[cfg(test)]
    fn connect_for_test(
        endpoint: &Path,
        ancestry_anchor: &Path,
        endpoint_owner: u32,
        peer_uid: u32,
        timeout: Duration,
    ) -> SystemBrokerResult<Self> {
        Self::connect_to(
            endpoint,
            ancestry_anchor,
            endpoint_owner,
            PeerUidPolicy::Exact(peer_uid),
            timeout,
        )
    }

    pub(super) fn peer(&self) -> &SystemBrokerPeerFacts {
        &self.facts
    }

    pub(super) fn peer_is_alive(&self) -> SystemBrokerResult<bool> {
        let mut event = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            // SAFETY: `event` is one initialized writable pollfd for this call.
            let result = unsafe { libc::poll(&raw mut event, 1, 0) };
            match result {
                0 => return Ok(true),
                1 if event.revents & libc::POLLIN != 0 => return Ok(false),
                1 if event.revents & libc::POLLNVAL != 0 => {
                    return Err(SystemBrokerError::new(
                        SystemBrokerErrorCode::PeerIdentityUnavailable,
                        "retained system broker peer descriptor is invalid",
                    ));
                }
                1 => {
                    return Err(SystemBrokerError::new(
                        SystemBrokerErrorCode::PeerIdentityUnavailable,
                        "retained system broker peer descriptor reported an unexpected event",
                    ));
                }
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(io_error(
                            SystemBrokerErrorCode::PeerIdentityUnavailable,
                            "cannot inspect retained system broker peer",
                            error,
                        ));
                    }
                }
                _ => unreachable!("poll returns at most the descriptor count"),
            }
        }
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
    pidfd: OwnedFd,
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
    let credentials = socket_peer_credentials(stream.as_raw_fd())?;
    let accepted = match policy {
        PeerUidPolicy::Ordinary => credentials.uid != 0,
        PeerUidPolicy::Exact(expected) => credentials.uid == expected,
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
    let pid = u32::try_from(credentials.pid).map_err(|_| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "system broker peer PID is invalid",
        )
    })?;
    let before = read_process_start_ticks(pid)?;
    let pidfd = open_pidfd(pid)?;
    let after = read_process_start_ticks(pid)?;
    if before != after {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::StalePeer,
            "system broker peer process identity changed during authentication",
        ));
    }
    if !pidfd_alive(&pidfd)? {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::StalePeer,
            "system broker peer exited during authentication",
        ));
    }
    validate_process_effective_ids(pid, credentials.uid, credentials.gid)?;
    Ok(AuthenticatedPeer {
        facts: SystemBrokerPeerFacts {
            process_id: pid,
            effective_user_id: credentials.uid,
            effective_group_id: credentials.gid,
            start_ticks: before,
        },
        pidfd,
    })
}

fn socket_peer_credentials(fd: RawFd) -> SystemBrokerResult<libc::ucred> {
    let mut credentials = MaybeUninit::<libc::ucred>::uninit();
    let mut length = mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the kernel writes at most `length` bytes into the correctly
    // aligned `ucred` storage for this live Unix stream descriptor.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if result != 0 || length as usize != mem::size_of::<libc::ucred>() {
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read Linux system broker peer credentials",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful getsockopt returned exactly one complete `ucred`.
    Ok(unsafe { credentials.assume_init() })
}

fn read_process_start_ticks(pid: u32) -> SystemBrokerResult<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|error| {
        io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read system broker peer process identity",
            error,
        )
    })?;
    parse_process_start_ticks(&stat).ok_or_else(|| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "system broker peer process identity is malformed",
        )
    })
}

fn parse_process_start_ticks(stat: &str) -> Option<u64> {
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn validate_process_effective_ids(pid: u32, uid: u32, gid: u32) -> SystemBrokerResult<()> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).map_err(|error| {
        io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot read system broker peer credentials from procfs",
            error,
        )
    })?;
    let effective_uid = proc_status_effective_id(&status, "Uid:").ok_or_else(|| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "system broker peer effective UID is missing from procfs",
        )
    })?;
    let effective_gid = proc_status_effective_id(&status, "Gid:").ok_or_else(|| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "system broker peer effective GID is missing from procfs",
        )
    })?;
    if effective_uid != uid || effective_gid != gid {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::StalePeer,
            "system broker peer credentials changed during authentication",
        ));
    }
    Ok(())
}

fn proc_status_effective_id(status: &str, key: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn open_pidfd(pid: u32) -> SystemBrokerResult<OwnedFd> {
    // SAFETY: `pidfd_open` receives only a validated positive kernel peer PID
    // and flags zero; no caller pointer crosses the boundary.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::PeerIdentityUnavailable,
            "cannot retain system broker peer process identity",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: a nonnegative pidfd is newly returned and has no other owner.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor as RawFd) })
}

fn pidfd_alive(pidfd: &OwnedFd) -> SystemBrokerResult<bool> {
    let mut event = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: `event` is one initialized writable pollfd for this call.
        let result = unsafe { libc::poll(&raw mut event, 1, 0) };
        match result {
            0 => return Ok(true),
            1 if event.revents & libc::POLLIN != 0 => return Ok(false),
            1 => {
                return Err(SystemBrokerError::new(
                    SystemBrokerErrorCode::PeerIdentityUnavailable,
                    "retained system broker peer descriptor reported an unexpected event",
                ));
            }
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(io_error(
                        SystemBrokerErrorCode::PeerIdentityUnavailable,
                        "cannot inspect retained system broker peer",
                        error,
                    ));
                }
            }
            _ => unreachable!("poll returns at most the descriptor count"),
        }
    }
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
) -> SystemBrokerResult<SocketIdentity> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "system broker socket path is not absolute and normalized",
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
        || metadata.nlink() != 1
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "fixed system broker endpoint is not the expected owned Unix socket",
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
    if !ancestry_anchor.is_absolute()
        || !directory.starts_with(ancestry_anchor)
        || ancestry_anchor
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "system broker socket ancestry anchor is invalid",
        ));
    }
    let mut current = ancestry_anchor.to_path_buf();
    let descendants = directory.strip_prefix(ancestry_anchor).map_err(|_| {
        SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "system broker socket escapes its ancestry anchor",
        )
    })?;
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

fn validate_activation_environment() -> SystemBrokerResult<()> {
    let expected_pid = std::process::id().to_string();
    match (std::env::var("LISTEN_PID"), std::env::var("LISTEN_FDS")) {
        (Ok(pid), Ok(fds)) if pid == expected_pid && fds == "1" => Ok(()),
        _ => Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "system broker requires exactly one systemd activation descriptor for this process",
        )),
    }
}

fn validate_listener_fd(fd: RawFd, endpoint: &Path, expected_owner: u32) -> SystemBrokerResult<()> {
    let status = fstat(fd, SystemBrokerErrorCode::InvalidActivation)?;
    if status.st_uid != expected_owner || (status.st_mode & libc::S_IFMT) != libc::S_IFSOCK {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "systemd activation descriptor is not the expected owned socket",
        ));
    }
    if socket_option(fd, libc::SO_TYPE)? != libc::SOCK_STREAM
        || socket_option(fd, libc::SO_ACCEPTCONN)? != 1
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "systemd activation descriptor is not a listening Unix stream socket",
        ));
    }
    let actual = socket_path(fd)?;
    if actual != endpoint.as_os_str() {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::InvalidActivation,
            "systemd activation descriptor is bound to the wrong socket path",
        ));
    }
    Ok(())
}

fn fstat(fd: RawFd, code: SystemBrokerErrorCode) -> SystemBrokerResult<libc::stat> {
    let mut status = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `status` is aligned writable storage and `fd` remains borrowed.
    if unsafe { libc::fstat(fd, status.as_mut_ptr()) } != 0 {
        return Err(io_error(
            code,
            "cannot inspect system broker descriptor",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful fstat initialized the complete stat value.
    Ok(unsafe { status.assume_init() })
}

fn socket_option(fd: RawFd, option: libc::c_int) -> SystemBrokerResult<libc::c_int> {
    socket_option_for(fd, option, SystemBrokerErrorCode::InvalidActivation)
}

fn socket_option_for(
    fd: RawFd,
    option: libc::c_int,
    code: SystemBrokerErrorCode,
) -> SystemBrokerResult<libc::c_int> {
    let mut value = 0;
    let mut length = mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt receives an initialized length and writable integer.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&raw mut value).cast(),
            &raw mut length,
        )
    } != 0
        || length as usize != mem::size_of::<libc::c_int>()
    {
        return Err(io_error(
            code,
            "cannot inspect system broker socket option",
            io::Error::last_os_error(),
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
    // SAFETY: successful getsockname initialized the returned address bytes;
    // `length` below bounds every read from its path field.
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
    let bytes: Vec<u8> = raw_path[..end].iter().map(|byte| *byte as u8).collect();
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

fn set_cloexec(fd: RawFd) -> SystemBrokerResult<()> {
    // SAFETY: both fcntl operations inspect/update flags on a borrowed fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::InvalidActivation,
            "cannot set close-on-exec on system broker activation descriptor",
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
    if bytes.is_empty()
        || bytes.len() >= mem::size_of::<libc::sockaddr_un>() - mem::size_of::<libc::sa_family_t>()
    {
        return Err(SystemBrokerError::new(
            SystemBrokerErrorCode::UnsafeEndpoint,
            "fixed system broker socket path exceeds the platform limit",
        ));
    }
    // SAFETY: socket has no pointers and returns a fresh descriptor on success.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::Io,
            "cannot create system broker socket",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: the successful socket call returned a fresh unowned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: all-zero is a valid initial sockaddr_un representation.
    let mut address = unsafe { mem::zeroed::<libc::sockaddr_un>() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *destination = source as libc::c_char;
    }
    let address_length = mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    // SAFETY: address_length covers the initialized family, path and NUL only;
    // the descriptor stays owned for the duration of the call.
    let connected = unsafe {
        libc::connect(
            fd,
            (&raw const address).cast(),
            address_length as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(code) if code == libc::EINPROGRESS || code == libc::EWOULDBLOCK)
        {
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
        let socket_error = socket_option_for(fd, libc::SO_ERROR, SystemBrokerErrorCode::Io)?;
        if socket_error != 0 {
            return Err(io_error(
                SystemBrokerErrorCode::Io,
                "cannot connect to fixed system broker socket",
                io::Error::from_raw_os_error(socket_error),
            ));
        }
    }
    // SAFETY: both fcntl operations inspect/update flags on an owned fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io_error(
            SystemBrokerErrorCode::Io,
            "cannot restore blocking system broker stream",
            io::Error::last_os_error(),
        ));
    }
    Ok(std::os::unix::net::UnixStream::from(descriptor))
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
        // SAFETY: `descriptor` is one initialized writable pollfd.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
        match result {
            0 => return Ok(false),
            1 if descriptor.revents & (events | libc::POLLERR | libc::POLLHUP) != 0 => {
                return Ok(true);
            }
            1 => {
                return Err(io::Error::other(
                    "system broker descriptor reported an unexpected event",
                ));
            }
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
        SystemBrokerListener, SystemBrokerStream, parse_process_start_ticks,
        proc_status_effective_id,
    };

    fn private_directory(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("agenterm-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).expect("create private fixture directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("make fixture directory private");
        directory
    }

    #[test]
    fn parses_process_start_ticks_after_a_spaced_command() {
        let stat = "42 (worker name) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765";
        assert_eq!(parse_process_start_ticks(stat), Some(98765));
    }

    #[test]
    fn parses_effective_ids_not_real_ids() {
        let status = "Name:\tworker\nUid:\t1000\t1001\t1002\t1003\nGid:\t2000\t2001\t2002\t2003\n";
        assert_eq!(proc_status_effective_id(status, "Uid:"), Some(1001));
        assert_eq!(proc_status_effective_id(status, "Gid:"), Some(2001));
    }

    #[test]
    fn private_activation_constructor_accepts_an_owned_real_test_socket() {
        let owner = unsafe { libc::geteuid() };
        let directory = private_directory("system-broker-activation");
        let endpoint = directory.join("broker.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&endpoint).expect("bind fixture Unix socket");
        let descriptor: OwnedFd = listener.into();

        let adopted = SystemBrokerListener::from_activation_fd_for_test(
            descriptor, &endpoint, &directory, owner,
        );
        assert!(adopted.is_ok());
        drop(adopted);
        fs::remove_file(&endpoint).expect("remove fixture socket");
        fs::remove_dir(&directory).expect("remove fixture directory");
    }

    #[test]
    fn private_client_authenticates_peer_and_half_close_delivers_eof() {
        let owner = unsafe { libc::geteuid() };
        let directory = private_directory("system-broker-client");
        let endpoint = directory.join("broker.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&endpoint).expect("bind fixture Unix socket");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("read through EOF");
            assert_eq!(request, b"one-frame");
        });

        let mut connected = SystemBrokerStream::connect_for_test(
            &endpoint,
            &directory,
            owner,
            owner,
            Duration::from_secs(1),
        )
        .expect("connect exact test peer");
        assert_eq!(connected.peer().effective_user_id, owner);
        assert!(connected.peer_is_alive().expect("inspect retained peer"));
        connected.write_all(b"one-frame").expect("write request");
        connected.shutdown_write().expect("half-close request");
        server.join().expect("join fixture server");
        drop(connected);
        fs::remove_file(&endpoint).expect("remove fixture socket");
        fs::remove_dir(&directory).expect("remove fixture directory");
    }
}
