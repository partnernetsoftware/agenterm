//! macOS application facts from bounded bundle and process discovery.
//!
//! The adapter uses CoreFoundation, Security.framework, and libproc directly.
//! It does not invoke `plutil`, `codesign`, or a shell.

#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::ffi::{CStr, OsStr, c_char, c_int, c_void};
use std::fs::Metadata;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::contract::app_facts::{
    AppFacts, AppFactsError, AppFactsErrorKind, AppFactsOptions, Fact,
};

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type CfUrlRef = *const c_void;
type CfBundleRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfDataRef = *const c_void;
type SecStaticCodeRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_URL_POSIX_PATH_STYLE: isize = 0;
const PROC_ALL_PIDS: u32 = 1;
const PROC_PIDT_SHORTBSDINFO: c_int = 13;
const MAX_INSTALLED_APPS: usize = 4_096;
const MAX_PROCESS_ENTRIES: usize = 32_768;
const MAX_NATIVE_TEXT_BYTES: usize = 64 * 1024;
const MAX_ENTITLEMENT_KEYS: usize = 256;
const MAX_CDHASH_BYTES: usize = 64;
const ERR_SEC_CS_UNSIGNED: i32 = -67_062;
const SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;
const SEC_CS_CHECK_ALL_ARCHITECTURES: u32 = 1 << 0;
const SEC_CS_NO_NETWORK_ACCESS: u32 = 1 << 29;

const SEARCH_ROOTS: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];

#[allow(clippy::duplicated_attributes)]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CfTypeRef);
    fn CFGetTypeID(value: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        allocator: CfTypeRef,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: u8,
    ) -> CfStringRef;
    fn CFStringGetLength(value: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        capacity: isize,
        encoding: u32,
    ) -> u8;
    fn CFURLCreateWithFileSystemPath(
        allocator: CfTypeRef,
        path: CfStringRef,
        style: isize,
        directory: u8,
    ) -> CfUrlRef;
    fn CFURLCopyFileSystemPath(url: CfUrlRef, style: isize) -> CfStringRef;
    fn CFBundleCreate(allocator: CfTypeRef, url: CfUrlRef) -> CfBundleRef;
    fn CFBundleGetIdentifier(bundle: CfBundleRef) -> CfStringRef;
    fn CFBundleGetValueForInfoDictionaryKey(bundle: CfBundleRef, key: CfStringRef) -> CfTypeRef;
    fn CFBundleCopyExecutableURL(bundle: CfBundleRef) -> CfUrlRef;
    fn CFDictionaryGetCount(dictionary: CfDictionaryRef) -> isize;
    fn CFDictionaryGetValue(dictionary: CfDictionaryRef, key: CfTypeRef) -> CfTypeRef;
    fn CFDictionaryGetKeysAndValues(
        dictionary: CfDictionaryRef,
        keys: *mut CfTypeRef,
        values: *mut CfTypeRef,
    );
    fn CFDataGetLength(data: CfDataRef) -> isize;
    fn CFDataGetBytePtr(data: CfDataRef) -> *const u8;
    static kSecCodeInfoEntitlements: CfStringRef;
    static kSecCodeInfoEntitlementsDict: CfStringRef;
    static kSecCodeInfoIdentifier: CfStringRef;
    static kSecCodeInfoTeamIdentifier: CfStringRef;
    static kSecCodeInfoUnique: CfStringRef;
    fn SecStaticCodeCreateWithPath(path: CfUrlRef, flags: u32, code: *mut SecStaticCodeRef) -> i32;
    fn SecCodeCopySigningInformation(
        code: SecStaticCodeRef,
        flags: u32,
        information: *mut CfDictionaryRef,
    ) -> i32;
    fn SecStaticCodeCheckValidity(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: CfTypeRef,
    ) -> i32;

}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listpids(kind: u32, type_info: u32, buffer: *mut c_void, size: c_int) -> c_int;
    fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, size: c_int)
    -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
}

