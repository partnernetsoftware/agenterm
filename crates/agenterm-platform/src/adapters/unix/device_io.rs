use std::{
    fs, io,
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use crate::{
    contract::{
        device_inventory::{DeviceIdentityContinuity, DeviceKind, DeviceSelector},
        device_io::*,
        process_observation::ProcessObservation,
    },
    device_inventory::{
        DeviceInventoryError, DeviceInventoryErrorKind, NativeDeviceInventory, NativeDeviceLocator,
        NativeDeviceRecord, error as inventory_error,
    },
    device_io::{error, write_error},
    filesystem::{metadata_is_link_like, protect_private_directory, write_private_atomic},
    process_observation,
};

const FIXTURE_ENABLE_ENV: &str = "AGENTERM_PLATFORM_INTERNAL_DEVICE_FIXTURE";
const FIXTURE_ROOT_ENV: &str = "AGENTERM_PLATFORM_DEVICE_FIXTURE_ROOT";
const FIXTURE_TOKEN_ENV: &str = "AGENTERM_PLATFORM_DEVICE_FIXTURE_TOKEN";
const FIXTURE_REGISTRY_MAX_BYTES: usize = 16 * 1024;
const FIXTURE_TOKEN_BYTES: usize = 32;

pub(crate) struct NativeDeviceIoTestFixture {
    master: OwnedFd,
    registry_path: PathBuf,
    token_digest: String,
    deadline: Instant,
}

pub(crate) struct NativeResolvedDevice {
    path: std::ffi::OsString,
    identity: Vec<u8>,
    device: u64,
    inode: u64,
    raw_device: u64,
}

pub(crate) struct NativeOpenedDevice {
    file: fs::File,
    original: libc::termios,
    restored: bool,
}

pub(crate) fn create_test_fixture(
    registry_root: &Path,
    lifetime: Duration,
) -> Result<(NativeDeviceIoTestFixture, String), DeviceIoError> {
    if std::env::var_os("AGENTERM_CU_INTERNAL_TEST_FIXTURE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
        || lifetime < Duration::from_secs(1)
        || lifetime > Duration::from_secs(300)
    {
        return Err(error(
            DeviceIoErrorKind::InvalidArgument,
            "device-fixture-disabled",
            "internal device fixture requires the explicit test marker and a bounded lifetime",
        ));
    }
    if !registry_root.is_absolute() {
        return Err(error(
            DeviceIoErrorKind::InvalidArgument,
            "device-fixture-root-invalid",
            "device fixture registry root must be absolute",
        ));
    }
    fs::create_dir_all(registry_root).map_err(fixture_io)?;
    protect_private_directory(registry_root).map_err(fixture_io)?;
    let root_metadata = fs::symlink_metadata(registry_root).map_err(fixture_io)?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata_is_link_like(&root_metadata)
        || !root_metadata.is_dir()
        || root_metadata.uid() != effective_uid
        || root_metadata.mode() & 0o777 != 0o700
    {
        return Err(error(
            DeviceIoErrorKind::UnsafeLocator,
            "device-fixture-root-unsafe",
            "device fixture registry root must be one direct private directory",
        ));
    }

    let token_bytes = crate::entropy::secure_random_array::<FIXTURE_TOKEN_BYTES>()
        .map_err(|failure| fixture_failure("device-fixture-entropy", failure.to_string()))?;
    let token = encode_hex(&token_bytes);
    let token_digest = sha256_hex(token.as_bytes());
    let registry_path = registry_root.join(format!("{token}.json"));
    if fs::symlink_metadata(&registry_path).is_ok() {
        return Err(fixture_failure(
            "device-fixture-collision",
            "device fixture registry name is already occupied",
        ));
    }

    let (master, slave) = open_pty_pair()?;
    let locator = tty_path(&slave)?;
    let locator_text = locator.to_str().ok_or_else(|| {
        fixture_failure(
            "device-fixture-locator-invalid",
            "PTY locator is not valid UTF-8",
        )
    })?;
    let metadata = fs::symlink_metadata(&locator).map_err(fixture_io)?;
    if metadata_is_link_like(&metadata) || !metadata.file_type().is_char_device() {
        return Err(fixture_failure(
            "device-fixture-locator-invalid",
            "PTY locator is not one direct character device",
        ));
    }
    set_nonblocking(master.as_raw_fd())?;
    let pid = std::process::id();
    let start_identity = process_observation::start_identity(pid)
        .map_err(|failure| fixture_failure("device-fixture-owner-identity", failure))?;
    let identity = format!("agenterm-platform-device-fixture-v1:{token_digest}");
    let document = serde_json::json!({
        "schema_version": 1,
        "token_digest": token_digest,
        "owner_pid": pid,
        "owner_start_identity": start_identity,
        "locator": locator_text,
        "identity_hex": encode_hex(identity.as_bytes()),
        "device": metadata.dev().to_string(),
        "inode": metadata.ino().to_string(),
        "raw_device": metadata.rdev().to_string(),
    });
    let bytes = serde_json::to_vec(&document).map_err(|failure| {
        fixture_failure("device-fixture-registry-invalid", failure.to_string())
    })?;
    write_private_atomic(&registry_path, &bytes).map_err(fixture_io)?;
    drop(slave);
    let deadline = Instant::now().checked_add(lifetime).ok_or_else(|| {
        fixture_failure(
            "device-fixture-deadline-invalid",
            "fixture deadline overflowed",
        )
    })?;
    Ok((
        NativeDeviceIoTestFixture {
            master,
            registry_path,
            token_digest,
            deadline,
        },
        token,
    ))
}

pub(crate) fn run_test_fixture(fixture: NativeDeviceIoTestFixture) -> Result<(), DeviceIoError> {
    let mut buffer = [0_u8; 64 * 1024];
    let result = 'fixture: loop {
        if matches!(
            fs::symlink_metadata(&fixture.registry_path),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        ) {
            break Ok(());
        }
        if Instant::now() >= fixture.deadline {
            break Ok(());
        }
        // SAFETY: master is live and buffer is writable for its full length.
        let count = unsafe {
            libc::read(
                fixture.master.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if count > 0 {
            let mut offset = 0_usize;
            let count = count as usize;
            while offset < count {
                // SAFETY: the remaining buffer is readable and master is live.
                let written = unsafe {
                    libc::write(
                        fixture.master.as_raw_fd(),
                        buffer[offset..count].as_ptr().cast(),
                        count - offset,
                    )
                };
                if written > 0 {
                    offset += written as usize;
                    continue;
                }
                let failure = io::Error::last_os_error();
                if failure.kind() == io::ErrorKind::WouldBlock {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                break 'fixture Err(fixture_io(failure));
            }
        } else {
            let failure = io::Error::last_os_error();
            if count == 0
                || failure.kind() == io::ErrorKind::WouldBlock
                || failure.raw_os_error() == Some(libc::EIO)
            {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            break Err(fixture_io(failure));
        }
    };
    remove_owned_registry(&fixture.registry_path, &fixture.token_digest);
    result
}

pub(crate) fn append_test_fixture(
    inventory: &mut NativeDeviceInventory,
    selector: DeviceSelector,
) -> Result<(), DeviceInventoryError> {
    if std::env::var_os(FIXTURE_ENABLE_ENV).as_deref() != Some(std::ffi::OsStr::new("1"))
        || !selector.includes(DeviceKind::Usb)
    {
        return Ok(());
    }
    let root = std::env::var_os(FIXTURE_ROOT_ENV).ok_or_else(|| {
        fixture_inventory_error(
            "device-fixture-context-missing",
            "device fixture root is missing",
        )
    })?;
    let token = std::env::var(FIXTURE_TOKEN_ENV).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-context-missing",
            "device fixture token is missing",
        )
    })?;
    validate_fixture_token(&token)?;
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err(fixture_inventory_error(
            "device-fixture-root-invalid",
            "device fixture root must be absolute",
        ));
    }
    let root_metadata = fs::symlink_metadata(&root).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-registry-unavailable",
            "device fixture root is unavailable",
        )
    })?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata_is_link_like(&root_metadata)
        || !root_metadata.is_dir()
        || root_metadata.uid() != effective_uid
        || root_metadata.mode() & 0o777 != 0o700
    {
        return Err(fixture_inventory_error(
            "device-fixture-root-unsafe",
            "device fixture root is not a direct directory",
        ));
    }
    let path = root.join(format!("{token}.json"));
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-registry-unavailable",
            "device fixture registry is unavailable",
        )
    })?;
    // SAFETY: geteuid has no preconditions and does not mutate process state.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata_is_link_like(&metadata)
        || !metadata.is_file()
        || metadata.len() > FIXTURE_REGISTRY_MAX_BYTES as u64
        || metadata.uid() != effective_uid
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(fixture_inventory_error(
            "device-fixture-registry-unsafe",
            "device fixture registry is not a bounded direct file",
        ));
    }
    let bytes = fs::read(&path).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-registry-unavailable",
            "device fixture registry could not be read",
        )
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-registry-invalid",
            "device fixture registry is not valid JSON",
        )
    })?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                fixture_inventory_error(
                    "device-fixture-registry-invalid",
                    "device fixture registry is missing a required string",
                )
            })
    };
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || field("token_digest")? != sha256_hex(token.as_bytes())
    {
        return Err(fixture_inventory_error(
            "device-fixture-registry-invalid",
            "device fixture registry identity does not match its token",
        ));
    }
    let owner_pid = value
        .get("owner_pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            fixture_inventory_error(
                "device-fixture-registry-invalid",
                "device fixture owner pid is invalid",
            )
        })?;
    let owner_identity = field("owner_start_identity")?;
    if !matches!(
        process_observation::observe(owner_pid),
        ProcessObservation::Live { start_identity: Some(actual) } if actual == owner_identity
    ) {
        return Err(fixture_inventory_error(
            "device-fixture-owner-gone",
            "device fixture owner is not the exact live process",
        ));
    }
    let locator = PathBuf::from(field("locator")?);
    let locator_metadata = fs::symlink_metadata(&locator).map_err(|_| {
        fixture_inventory_error(
            "device-fixture-locator-unavailable",
            "device fixture locator is unavailable",
        )
    })?;
    if metadata_is_link_like(&locator_metadata)
        || !locator_metadata.file_type().is_char_device()
        || locator_metadata.dev().to_string() != field("device")?
        || locator_metadata.ino().to_string() != field("inode")?
        || locator_metadata.rdev().to_string() != field("raw_device")?
    {
        return Err(fixture_inventory_error(
            "device-fixture-locator-changed",
            "device fixture locator no longer names the registered character device",
        ));
    }
    let identity_material = decode_hex(field("identity_hex")?).ok_or_else(|| {
        fixture_inventory_error(
            "device-fixture-registry-invalid",
            "device fixture identity encoding is invalid",
        )
    })?;
    inventory.devices.push(NativeDeviceRecord {
        identity_material,
        identity_continuity: DeviceIdentityContinuity::Topology,
        kind: DeviceKind::Usb,
        name: Some("AgenTerm serial court fixture".to_owned()),
        vendor: Some("AgenTerm".to_owned()),
        model: Some("PTY echo".to_owned()),
        transport: Some("private-pty-fixture".to_owned()),
        locator: Some(NativeDeviceLocator {
            value: locator.into_os_string(),
        }),
    });
    Ok(())
}