unsafe extern "C" {
    fn geteuid() -> u32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcBsdShortInfo {
    _pid: u32,
    _parent_pid: u32,
    _process_group: u32,
    _status: u32,
    _command: [c_char; 16],
    _flags: u32,
    uid: u32,
    _gid: u32,
    _real_uid: u32,
    _real_gid: u32,
    _saved_uid: u32,
    _saved_gid: u32,
    _reserved: u32,
}

const _: () = assert!(std::mem::size_of::<ProcBsdShortInfo>() == 64);

struct OwnedCf(CfTypeRef);

impl OwnedCf {
    fn new(value: CfTypeRef) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: `OwnedCf` is constructed only from Create/Copy-rule values.
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}

struct Candidate {
    path: PathBuf,
    bundle_identity: FileIdentity,
    info_identity: FileIdentity,
}

struct BundleFacts {
    name: String,
    identifier: Fact<String>,
    version: Fact<String>,
    executable_path: Option<PathBuf>,
    executable: Fact<String>,
}

struct SigningFacts {
    signature: Fact<String>,
    verified: Fact<bool>,
    entitlements: Fact<Vec<String>>,
}

fn error(kind: AppFactsErrorKind, code: &'static str, message: impl Into<String>) -> AppFactsError {
    AppFactsError::new(kind, code, message)
}

fn cf_string(text: &str) -> Result<OwnedCf, AppFactsError> {
    let length = isize::try_from(text.len()).map_err(|_| {
        error(
            AppFactsErrorKind::InvalidInput,
            "app_facts_invalid_selector",
            "application text is too large for CoreFoundation",
        )
    })?;
    // SAFETY: the byte slice is live for the call and its length is exact.
    OwnedCf::new(unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            length,
            K_CF_STRING_ENCODING_UTF8,
            0,
        )
    })
    .ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_bundle_unreadable",
            "CoreFoundation could not encode application metadata",
        )
    })
}

fn cf_string_value(value: CfTypeRef) -> Option<String> {
    // SAFETY: non-null CoreFoundation objects permit type-id inspection, and
    // subsequent calls are made only after proving this value is a CFString.
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let length = unsafe { CFStringGetLength(value) };
    if length < 0 {
        return None;
    }
    let maximum = unsafe { CFStringGetMaximumSizeForEncoding(length, K_CF_STRING_ENCODING_UTF8) };
    if maximum < 0 || usize::try_from(maximum).ok()? >= MAX_NATIVE_TEXT_BYTES {
        return None;
    }
    let capacity = maximum.checked_add(1)?;
    let mut bytes = vec![0_u8; usize::try_from(capacity).ok()?];
    // SAFETY: the buffer is writable for `capacity` bytes and the value is a CFString.
    if unsafe {
        CFStringGetCString(
            value,
            bytes.as_mut_ptr().cast(),
            capacity,
            K_CF_STRING_ENCODING_UTF8,
        )
    } == 0
    {
        return None;
    }
    let end = bytes.iter().position(|byte| *byte == 0)?;
    String::from_utf8(bytes[..end].to_vec()).ok()
}

fn cf_url(path: &Path, directory: bool) -> Result<OwnedCf, AppFactsError> {
    let path = path.to_str().ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_non_utf8_path",
            "application path is not valid UTF-8",
        )
    })?;
    let text = cf_string(path)?;
    // SAFETY: `text` is a live CFString and all other arguments are value types.
    OwnedCf::new(unsafe {
        CFURLCreateWithFileSystemPath(
            std::ptr::null(),
            text.0,
            K_CF_URL_POSIX_PATH_STYLE,
            u8::from(directory),
        )
    })
    .ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_bundle_unreadable",
            "CoreFoundation could not create an application URL",
        )
    })
}

fn path_from_url(url: CfUrlRef) -> Result<PathBuf, AppFactsError> {
    // SAFETY: callers pass a live CFURL. The returned string follows the Copy rule.
    let text = OwnedCf::new(unsafe { CFURLCopyFileSystemPath(url, K_CF_URL_POSIX_PATH_STYLE) })
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_scan_failed",
                "CoreFoundation could not decode an application URL",
            )
        })?;
    cf_string_value(text.0).map(PathBuf::from).ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_non_utf8_path",
            "application URL is not a bounded UTF-8 path",
        )
    })
}

fn default_application_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = SEARCH_ROOTS.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join("Applications"));
    }
    roots
}

fn application_roots() -> Vec<PathBuf> {
    std::env::var_os("AGENTERM_APP_FACTS_ROOTS").map_or_else(default_application_roots, |roots| {
        std::env::split_paths(&roots)
            .filter(|path| path.is_absolute())
            .collect()
    })
}

fn candidate(path: &Path) -> Result<Candidate, AppFactsError> {
    let path = std::fs::canonicalize(path).map_err(|_| {
        error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to an installed macOS bundle",
        )
    })?;
    let bundle_metadata = std::fs::symlink_metadata(&path).map_err(|_| {
        error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to an installed macOS bundle",
        )
    })?;
    let info_path = path.join("Contents/Info.plist");
    let info_metadata = std::fs::symlink_metadata(&info_path).map_err(|_| {
        error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application bundle has no regular Contents/Info.plist",
        )
    })?;
    if !bundle_metadata.is_dir()
        || bundle_metadata.file_type().is_symlink()
        || !info_metadata.is_file()
        || info_metadata.file_type().is_symlink()
    {
        return Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to a regular macOS bundle",
        ));
    }
    Ok(Candidate {
        path,
        bundle_identity: FileIdentity::from_metadata(&bundle_metadata),
        info_identity: FileIdentity::from_metadata(&info_metadata),
    })
}

fn paths_for_bundle_identifier(
    identifier: &str,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>, AppFactsError> {
    let mut visited = 0_usize;
    let mut paths = Vec::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => {
                return Err(error(
                    AppFactsErrorKind::Io,
                    "app_facts_scan_failed",
                    format!(
                        "could not read macOS application directory {}: {cause}",
                        root.display()
                    ),
                ));
            }
        };
        for entry in entries {
            visited = visited.saturating_add(1);
            if visited > MAX_INSTALLED_APPS {
                return Err(error(
                    AppFactsErrorKind::ScanTruncated,
                    "app_facts_scan_truncated",
                    format!("macOS application scan exceeded {MAX_INSTALLED_APPS} entries"),
                ));
            }
            let entry = entry.map_err(|cause| {
                error(
                    AppFactsErrorKind::Io,
                    "app_facts_scan_failed",
                    format!("could not inspect an application directory entry: {cause}"),
                )
            })?;
            if entry.path().extension() != Some(OsStr::new("app")) {
                continue;
            }
            let Ok(candidate) = candidate(&entry.path()) else {
                continue;
            };
            let Ok(facts) = read_bundle(&candidate) else {
                continue;
            };
            if facts.identifier.value.as_deref() == Some(identifier) {
                paths.push(candidate.path);
            }
        }
    }
    Ok(paths)
}

fn paths_for_name(name: &str, roots: &[PathBuf]) -> Result<Vec<PathBuf>, AppFactsError> {
    let expected = format!("{name}.app");
    let mut visited = 0_usize;
    let mut paths = Vec::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => {
                return Err(error(
                    AppFactsErrorKind::Io,
                    "app_facts_scan_failed",
                    format!(
                        "could not read macOS application directory {}: {cause}",
                        root.display()
                    ),
                ));
            }
        };
        for entry in entries {
            visited = visited.saturating_add(1);
            if visited > MAX_INSTALLED_APPS {
                return Err(error(
                    AppFactsErrorKind::ScanTruncated,
                    "app_facts_scan_truncated",
                    format!("macOS application scan exceeded {MAX_INSTALLED_APPS} entries"),
                ));
            }
            let entry = entry.map_err(|cause| {
                error(
                    AppFactsErrorKind::Io,
                    "app_facts_scan_failed",
                    format!("could not inspect an application directory entry: {cause}"),
                )
            })?;
            if entry.file_name() == std::ffi::OsStr::new(&expected) {
                paths.push(entry.path());
            }
        }
    }
    Ok(paths)
}

fn resolve(selector: &str, roots: &[PathBuf]) -> Result<Candidate, AppFactsError> {
    let paths = if selector.contains('/') {
        vec![PathBuf::from(selector)]
    } else if selector.contains('.') {
        paths_for_bundle_identifier(selector, &default_application_roots())?
    } else {
        paths_for_name(selector, roots)?
    };
    let mut canonical = BTreeSet::new();
    for path in paths {
        if let Ok(path) = std::fs::canonicalize(path) {
            canonical.insert(path);
        }
    }
    match canonical.len() {
        0 => Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector matched no normalized path, bundle identifier, or exact Name",
        )),
        1 => candidate(canonical.first().expect("one candidate")),
        count => Err(error(
            AppFactsErrorKind::Ambiguous,
            "app_facts_ambiguous",
            format!("application selector matched {count} macOS bundles"),
        )),
    }
}

fn bundle_value(bundle: CfBundleRef, key: &str) -> Result<Option<String>, AppFactsError> {
    let key_name = key;
    let key = cf_string(key_name)?;
    // SAFETY: both bundle and key are live for this borrowed Get-rule lookup.
    let value = unsafe { CFBundleGetValueForInfoDictionaryKey(bundle, key.0) };
    if value.is_null() {
        return Ok(None);
    }
    cf_string_value(value).map(Some).ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_bundle_unreadable",
            format!("application bundle value {key_name} is not a bounded UTF-8 string"),
        )
    })
}