impl Drop for NativeOpenedDevice {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore(&self.file, &self.original);
        }
    }
}

pub(crate) fn resolve_record(
    record: &NativeDeviceRecord,
) -> Result<NativeResolvedDevice, DeviceIoError> {
    let locator = record.locator.as_ref().ok_or_else(|| {
        error(
            DeviceIoErrorKind::NotClaimable,
            "device-not-claimable",
            "inventory device has no serial character-device endpoint",
        )
    })?;
    let path = std::path::Path::new(&locator.value);
    let metadata = fs::symlink_metadata(path).map_err(map_open_error)?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            DeviceIoErrorKind::UnsafeLocator,
            "device-locator-unsafe",
            "serial locator is a symbolic link",
        ));
    }
    if !metadata.file_type().is_char_device() {
        return Err(error(
            DeviceIoErrorKind::NotCharacterDevice,
            "device-not-character",
            "serial locator is not a character device",
        ));
    }
    Ok(NativeResolvedDevice {
        path: locator.value.clone(),
        identity: record.identity_material.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
        raw_device: metadata.rdev(),
    })
}

pub(crate) fn matches_record(resolved: &NativeResolvedDevice, record: &NativeDeviceRecord) -> bool {
    record.identity_material == resolved.identity
        && record
            .locator
            .as_ref()
            .is_some_and(|locator| locator.value == resolved.path)
}