fn read_bundle(candidate: &Candidate) -> Result<BundleFacts, AppFactsError> {
    let url = cf_url(&candidate.path, true)?;
    // SAFETY: `url` is a live file URL. The returned bundle follows the Create rule.
    let bundle =
        OwnedCf::new(unsafe { CFBundleCreate(std::ptr::null(), url.0) }).ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_bundle_unreadable",
                "CFBundleCreate could not read the application bundle",
            )
        })?;
    // SAFETY: the retained bundle is live; the returned identifier is borrowed.
    let identifier = unsafe { CFBundleGetIdentifier(bundle.0) };
    let identifier = if identifier.is_null() {
        Fact::absent("bundle-declares-no-CFBundleIdentifier")
    } else {
        Fact::present(cf_string_value(identifier).ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_bundle_unreadable",
                "CFBundleIdentifier is not a bounded UTF-8 string",
            )
        })?)
    };
    let name = bundle_value(bundle.0, "CFBundleDisplayName")?
        .or(bundle_value(bundle.0, "CFBundleName")?)
        .or_else(|| {
            candidate
                .path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_non_utf8_path",
                "application bundle name is not valid UTF-8",
            )
        })?;
    let version = bundle_value(bundle.0, "CFBundleShortVersionString")?
        .or(bundle_value(bundle.0, "CFBundleVersion")?)
        .map_or_else(|| Fact::absent("bundle-declares-no-version"), Fact::present);
    let declared_executable = bundle_value(bundle.0, "CFBundleExecutable")?;
    // SAFETY: the retained bundle is live; a non-null URL follows the Copy rule.
    let executable_url = unsafe { CFBundleCopyExecutableURL(bundle.0) };
    let executable_path = OwnedCf::new(executable_url)
        .map(|url| path_from_url(url.0))
        .transpose()?
        .and_then(|path| std::fs::canonicalize(path).ok())
        .or_else(|| {
            declared_executable.and_then(|name| {
                std::fs::canonicalize(candidate.path.join("Contents/MacOS").join(name)).ok()
            })
        });
    let executable = match executable_path.as_ref().and_then(|path| path.to_str()) {
        Some(path) => Fact::present(path.to_owned()),
        None => Fact::unavailable("bundle-declares-no-executable"),
    };
    Ok(BundleFacts {
        name,
        identifier,
        version,
        executable_path,
        executable,
    })
}

fn running_fact(executable: Option<&PathBuf>) -> Result<Fact<bool>, AppFactsError> {
    let Some(executable) = executable else {
        return Ok(Fact::unavailable("bundle-declares-no-executable"));
    };
    let mut pids = vec![0_i32; MAX_PROCESS_ENTRIES + 1];
    let size = c_int::try_from(pids.len() * std::mem::size_of::<i32>()).map_err(|_| {
        error(
            AppFactsErrorKind::ScanTruncated,
            "app_facts_scan_truncated",
            "macOS process inventory buffer is too large",
        )
    })?;
    // SAFETY: the buffer is writable for exactly `size` bytes.
    let written = unsafe { proc_listpids(PROC_ALL_PIDS, 0, pids.as_mut_ptr().cast(), size) };
    if written < 0 {
        return Ok(Fact::unavailable("macos-process-inventory-unavailable"));
    }
    let count = usize::try_from(written).unwrap_or(0) / std::mem::size_of::<i32>();
    if count > MAX_PROCESS_ENTRIES {
        return Err(error(
            AppFactsErrorKind::ScanTruncated,
            "app_facts_scan_truncated",
            format!("macOS process scan exceeded {MAX_PROCESS_ENTRIES} entries"),
        ));
    }
    let expected = executable.as_os_str().as_encoded_bytes();
    let mut unreadable = 0_usize;
    let mut path_bytes = vec![0_u8; 4_096];
    // `app-facts` describes the caller's application session. Filtering the
    // libproc snapshot by effective uid prevents unrelated protected system
    // processes from turning every honest negative result into unavailable.
    // SAFETY: geteuid has no arguments and no failure mode.
    let effective_uid = unsafe { geteuid() };
    for pid in pids.into_iter().take(count).filter(|pid| *pid > 0) {
        let mut info = std::mem::MaybeUninit::<ProcBsdShortInfo>::zeroed();
        let info_size = c_int::try_from(std::mem::size_of::<ProcBsdShortInfo>()).expect("size");
        // SAFETY: the output points to exactly one writable short BSD record.
        let info_written = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDT_SHORTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                info_size,
            )
        };
        if info_written != info_size {
            // The snapshot entry either exited or is not observable to this
            // caller. It cannot be established as a current-user process.
            continue;
        }
        // SAFETY: proc_pidinfo reported that it initialized the complete record.
        let info = unsafe { info.assume_init() };
        if info.uid != effective_uid {
            continue;
        }
        path_bytes.fill(0);
        // SAFETY: the path buffer is writable for the supplied capacity.
        let length =
            unsafe { proc_pidpath(pid, path_bytes.as_mut_ptr().cast(), path_bytes.len() as u32) };
        if length <= 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(2 | 3)) {
                // ENOENT means the process image was replaced or removed;
                // ESRCH means the snapshot entry exited. Neither can match
                // the retained on-disk executable identity observed here.
                continue;
            }
            unreadable = unreadable.saturating_add(1);
            continue;
        }
        let Ok(path) = CStr::from_bytes_until_nul(&path_bytes) else {
            unreadable = unreadable.saturating_add(1);
            continue;
        };
        let process_path = Path::new(OsStr::from_bytes(path.to_bytes()));
        let process_path = match std::fs::canonicalize(process_path) {
            Ok(path) => path,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                unreadable = unreadable.saturating_add(1);
                continue;
            }
        };
        if process_path.as_os_str().as_encoded_bytes() == expected {
            return Ok(Fact::present(true));
        }
    }
    Ok(if unreadable == 0 {
        Fact::present(false)
    } else {
        Fact::unavailable("macos-process-inventory-incomplete")
    })
}

fn dictionary_value(dictionary: CfDictionaryRef, key: CfStringRef) -> CfTypeRef {
    if dictionary.is_null() || key.is_null() {
        return std::ptr::null();
    }
    // SAFETY: both values are live CoreFoundation references and the returned
    // value remains borrowed from the retained dictionary.
    unsafe { CFDictionaryGetValue(dictionary, key) }
}

fn data_hex(value: CfTypeRef) -> Result<Option<String>, AppFactsError> {
    if value.is_null() {
        return Ok(None);
    }
    // SAFETY: `value` is non-null and type inspection does not retain it.
    if unsafe { CFGetTypeID(value) } != unsafe { CFDataGetTypeID() } {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "code-signing cdhash is not CFData",
        ));
    }
    // SAFETY: `value` has been proven to be CFData.
    let length = usize::try_from(unsafe { CFDataGetLength(value) }).map_err(|_| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "code-signing cdhash has an invalid length",
        )
    })?;
    if length == 0 || length > MAX_CDHASH_BYTES {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!("code-signing cdhash must contain 1..={MAX_CDHASH_BYTES} bytes"),
        ));
    }
    // SAFETY: the CFData remains retained through use of its borrowed bytes.
    let bytes = unsafe { CFDataGetBytePtr(value) };
    if bytes.is_null() {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "code-signing cdhash bytes are unavailable",
        ));
    }
    // SAFETY: the pointer is non-null and CFData reported exactly `length`
    // initialized bytes; the explicit ceiling bounds the borrowed slice.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let mut hex = String::with_capacity(length * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(Some(hex))
}

fn entitlement_keys(information: CfDictionaryRef) -> Result<Fact<Vec<String>>, AppFactsError> {
    // SAFETY: these framework-owned key references have process lifetime.
    let dictionary = dictionary_value(information, unsafe { kSecCodeInfoEntitlementsDict });
    if dictionary.is_null() {
        let raw = dictionary_value(information, unsafe { kSecCodeInfoEntitlements });
        return Ok(if raw.is_null() {
            Fact::present(Vec::new())
        } else if unsafe { CFGetTypeID(raw) } != unsafe { CFDataGetTypeID() } {
            Fact::unavailable("entitlements-value-is-not-data")
        } else {
            Fact::unavailable("entitlements-not-in-dictionary-form")
        });
    }
    // SAFETY: the non-null value remains borrowed from retained information.
    if unsafe { CFGetTypeID(dictionary) } != unsafe { CFDictionaryGetTypeID() } {
        return Ok(Fact::unavailable("entitlements-not-in-dictionary-form"));
    }
    // SAFETY: the value has been proven to be a CFDictionary.
    let count = usize::try_from(unsafe { CFDictionaryGetCount(dictionary) }).unwrap_or(usize::MAX);
    if count > MAX_ENTITLEMENT_KEYS {
        return Err(error(
            AppFactsErrorKind::ScanTruncated,
            "app_facts_scan_truncated",
            format!("application signature has more than {MAX_ENTITLEMENT_KEYS} entitlements"),
        ));
    }
    let mut keys = vec![std::ptr::null(); count];
    let mut values = vec![std::ptr::null(); count];
    // SAFETY: both vectors have exactly the dictionary's reported bounded
    // count, and the dictionary remains live while its borrowed keys are read.
    unsafe {
        CFDictionaryGetKeysAndValues(dictionary, keys.as_mut_ptr(), values.as_mut_ptr());
    }
    let mut names = Vec::with_capacity(count);
    for key in keys {
        let Some(key) = cf_string_value(key) else {
            return Ok(Fact::unavailable("entitlement-key-is-not-a-string"));
        };
        names.push(key);
    }
    names.sort();
    names.dedup();
    Ok(Fact::present(names))
}