pub(crate) fn open_exclusive(
    resolved: &NativeResolvedDevice,
    config: SerialConfiguration,
) -> Result<NativeOpenedDevice, DeviceIoError> {
    let baud = baud(config.baud_rate)?;
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::path::Path::new(&resolved.path);
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(error(
            DeviceIoErrorKind::UnsafeLocator,
            "device-locator-unsafe",
            "serial locator contains NUL",
        ));
    }
    let mut c_path = Vec::with_capacity(bytes.len() + 1);
    c_path.extend_from_slice(bytes);
    c_path.push(0);
    // SAFETY: c_path is NUL-terminated; returned descriptor is uniquely owned below.
    let fd = unsafe {
        libc::open(
            c_path.as_ptr().cast(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(map_open_error(io::Error::last_os_error()));
    }
    // SAFETY: successful open transferred this descriptor to us exactly once.
    let file = unsafe { fs::File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(map_open_error)?;
    if !metadata.file_type().is_char_device()
        || metadata.dev() != resolved.device
        || metadata.ino() != resolved.inode
        || metadata.rdev() != resolved.raw_device
    {
        return Err(error(
            DeviceIoErrorKind::IdentityChanged,
            "device-identity-changed",
            "opened character device does not match resolved object identity",
        ));
    }
    // SAFETY: fd is a live descriptor and TIOCEXCL takes no third argument.
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCEXCL as libc::c_ulong) } != 0 {
        return Err(map_exclusive_error(io::Error::last_os_error()));
    }
    let mut original = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes the pointed termios on success.
    if unsafe { libc::tcgetattr(file.as_raw_fd(), original.as_mut_ptr()) } != 0 {
        return Err(error(
            DeviceIoErrorKind::Unsupported,
            "device-serial-unsupported",
            "character device is not a configurable serial TTY",
        ));
    }
    // SAFETY: successful tcgetattr initialized original.
    let original = unsafe { original.assume_init() };
    let mut requested = original;
    // SAFETY: cfmakeraw mutates an initialized termios value.
    unsafe { libc::cfmakeraw(&mut requested) };
    apply_config(&mut requested, config, baud);
    // SAFETY: fd and requested termios are valid.
    if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &requested) } != 0 {
        let _ = restore(&file, &original);
        return Err(error(
            DeviceIoErrorKind::SerialApplyFailed,
            "device-serial-apply-failed",
            "serial configuration could not be applied",
        ));
    }
    let mut actual = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes actual on success.
    if unsafe { libc::tcgetattr(file.as_raw_fd(), actual.as_mut_ptr()) } != 0 {
        let _ = restore(&file, &original);
        return Err(error(
            DeviceIoErrorKind::SerialReadbackMismatch,
            "device-serial-readback-mismatch",
            "serial configuration could not be read back",
        ));
    }
    // SAFETY: successful tcgetattr initialized actual.
    let actual = unsafe { actual.assume_init() };
    if !config_matches(&actual, config, baud) {
        let _ = restore(&file, &original);
        return Err(error(
            DeviceIoErrorKind::SerialReadbackMismatch,
            "device-serial-readback-mismatch",
            "serial configuration readback differed from the request",
        ));
    }
    Ok(NativeOpenedDevice {
        file,
        original,
        restored: false,
    })
}