fn verification_failure(status: i32) -> Fact<bool> {
    if matches!(status, -67_061 | -67_057 | -67_056 | -67_055 | -67_054) {
        Fact::present(false)
    } else {
        Fact::unavailable(format!(
            "signature-validation-failed-with-OSStatus-{status}"
        ))
    }
}

fn signing_facts(path: &Path, options: AppFactsOptions) -> Result<SigningFacts, AppFactsError> {
    if !options.signing && !options.verify && !options.entitlements {
        return Ok(SigningFacts {
            signature: Fact::not_requested("signing-was-not-requested"),
            verified: Fact::not_requested("signature-verification-was-not-requested"),
            entitlements: Fact::not_requested("entitlements-were-not-requested"),
        });
    }
    let url = cf_url(path, true)?;
    let mut code = std::ptr::null();
    // SAFETY: the URL is retained, and Security initializes a create-rule code
    // reference only on success.
    let create_status = unsafe { SecStaticCodeCreateWithPath(url.0, 0, &raw mut code) };
    if create_status == ERR_SEC_CS_UNSIGNED {
        return Ok(unsigned_signing_facts(options));
    }
    if create_status != 0 || code.is_null() {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!("SecStaticCodeCreateWithPath failed with OSStatus {create_status}"),
        ));
    }
    let code = OwnedCf(code);
    // SAFETY: the static code remains retained. No requirement is supplied,
    // and the no-network flag keeps explicit verification host-local.
    let verify_status = options.verify.then(|| unsafe {
        SecStaticCodeCheckValidity(
            code.0,
            SEC_CS_CHECK_ALL_ARCHITECTURES | SEC_CS_NO_NETWORK_ACCESS,
            std::ptr::null(),
        )
    });
    if verify_status == Some(ERR_SEC_CS_UNSIGNED) {
        return Ok(unsigned_signing_facts(options));
    }
    if let Some(status) = verify_status.filter(|status| *status != 0) {
        let reason = format!("signature-validation-failed-with-OSStatus-{status}");
        return Ok(SigningFacts {
            signature: if options.signing || options.verify {
                Fact::unavailable(reason.clone())
            } else {
                Fact::not_requested("signing-was-not-requested")
            },
            verified: verification_failure(status),
            entitlements: if options.entitlements {
                Fact::unavailable(reason)
            } else {
                Fact::not_requested("entitlements-were-not-requested")
            },
        });
    }
    let mut information = std::ptr::null();
    // SAFETY: the code remains retained and Security initializes one
    // create-rule dictionary on success.
    let information_status = unsafe {
        SecCodeCopySigningInformation(code.0, SEC_CS_SIGNING_INFORMATION, &raw mut information)
    };
    if information_status == ERR_SEC_CS_UNSIGNED {
        return Ok(unsigned_signing_facts(options));
    }
    if information_status != 0 || information.is_null() {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!("SecCodeCopySigningInformation failed with OSStatus {information_status}"),
        ));
    }
    let information = OwnedCf(information);
    // SAFETY: the create-rule result is retained and non-null.
    if unsafe { CFGetTypeID(information.0) } != unsafe { CFDictionaryGetTypeID() } {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "SecCodeCopySigningInformation returned a non-dictionary result",
        ));
    }
    // SAFETY: Security.framework owns these static key references for process
    // lifetime; values remain borrowed from the retained information map.
    let identifier = dictionary_value(information.0, unsafe { kSecCodeInfoIdentifier });
    if identifier.is_null() {
        return Ok(unsigned_signing_facts(options));
    }
    let identifier = cf_string_value(identifier).ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "code-signing identifier is not a bounded UTF-8 string",
        )
    })?;
    let team_value = dictionary_value(information.0, unsafe { kSecCodeInfoTeamIdentifier });
    let team = if team_value.is_null() {
        "none".to_owned()
    } else {
        cf_string_value(team_value).ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_signature_query_failed",
                "code-signing Team identifier is not a bounded UTF-8 string",
            )
        })?
    };
    let cdhash = data_hex(dictionary_value(information.0, unsafe {
        kSecCodeInfoUnique
    }))?;
    let signature = cdhash.map_or_else(
        || Fact::unavailable("code-signing-cdhash-unavailable"),
        |cdhash| {
            Fact::present(format!(
                "identifier={identifier};team={team};cdhash={cdhash}"
            ))
        },
    );
    Ok(SigningFacts {
        signature: if options.signing || options.verify {
            signature
        } else {
            Fact::not_requested("signing-was-not-requested")
        },
        verified: verify_status.map_or_else(
            || Fact::not_requested("signature-verification-was-not-requested"),
            |status| Fact::present(status == 0),
        ),
        entitlements: if options.entitlements {
            entitlement_keys(information.0)?
        } else {
            Fact::not_requested("entitlements-were-not-requested")
        },
    })
}