pub(crate) fn read_once(
    device: &mut NativeOpenedDevice,
    max: usize,
) -> Result<DeviceReadOutcome, DeviceIoError> {
    let mut bytes = vec![0; max];
    // SAFETY: buffer is writable for max bytes and fd is live.
    let count = unsafe { libc::read(device.file.as_raw_fd(), bytes.as_mut_ptr().cast(), max) };
    if count < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            bytes.clear();
            return Ok(DeviceReadOutcome {
                bytes,
                state: DeviceReadState::WouldBlock,
            });
        }
        return Err(error(
            DeviceIoErrorKind::ReadFailed,
            "device-read-failed",
            e.to_string(),
        ));
    }
    bytes.truncate(count as usize);
    let state = if bytes.is_empty() {
        DeviceReadState::EndOfFile
    } else {
        DeviceReadState::Data
    };
    Ok(DeviceReadOutcome { bytes, state })
}

pub(crate) fn write_once(
    device: &mut NativeOpenedDevice,
    bytes: &[u8],
    _timeout_ms: u32,
) -> Result<DeviceWriteOutcome, DeviceIoError> {
    // SAFETY: bytes points to requested readable memory and fd is live.
    let count = unsafe { libc::write(device.file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    if count < 0 {
        return Err(write_error(
            "device-write-failed",
            io::Error::last_os_error().to_string(),
            0,
            true,
            false,
        ));
    }
    let written = count as usize;
    Ok(DeviceWriteOutcome {
        requested_bytes: bytes.len(),
        written_bytes: written,
        delivery: if written == bytes.len() {
            DeviceWriteDelivery::Complete
        } else {
            DeviceWriteDelivery::Partial
        },
    })
}

pub(crate) fn close_restore(mut device: NativeOpenedDevice) -> Result<(), DeviceIoError> {
    let result = restore(&device.file, &device.original);
    device.restored = result.is_ok();
    result
}

fn restore(file: &fs::File, original: &libc::termios) -> Result<(), DeviceIoError> {
    // SAFETY: fd and original termios are valid for this live device.
    if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, original) } != 0 {
        return Err(error(
            DeviceIoErrorKind::SerialRestoreFailed,
            "device-serial-restore-failed",
            io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

fn baud(rate: u32) -> Result<libc::speed_t, DeviceIoError> {
    match rate {
        9600 => Ok(libc::B9600),
        19200 => Ok(libc::B19200),
        38400 => Ok(libc::B38400),
        57600 => Ok(libc::B57600),
        115200 => Ok(libc::B115200),
        230400 => Ok(libc::B230400),
        _ => Err(error(
            DeviceIoErrorKind::Unsupported,
            "device-serial-unsupported",
            format!("unsupported baud rate {rate}"),
        )),
    }
}

fn apply_config(term: &mut libc::termios, c: SerialConfiguration, baud: libc::speed_t) {
    term.c_cflag &= !(libc::CSIZE | libc::PARENB | libc::PARODD | libc::CSTOPB | libc::CRTSCTS);
    term.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    term.c_cflag |= match c.data_bits {
        SerialDataBits::Five => libc::CS5,
        SerialDataBits::Six => libc::CS6,
        SerialDataBits::Seven => libc::CS7,
        SerialDataBits::Eight => libc::CS8,
    };
    match c.parity {
        SerialParity::None => {}
        SerialParity::Even => term.c_cflag |= libc::PARENB,
        SerialParity::Odd => term.c_cflag |= libc::PARENB | libc::PARODD,
    }
    if c.stop_bits == SerialStopBits::Two {
        term.c_cflag |= libc::CSTOPB
    }
    match c.flow_control {
        SerialFlowControl::None => {}
        SerialFlowControl::Software => term.c_iflag |= libc::IXON | libc::IXOFF,
        SerialFlowControl::Hardware => term.c_cflag |= libc::CRTSCTS,
    }
    // SAFETY: term is initialized; baud is one of the platform constants above.
    unsafe {
        libc::cfsetispeed(term, baud);
        libc::cfsetospeed(term, baud);
    }
}

fn config_matches(t: &libc::termios, c: SerialConfiguration, baud: libc::speed_t) -> bool {
    let size = t.c_cflag & libc::CSIZE;
    size==match c.data_bits{SerialDataBits::Five=>libc::CS5,SerialDataBits::Six=>libc::CS6,SerialDataBits::Seven=>libc::CS7,SerialDataBits::Eight=>libc::CS8}
    && ((t.c_cflag&libc::PARENB)!=0)==(c.parity!=SerialParity::None) && ((t.c_cflag&libc::PARODD)!=0)==(c.parity==SerialParity::Odd)
    && ((t.c_cflag&libc::CSTOPB)!=0)==(c.stop_bits==SerialStopBits::Two)
    && match c.flow_control{SerialFlowControl::None=>t.c_iflag&(libc::IXON|libc::IXOFF)==0&&t.c_cflag&libc::CRTSCTS==0,SerialFlowControl::Software=>t.c_iflag&(libc::IXON|libc::IXOFF)==libc::IXON|libc::IXOFF,SerialFlowControl::Hardware=>t.c_cflag&libc::CRTSCTS!=0}
    // SAFETY: t is initialized.
    && unsafe{libc::cfgetispeed(t)}==baud && unsafe{libc::cfgetospeed(t)}==baud
}

fn map_open_error(e: io::Error) -> DeviceIoError {
    let kind = match e.kind() {
        io::ErrorKind::NotFound => DeviceIoErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => DeviceIoErrorKind::PermissionDenied,
        _ => DeviceIoErrorKind::OpenFailed,
    };
    error(
        kind,
        match kind {
            DeviceIoErrorKind::NotFound => "device-not-found",
            DeviceIoErrorKind::PermissionDenied => "device-open-permission",
            _ => "device-open-failed",
        },
        e.to_string(),
    )
}
fn map_exclusive_error(e: io::Error) -> DeviceIoError {
    error(
        if matches!(e.raw_os_error(), Some(libc::EBUSY) | Some(libc::EACCES)) {
            DeviceIoErrorKind::Busy
        } else {
            DeviceIoErrorKind::Unsupported
        },
        if matches!(e.raw_os_error(), Some(libc::EBUSY) | Some(libc::EACCES)) {
            "device-exclusive-busy"
        } else {
            "device-exclusive-unsupported"
        },
        e.to_string(),
    )
}

fn open_pty_pair() -> Result<(OwnedFd, OwnedFd), DeviceIoError> {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: both output pointers are valid; no name/termios/winsize is requested.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(fixture_io(io::Error::last_os_error()));
    }
    // SAFETY: successful openpty transferred two distinct owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) })
}

fn tty_path(slave: &OwnedFd) -> Result<PathBuf, DeviceIoError> {
    let mut bytes = [0_u8; 1024];
    // SAFETY: slave is live and bytes is one writable buffer of the stated size.
    if unsafe { libc::ttyname_r(slave.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(fixture_io(io::Error::last_os_error()));
    }
    let end = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
        fixture_failure(
            "device-fixture-locator-invalid",
            "PTY locator exceeded its bounded native buffer",
        )
    })?;
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| {
        fixture_failure(
            "device-fixture-locator-invalid",
            "PTY locator is not valid UTF-8",
        )
    })?;
    Ok(PathBuf::from(text))
}

fn set_nonblocking(fd: std::os::fd::RawFd) -> Result<(), DeviceIoError> {
    // SAFETY: fd is a live fixture descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(fixture_io(io::Error::last_os_error()));
    }
    Ok(())
}