fn unsigned_signing_facts(options: AppFactsOptions) -> SigningFacts {
    SigningFacts {
        signature: if options.signing || options.verify {
            Fact::absent("bundle-is-not-code-signed")
        } else {
            Fact::not_requested("signing-was-not-requested")
        },
        verified: if options.verify {
            Fact::absent("bundle-is-not-code-signed")
        } else {
            Fact::not_requested("signature-verification-was-not-requested")
        },
        entitlements: if options.entitlements {
            Fact::absent("bundle-is-not-code-signed")
        } else {
            Fact::not_requested("entitlements-were-not-requested")
        },
    }
}

fn revalidate(candidate: &Candidate) -> Result<(), AppFactsError> {
    let bundle = std::fs::symlink_metadata(&candidate.path)
        .map(|metadata| FileIdentity::from_metadata(&metadata));
    let info = std::fs::symlink_metadata(candidate.path.join("Contents/Info.plist"))
        .map(|metadata| FileIdentity::from_metadata(&metadata));
    if matches!(bundle, Ok(identity) if identity == candidate.bundle_identity)
        && matches!(info, Ok(identity) if identity == candidate.info_identity)
    {
        Ok(())
    } else {
        Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            "application bundle identity changed during application-facts observation",
        ))
    }
}

fn query_in(
    selector: &str,
    options: AppFactsOptions,
    roots: &[PathBuf],
) -> Result<AppFacts, AppFactsError> {
    let candidate = resolve(selector, roots)?;
    let facts = read_bundle(&candidate)?;
    let executable_identity = facts
        .executable_path
        .as_ref()
        .map(|path| {
            std::fs::metadata(path)
                .map(|metadata| FileIdentity::from_metadata(&metadata))
                .map_err(|cause| {
                    error(
                        AppFactsErrorKind::IdentityDrift,
                        "app_facts_identity_drift",
                        format!("application executable disappeared before observation: {cause}"),
                    )
                })
        })
        .transpose()?;
    let running = running_fact(facts.executable_path.as_ref())?;
    let signing = signing_facts(&candidate.path, options)?;
    revalidate(&candidate)?;
    if let (Some(path), Some(identity)) = (&facts.executable_path, executable_identity) {
        let current =
            std::fs::metadata(path).map(|metadata| FileIdentity::from_metadata(&metadata));
        if !matches!(current, Ok(current) if current == identity) {
            return Err(error(
                AppFactsErrorKind::IdentityDrift,
                "app_facts_identity_drift",
                "application executable identity changed during application-facts observation",
            ));
        }
    }
    let path = candidate.path.to_str().ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_non_utf8_path",
            "application bundle path is not valid UTF-8",
        )
    })?;
    Ok(AppFacts {
        schema_version: 1,
        platform: "macos".to_owned(),
        selector: selector.to_owned(),
        desktop_entry_id: Fact::not_applicable("macos-bundles-have-no-xdg-desktop-entry"),
        name: Fact::present(facts.name),
        bundle: facts.identifier,
        path: Fact::present(path.to_owned()),
        version: facts.version,
        executable: facts.executable,
        running,
        signature: signing.signature,
        signature_verified: signing.verified,
        entitlements: signing.entitlements,
    })
}