fn validate_fixture_token(token: &str) -> Result<(), DeviceInventoryError> {
    if token.len() != FIXTURE_TOKEN_BYTES * 2
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Err(fixture_inventory_error(
            "device-fixture-token-invalid",
            "device fixture token is not canonical lowercase hex",
        ))
    } else {
        Ok(())
    }
}

fn remove_owned_registry(path: &Path, token_digest: &str) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata_is_link_like(&metadata)
        || !metadata.is_file()
        || metadata.len() > FIXTURE_REGISTRY_MAX_BYTES as u64
    {
        return;
    }
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if value
        .get("token_digest")
        .and_then(serde_json::Value::as_str)
        == Some(token_digest)
    {
        let _ = fs::remove_file(path);
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.len() > 2 * 1024 {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn fixture_io(failure: io::Error) -> DeviceIoError {
    fixture_failure("device-fixture-io", failure.to_string())
}

fn fixture_failure(code: &'static str, detail: impl Into<String>) -> DeviceIoError {
    error(DeviceIoErrorKind::OpenFailed, code, detail)
}

fn fixture_inventory_error(code: &'static str, detail: impl Into<String>) -> DeviceInventoryError {
    inventory_error(DeviceInventoryErrorKind::ProviderFailed, code, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baud_catalog_rejects_unowned_native_rates() {
        assert_eq!(baud(115_200).unwrap(), libc::B115200);
        assert_eq!(
            baud(123_456).unwrap_err().code(),
            "device-serial-unsupported"
        );
    }
}