pub(crate) fn query(selector: &str, options: AppFactsOptions) -> Result<AppFacts, AppFactsError> {
    query_in(selector, options, &application_roots())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-macos-app-facts-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn fixture_app(root: &Path, name: &str) -> PathBuf {
        let app = root.join(format!("{name}.app"));
        let executable = app.join("Contents/MacOS/facts");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::copy("/usr/bin/true", &executable).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>CFBundleIdentifier</key><string>org.example.facts</string>\n\
             <key>CFBundleName</key><string>{name}</string>\n\
             <key>CFBundleShortVersionString</key><string>1.2.3</string>\n\
             <key>CFBundleExecutable</key><string>facts</string>\n\
             <key>CFBundlePackageType</key><string>APPL</string>\n\
             </dict></plist>\n"
        );
        std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        app
    }

    #[test]
    fn path_and_name_selectors_return_bundle_facts() {
        let root = fixture_root();
        let app = fixture_app(&root, "Fixture Facts");
        for selector in [app.to_str().unwrap(), "Fixture Facts"] {
            let facts = query_in(
                selector,
                AppFactsOptions::default(),
                std::slice::from_ref(&root),
            )
            .unwrap();
            assert_eq!(facts.platform, "macos");
            assert_eq!(facts.name.value.as_deref(), Some("Fixture Facts"));
            assert_eq!(facts.bundle.value.as_deref(), Some("org.example.facts"));
            assert_eq!(facts.version.value.as_deref(), Some("1.2.3"));
            assert!(
                facts
                    .executable
                    .value
                    .as_deref()
                    .unwrap()
                    .ends_with("/facts")
            );
            assert_eq!(facts.running.status, crate::app_facts::FactStatus::Present);
            assert_eq!(facts.running.value, Some(false));
            assert_eq!(
                facts.signature.status,
                crate::app_facts::FactStatus::NotRequested
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_and_ambiguous_selectors_fail_typed() {
        let one = fixture_root();
        let two = fixture_root();
        fixture_app(&one, "Duplicate");
        fixture_app(&two, "Duplicate");
        let roots = [one.clone(), two.clone()];
        let ambiguous = query_in("Duplicate", AppFactsOptions::default(), &roots).unwrap_err();
        assert_eq!(ambiguous.kind(), AppFactsErrorKind::Ambiguous);
        let missing = query_in("Missing", AppFactsOptions::default(), &roots).unwrap_err();
        assert_eq!(missing.kind(), AppFactsErrorKind::NotFound);
        std::fs::remove_dir_all(one).ok();
        std::fs::remove_dir_all(two).ok();
    }

    #[test]
    fn directory_without_info_plist_is_not_an_application() {
        let root = fixture_root();
        let path = root.join("Broken.app");
        std::fs::create_dir_all(&path).unwrap();
        let result = query_in(
            path.to_str().unwrap(),
            AppFactsOptions::default(),
            std::slice::from_ref(&root),
        );
        assert_eq!(result.unwrap_err().kind(), AppFactsErrorKind::NotFound);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn running_fact_matches_the_exact_bundle_executable() {
        let root = fixture_root();
        let app = fixture_app(&root, "Running Fixture");
        let executable = app.join("Contents/MacOS/facts");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        let mut child = std::process::Command::new(&executable)
            .arg("30")
            .spawn()
            .unwrap();
        let result = query_in(
            app.to_str().unwrap(),
            AppFactsOptions::default(),
            std::slice::from_ref(&root),
        );
        child.kill().ok();
        child.wait().ok();
        let facts = result.unwrap();
        assert_eq!(facts.running.value, Some(true));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn signed_system_bundle_reports_native_signature() {
        let path = Path::new("/System/Applications/Calculator.app");
        if !path.exists() {
            return;
        }
        let facts = query_in(
            path.to_str().unwrap(),
            AppFactsOptions {
                signing: true,
                verify: true,
                entitlements: true,
            },
            &[],
        )
        .unwrap();
        assert_eq!(facts.bundle.value.as_deref(), Some("com.apple.calculator"));
        assert!(
            facts
                .signature
                .value
                .as_deref()
                .unwrap()
                .starts_with("identifier=com.apple.calculator;")
        );
        assert_eq!(facts.signature_verified.value, Some(true));
        assert!(facts.entitlements.value.is_some());

        let by_identifier =
            query_in("com.apple.calculator", AppFactsOptions::default(), &[]).unwrap();
        assert_eq!(by_identifier.path.value, facts.path.value);
    }

    #[test]
    fn signature_validation_distinguishes_invalid_from_unavailable() {
        for status in [-67_061, -67_057, -67_056, -67_055, -67_054] {
            assert_eq!(verification_failure(status).value, Some(false));
        }
        for status in [-67_060, -67_059, -67_048, -1] {
            let fact = verification_failure(status);
            assert_eq!(fact.status, crate::app_facts::FactStatus::Unavailable);
            assert_eq!(fact.value, None);
            assert!(fact.reason.unwrap().contains(&status.to_string()));
        }
    }
}
