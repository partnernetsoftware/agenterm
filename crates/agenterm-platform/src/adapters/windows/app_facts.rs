//! Windows application facts from bounded Uninstall registry discovery.
//!
//! This adapter reads PE version resources, process image paths, and embedded
//! Authenticode data through Win32 directly. It never invokes a shell, follows
//! shortcuts, inspects catalog signatures, or permits trust verification to
//! contact the network.

#![cfg(target_os = "windows")]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};

use crate::contract::app_facts::{
    AppFacts, AppFactsError, AppFactsErrorKind, AppFactsOptions, Fact,
};
use windows_sys::Win32::Foundation::{
    CERT_E_CHAINING, CERT_E_EXPIRED, CERT_E_UNTRUSTEDROOT, CloseHandle, ERROR_FILE_NOT_FOUND,
    ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES, ERROR_NO_MORE_ITEMS,
    ERROR_PATH_NOT_FOUND, ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_TYPE_NOT_FOUND,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, TRUST_E_BAD_DIGEST, TRUST_E_EXPLICIT_DISTRUST,
    TRUST_E_NOSIGNATURE,
};
use windows_sys::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_FIND_SUBJECT_CERT, CERT_HASH_PROP_ID, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY,
    CERT_QUERY_OBJECT_FILE, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, CertCloseStore,
    CertFindCertificateInStore, CertFreeCertificateContext, CertGetCertificateContextProperty,
    CertGetNameStringW, CryptMsgClose, CryptMsgGetParam, CryptQueryObject, HCERTSTORE,
    PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetBinaryTypeW, GetFileInformationByHandle, GetFileVersionInfoSizeW, GetFileVersionInfoW,
    VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

const UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const MAX_INSTALLED_APPS: usize = 4_096;
const MAX_PROCESS_ENTRIES: usize = 32_768;
const MAX_NATIVE_TEXT_UNITS: usize = 32 * 1024;
const MAX_VERSION_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_THUMBPRINT_BYTES: usize = 128;
const EMBEDDED_SIGNATURE_ABSENT: &str = "executable-has-no-embedded-authenticode-signature;embedded-only-catalog-signatures-are-not-inspected";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    volume_serial_number: u32,
    file_index: u64,
    length: u64,
    last_write_time: u64,
}

impl FileIdentity {
    fn read(path: &Path) -> Result<Self, AppFactsError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
            .map_err(|cause| {
                error(
                    AppFactsErrorKind::Io,
                    "app_facts_bundle_unreadable",
                    format!("could not open the Windows executable identity: {cause}"),
                )
            })?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file owns a live handle and the output record is writable.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &raw mut information) }
            == 0
        {
            return Err(error(
                AppFactsErrorKind::Io,
                "app_facts_bundle_unreadable",
                format!(
                    "could not read the Windows executable identity: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(Self {
            volume_serial_number: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
            length: (u64::from(information.nFileSizeHigh) << 32)
                | u64::from(information.nFileSizeLow),
            last_write_time: (u64::from(information.ftLastWriteTime.dwHighDateTime) << 32)
                | u64::from(information.ftLastWriteTime.dwLowDateTime),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationOrigin {
    root: HKEY,
    view: u32,
    key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Registration {
    origin: RegistrationOrigin,
    display_name: Option<String>,
    display_version: Option<String>,
    executable: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    identity: FileIdentity,
    registration: Option<Registration>,
}

#[derive(Debug)]
struct VersionFacts {
    product_name: Option<String>,
    version: Option<String>,
}

#[derive(Debug)]
struct SigningFacts {
    signature: Fact<String>,
    verified: Fact<bool>,
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a non-null Win32 handle.
        unsafe { CloseHandle(self.0) };
    }
}

struct OwnedRegistryKey(HKEY);

impl Drop for OwnedRegistryKey {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a key returned by RegOpenKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

struct OwnedCryptQuery {
    store: HCERTSTORE,
    message: *mut c_void,
}

impl Drop for OwnedCryptQuery {
    fn drop(&mut self) {
        // SAFETY: both values are successful CryptQueryObject outputs and are
        // independently released exactly once here.
        unsafe {
            if !self.message.is_null() {
                CryptMsgClose(self.message);
            }
            if !self.store.is_null() {
                CertCloseStore(self.store, 0);
            }
        }
    }
}

struct OwnedCertificate(*const CERT_CONTEXT);

impl Drop for OwnedCertificate {
    fn drop(&mut self) {
        // SAFETY: this context came from CertFindCertificateInStore and is
        // released exactly once.
        unsafe { CertFreeCertificateContext(self.0) };
    }
}

struct TrustCloseGuard {
    data: *mut WINTRUST_DATA,
}

impl Drop for TrustCloseGuard {
    fn drop(&mut self) {
        // SAFETY: `data` points to the still-live stack record used for VERIFY.
        // WinVerifyTrust requires a matching CLOSE even when VERIFY failed.
        unsafe {
            (*self.data).dwStateAction = WTD_STATEACTION_CLOSE;
            let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
            WinVerifyTrust(std::ptr::null_mut(), &raw mut action, self.data.cast());
        }
    }
}

fn error(kind: AppFactsErrorKind, code: &'static str, message: impl Into<String>) -> AppFactsError {
    AppFactsError::new(kind, code, message)
}

fn wide_nul(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn string_wide_nul(value: &str) -> Vec<u16> {
    wide_nul(OsStr::new(value))
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

fn path_text(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

fn path_key(path: &Path) -> String {
    path.as_os_str().to_string_lossy().to_uppercase()
}

fn text_key(value: &OsStr) -> String {
    value.to_string_lossy().to_uppercase()
}

fn canonical_executable(path: &Path) -> Result<(PathBuf, FileIdentity), AppFactsError> {
    let canonical = strip_verbatim_prefix(std::fs::canonicalize(path).map_err(|_| {
        error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to a Windows executable",
        )
    })?);
    let metadata = std::fs::metadata(&canonical).map_err(|_| {
        error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to a regular Windows executable",
        )
    })?;
    let is_exe = canonical
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    if !metadata.is_file() || !is_exe {
        return Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to a regular .exe file",
        ));
    }
    let mut binary_type = 0_u32;
    let wide = wide_nul(canonical.as_os_str());
    // SAFETY: the canonical path is NUL-terminated and the output pointer is writable.
    if unsafe { GetBinaryTypeW(wide.as_ptr(), &raw mut binary_type) } == 0 {
        return Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector did not resolve to a Windows executable image",
        ));
    }
    let identity = FileIdentity::read(&canonical)?;
    Ok((canonical, identity))
}

fn path_selector(selector: &str) -> bool {
    selector.contains(['/', '\\']) || selector.as_bytes().get(1) == Some(&b':')
}

fn open_registry_key(
    root: HKEY,
    path: &str,
    view: u32,
) -> Result<Option<OwnedRegistryKey>, AppFactsError> {
    let path = string_wide_nul(path);
    let mut key = std::ptr::null_mut();
    // SAFETY: the path is NUL-terminated and the output pointer is writable.
    let status = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &raw mut key) };
    if status == 0 {
        return Ok(Some(OwnedRegistryKey(key)));
    }
    if matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        return Ok(None);
    }
    Err(error(
        AppFactsErrorKind::Io,
        "app_facts_scan_failed",
        format!("could not open Windows Uninstall registry view: error {status}"),
    ))
}

fn registry_string(key: HKEY, name: &str) -> Result<Option<String>, AppFactsError> {
    let name = string_wide_nul(name);
    let mut bytes = 0_u32;
    // SAFETY: the key is live, names are terminated, and the size pointer is writable.
    let status = unsafe {
        RegGetValueW(
            key,
            std::ptr::null(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut bytes,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != 0 || bytes == 0 {
        return if status == 0 {
            Ok(None)
        } else {
            Err(error(
                AppFactsErrorKind::Io,
                "app_facts_scan_failed",
                format!("could not size an Uninstall registry value: error {status}"),
            ))
        };
    }
    let units = usize::try_from(bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(1))
        .map(|bytes| bytes / 2)
        .filter(|units| *units <= MAX_NATIVE_TEXT_UNITS)
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_scan_failed",
                "Uninstall registry value exceeded the native text bound",
            )
        })?;
    let mut value = vec![0_u16; units];
    let mut capacity = u32::try_from(value.len() * 2).expect("bounded registry value");
    // SAFETY: the buffer is writable for `capacity` bytes and the key remains live.
    let status = unsafe {
        RegGetValueW(
            key,
            std::ptr::null(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &raw mut capacity,
        )
    };
    if status == ERROR_MORE_DATA {
        return Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            "Uninstall registry value changed while it was read",
        ));
    }
    if status != 0 {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_scan_failed",
            format!("could not read an Uninstall registry value: error {status}"),
        ));
    }
    let used = usize::try_from(capacity).unwrap_or(0) / 2;
    let end = value[..used.min(value.len())]
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(used.min(value.len()));
    let decoded = OsString::from_wide(&value[..end])
        .to_string_lossy()
        .into_owned();
    Ok((!decoded.is_empty()).then_some(decoded))
}

fn display_icon_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    let without_index = value
        .rsplit_once(',')
        .filter(|(_, suffix)| suffix.trim().parse::<i32>().is_ok())
        .map_or(value, |(path, _)| path);
    let unquoted = without_index.trim().trim_matches('"').trim();
    Path::new(unquoted)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        .then(|| PathBuf::from(unquoted))
}

fn read_registration(
    root: HKEY,
    view: u32,
    key_name: &str,
    key: HKEY,
) -> Result<Registration, AppFactsError> {
    let executable = registry_string(key, "DisplayIcon")?
        .and_then(|value| display_icon_path(&value))
        .and_then(|path| canonical_executable(&path).ok().map(|(path, _)| path));
    Ok(Registration {
        origin: RegistrationOrigin {
            root,
            view,
            key: key_name.to_owned(),
        },
        display_name: registry_string(key, "DisplayName")?,
        display_version: registry_string(key, "DisplayVersion")?,
        executable,
    })
}

fn scan_registrations() -> Result<Vec<Registration>, AppFactsError> {
    let mut registrations = Vec::new();
    let mut visited = 0_usize;
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let Some(uninstall) = open_registry_key(root, UNINSTALL_KEY, view)? else {
                continue;
            };
            let mut index = 0_u32;
            loop {
                let mut name = vec![0_u16; 256];
                let mut length = u32::try_from(name.len()).expect("fixed registry key bound");
                // SAFETY: the parent key is live and the name buffer has the supplied capacity.
                let status = unsafe {
                    RegEnumKeyExW(
                        uninstall.0,
                        index,
                        name.as_mut_ptr(),
                        &raw mut length,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != 0 {
                    return Err(error(
                        AppFactsErrorKind::Io,
                        "app_facts_scan_failed",
                        format!("could not enumerate an Uninstall registry key: error {status}"),
                    ));
                }
                visited = visited.saturating_add(1);
                if visited > MAX_INSTALLED_APPS {
                    return Err(error(
                        AppFactsErrorKind::ScanTruncated,
                        "app_facts_scan_truncated",
                        format!("Windows application scan exceeded {MAX_INSTALLED_APPS} entries"),
                    ));
                }
                let key_name = OsString::from_wide(&name[..usize::try_from(length).unwrap_or(0)])
                    .to_string_lossy()
                    .into_owned();
                let child_path = format!("{UNINSTALL_KEY}\\{key_name}");
                if let Some(child) = open_registry_key(root, &child_path, view)? {
                    registrations.push(read_registration(root, view, &key_name, child.0)?);
                }
                index = index.checked_add(1).ok_or_else(|| {
                    error(
                        AppFactsErrorKind::ScanTruncated,
                        "app_facts_scan_truncated",
                        "Windows application registry index overflowed",
                    )
                })?;
            }
        }
    }
    Ok(registrations)
}

fn resolve_registration(
    selector: &str,
    registrations: Vec<Registration>,
) -> Result<Candidate, AppFactsError> {
    let by_id: Vec<_> = registrations
        .iter()
        .filter(|registration| registration.origin.key == selector)
        .cloned()
        .collect();
    let mut matches: Vec<_> = if by_id.is_empty() {
        registrations
            .into_iter()
            .filter(|registration| registration.display_name.as_deref() == Some(selector))
            .collect()
    } else {
        by_id
    };
    matches.sort_by_key(|registration| {
        let root_rank = if registration.origin.root == HKEY_CURRENT_USER {
            0_u8
        } else {
            1_u8
        };
        let view_rank = if registration.origin.view == KEY_WOW64_64KEY {
            0_u8
        } else {
            1_u8
        };
        (
            registration
                .executable
                .as_deref()
                .map(path_key)
                .unwrap_or_default(),
            root_rank,
            view_rank,
            registration.origin.key.clone(),
        )
    });
    let mut canonical = BTreeMap::<String, (PathBuf, Registration)>::new();
    for registration in matches {
        let Some(path) = registration.executable.clone() else {
            continue;
        };
        canonical
            .entry(path_key(&path))
            .or_insert((path, registration));
    }
    match canonical.len() {
        0 => Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector matched no Uninstall key id or unique DisplayName with a readable executable",
        )),
        1 => {
            let (_, (path, registration)) = canonical.into_iter().next().expect("one candidate");
            let (_, identity) = canonical_executable(&path)?;
            Ok(Candidate {
                path,
                identity,
                registration: Some(registration),
            })
        }
        count => Err(error(
            AppFactsErrorKind::Ambiguous,
            "app_facts_ambiguous",
            format!("application selector matched {count} distinct Windows executables"),
        )),
    }
}

fn resolve(selector: &str) -> Result<Candidate, AppFactsError> {
    if path_selector(selector) {
        let (path, identity) = canonical_executable(Path::new(selector))?;
        Ok(Candidate {
            path,
            identity,
            registration: None,
        })
    } else {
        resolve_registration(selector, scan_registrations()?)
    }
}

fn pointer_inside(buffer: &[u8], pointer: *const c_void, bytes: usize) -> bool {
    let start = buffer.as_ptr() as usize;
    let end = start.saturating_add(buffer.len());
    let pointer = pointer as usize;
    pointer >= start && pointer.checked_add(bytes).is_some_and(|last| last <= end)
}

fn version_resource(path: &Path) -> Result<VersionFacts, AppFactsError> {
    let path = wide_nul(path.as_os_str());
    // SAFETY: the file path is NUL-terminated; no legacy handle is requested.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        let cause = std::io::Error::last_os_error();
        if matches!(
            cause.raw_os_error().map(|value| value as u32),
            Some(ERROR_RESOURCE_DATA_NOT_FOUND | ERROR_RESOURCE_TYPE_NOT_FOUND)
        ) {
            return Ok(VersionFacts {
                product_name: None,
                version: None,
            });
        }
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_bundle_unreadable",
            format!("could not size the executable version resource: {cause}"),
        ));
    }
    let size = usize::try_from(size)
        .ok()
        .filter(|size| *size <= MAX_VERSION_RESOURCE_BYTES)
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_bundle_unreadable",
                "executable version resource exceeded the native byte bound",
            )
        })?;
    let mut bytes = vec![0_u8; size];
    // SAFETY: the resource buffer is writable for the exact size returned above.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size as u32, bytes.as_mut_ptr().cast()) } == 0
    {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_bundle_unreadable",
            format!(
                "could not read the executable version resource: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let root = string_wide_nul("\\");
    let mut fixed_pointer = std::ptr::null_mut();
    let mut fixed_length = 0_u32;
    // SAFETY: query output pointers are writable and borrow from `bytes`.
    let has_fixed = unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            root.as_ptr(),
            &raw mut fixed_pointer,
            &raw mut fixed_length,
        )
    } != 0;
    let version = if has_fixed
        && usize::try_from(fixed_length).unwrap_or(0) >= std::mem::size_of::<VS_FIXEDFILEINFO>()
        && pointer_inside(
            &bytes,
            fixed_pointer,
            std::mem::size_of::<VS_FIXEDFILEINFO>(),
        ) {
        // SAFETY: containment and minimum byte length were checked; Windows may
        // return a pointer whose alignment is not expressible by the byte Vec.
        let fixed = unsafe { fixed_pointer.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
        (fixed.dwSignature == 0xfeef_04bd).then(|| {
            format!(
                "{}.{}.{}.{}",
                fixed.dwFileVersionMS >> 16,
                fixed.dwFileVersionMS & 0xffff,
                fixed.dwFileVersionLS >> 16,
                fixed.dwFileVersionLS & 0xffff
            )
        })
    } else {
        None
    };

    let translation_name = string_wide_nul("\\VarFileInfo\\Translation");
    let mut translation = std::ptr::null_mut();
    let mut translation_length = 0_u32;
    // SAFETY: query output pointers are writable and borrow from `bytes`.
    let has_translation = unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            translation_name.as_ptr(),
            &raw mut translation,
            &raw mut translation_length,
        )
    } != 0;
    let product_name =
        if has_translation && translation_length >= 4 && pointer_inside(&bytes, translation, 4) {
            // SAFETY: four contained bytes were proved; unaligned access is deliberate.
            let pair = unsafe { translation.cast::<u32>().read_unaligned() };
            let language = pair & 0xffff;
            let codepage = pair >> 16;
            let query = string_wide_nul(&format!(
                "\\StringFileInfo\\{language:04x}{codepage:04x}\\ProductName"
            ));
            let mut value = std::ptr::null_mut();
            let mut units = 0_u32;
            // SAFETY: query output pointers are writable and borrow from `bytes`.
            let found = unsafe {
                VerQueryValueW(
                    bytes.as_ptr().cast(),
                    query.as_ptr(),
                    &raw mut value,
                    &raw mut units,
                )
            } != 0;
            let units = usize::try_from(units).unwrap_or(0);
            if found
                && units > 0
                && units <= MAX_NATIVE_TEXT_UNITS
                && pointer_inside(&bytes, value, units.saturating_mul(2))
            {
                // SAFETY: the UTF-16 range is wholly contained in `bytes`.
                let value = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), units) };
                let end = value
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(value.len());
                let text = OsString::from_wide(&value[..end])
                    .to_string_lossy()
                    .into_owned();
                (!text.trim().is_empty()).then_some(text)
            } else {
                None
            }
        } else {
            None
        };
    Ok(VersionFacts {
        product_name,
        version,
    })
}

fn process_ids_named(executable_name: &OsStr) -> Result<Option<Vec<u32>>, AppFactsError> {
    // ToolHelp exposes the executable basename without requiring a process
    // handle. Filter on that public inventory first so unrelated protected
    // processes cannot turn an exact negative result into `unavailable`.
    // SAFETY: no borrowed pointer crosses the snapshot call.
    let Some(snapshot) =
        OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
    else {
        return Ok(None);
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: the snapshot is live and the initialized record has the
    // documented size for this target.
    let mut present = unsafe { Process32FirstW(snapshot.0, &raw mut entry) } != 0;
    if !present {
        // SAFETY: GetLastError is read immediately after the failed iterator call.
        return Ok((unsafe { GetLastError() } == ERROR_NO_MORE_FILES).then(Vec::new));
    }
    let expected_name = text_key(executable_name);
    let mut visited = 0_usize;
    let mut matches = Vec::new();
    loop {
        visited = visited.saturating_add(1);
        if visited > MAX_PROCESS_ENTRIES {
            return Err(error(
                AppFactsErrorKind::ScanTruncated,
                "app_facts_scan_truncated",
                format!("Windows process scan exceeded {MAX_PROCESS_ENTRIES} entries"),
            ));
        }
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        if text_key(&OsString::from_wide(&entry.szExeFile[..end])) == expected_name {
            matches.push(entry.th32ProcessID);
        }
        // SAFETY: the snapshot and output record remain live for the iterator call.
        present = unsafe { Process32NextW(snapshot.0, &raw mut entry) } != 0;
        if !present {
            // SAFETY: GetLastError is read immediately after the failed iterator call.
            return Ok((unsafe { GetLastError() } == ERROR_NO_MORE_FILES).then_some(matches));
        }
    }
}

fn running_fact(executable: &Path) -> Result<Fact<bool>, AppFactsError> {
    let expected = path_key(executable);
    let Some(executable_name) = executable.file_name() else {
        return Ok(Fact::unavailable("windows-process-identity-unavailable"));
    };
    let Some(pids) = process_ids_named(executable_name)? else {
        return Ok(Fact::unavailable("windows-process-inventory-unavailable"));
    };
    let mut unreadable = 0_usize;
    for pid in pids.into_iter().filter(|pid| *pid != 0) {
        // SAFETY: OpenProcess copies the numeric PID and returns an owned handle.
        let Some(process) =
            OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
        else {
            let cause = std::io::Error::last_os_error();
            if cause.raw_os_error().map(|value| value as u32) != Some(ERROR_INVALID_PARAMETER) {
                unreadable = unreadable.saturating_add(1);
            }
            continue;
        };
        let mut path = vec![0_u16; MAX_NATIVE_TEXT_UNITS];
        let mut length = u32::try_from(path.len()).expect("bounded process path");
        // SAFETY: the process handle is live and the output buffer has `length` units.
        if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &raw mut length) }
            == 0
        {
            let cause = std::io::Error::last_os_error();
            if cause.raw_os_error().map(|value| value as u32) != Some(ERROR_INVALID_PARAMETER) {
                unreadable = unreadable.saturating_add(1);
            }
            continue;
        }
        let process_path = PathBuf::from(OsString::from_wide(
            &path[..usize::try_from(length).unwrap_or(0).min(path.len())],
        ));
        match std::fs::canonicalize(process_path).map(strip_verbatim_prefix) {
            Ok(path) if path_key(&path) == expected => return Ok(Fact::present(true)),
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => unreadable = unreadable.saturating_add(1),
        }
    }
    Ok(if unreadable == 0 {
        Fact::present(false)
    } else {
        Fact::unavailable("windows-process-inventory-incomplete")
    })
}

fn embedded_signature(path: &Path) -> Result<Option<String>, AppFactsError> {
    let path = wide_nul(path.as_os_str());
    let mut store = std::ptr::null_mut();
    let mut message = std::ptr::null_mut();
    // SAFETY: the path is NUL-terminated and both ownership outputs are writable.
    if unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            path.as_ptr().cast(),
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut store,
            &raw mut message,
            std::ptr::null_mut(),
        )
    } == 0
    {
        let cause = std::io::Error::last_os_error();
        if cause.raw_os_error().map(|value| value as u32) == Some(0x8009_2009) {
            return Ok(None);
        }
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!("CryptQueryObject failed: {cause}"),
        ));
    }
    let query = OwnedCryptQuery { store, message };
    let mut signer_bytes = 0_u32;
    // SAFETY: the cryptographic message remains owned by `query`.
    if unsafe {
        CryptMsgGetParam(
            query.message,
            CMSG_SIGNER_INFO_PARAM,
            0,
            std::ptr::null_mut(),
            &raw mut signer_bytes,
        )
    } == 0
    {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!(
                "CryptMsgGetParam could not size signer data: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let signer_size = usize::try_from(signer_bytes)
        .ok()
        .filter(|size| {
            *size >= std::mem::size_of::<CMSG_SIGNER_INFO>() && *size <= MAX_VERSION_RESOURCE_BYTES
        })
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_signature_query_failed",
                "embedded signer data had an invalid bounded size",
            )
        })?;
    let mut signer_storage = vec![0_u8; signer_size];
    // SAFETY: storage is writable for the exact queried signer byte count.
    if unsafe {
        CryptMsgGetParam(
            query.message,
            CMSG_SIGNER_INFO_PARAM,
            0,
            signer_storage.as_mut_ptr().cast(),
            &raw mut signer_bytes,
        )
    } == 0
    {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!(
                "CryptMsgGetParam could not read signer data: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    // SAFETY: the bounded storage contains at least one complete signer record;
    // unaligned access avoids depending on Vec<u8>'s alignment.
    let signer = unsafe {
        signer_storage
            .as_ptr()
            .cast::<CMSG_SIGNER_INFO>()
            .read_unaligned()
    };
    let certificate_info = windows_sys::Win32::Security::Cryptography::CERT_INFO {
        Issuer: signer.Issuer,
        SerialNumber: signer.SerialNumber,
        ..windows_sys::Win32::Security::Cryptography::CERT_INFO::default()
    };
    // SAFETY: store is live, and certificate_info borrows issuer/serial blobs
    // from signer_storage, which remains live through this lookup.
    let certificate = unsafe {
        CertFindCertificateInStore(
            query.store,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_CERT,
            (&raw const certificate_info).cast(),
            std::ptr::null(),
        )
    };
    let certificate = (!certificate.is_null())
        .then_some(OwnedCertificate(certificate))
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_signature_query_failed",
                format!(
                    "signer certificate lookup failed: {}",
                    std::io::Error::last_os_error()
                ),
            )
        })?;
    // SAFETY: certificate is retained; a null output requests the unit count.
    let subject_units = unsafe {
        CertGetNameStringW(
            certificate.0,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };
    let subject_units = usize::try_from(subject_units)
        .ok()
        .filter(|units| *units > 1 && *units <= MAX_NATIVE_TEXT_UNITS)
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_signature_query_failed",
                "signer subject was absent or exceeded the native text bound",
            )
        })?;
    let mut subject = vec![0_u16; subject_units];
    // SAFETY: certificate is retained and the output buffer has the supplied capacity.
    let written = unsafe {
        CertGetNameStringW(
            certificate.0,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            subject.as_mut_ptr(),
            subject_units as u32,
        )
    };
    if written == 0 || usize::try_from(written).unwrap_or(0) > subject.len() {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            "signer subject could not be decoded",
        ));
    }
    let end = subject
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(subject.len());
    let subject = OsString::from_wide(&subject[..end])
        .to_string_lossy()
        .into_owned();

    let mut thumbprint_bytes = 0_u32;
    // SAFETY: certificate is retained and the size pointer is writable.
    if unsafe {
        CertGetCertificateContextProperty(
            certificate.0,
            CERT_HASH_PROP_ID,
            std::ptr::null_mut(),
            &raw mut thumbprint_bytes,
        )
    } == 0
    {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!(
                "could not size signer thumbprint: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let thumbprint_size = usize::try_from(thumbprint_bytes)
        .ok()
        .filter(|size| *size > 0 && *size <= MAX_THUMBPRINT_BYTES)
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_signature_query_failed",
                "signer thumbprint had an invalid bounded size",
            )
        })?;
    let mut thumbprint = vec![0_u8; thumbprint_size];
    // SAFETY: certificate is retained and the output buffer has the queried size.
    if unsafe {
        CertGetCertificateContextProperty(
            certificate.0,
            CERT_HASH_PROP_ID,
            thumbprint.as_mut_ptr().cast(),
            &raw mut thumbprint_bytes,
        )
    } == 0
    {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_signature_query_failed",
            format!(
                "could not read signer thumbprint: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let thumbprint = thumbprint
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(Some(format!("subject={subject};thumbprint={thumbprint}")))
}

fn verify_signature(path: &Path) -> Fact<bool> {
    let path = wide_nul(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: windows_sys::Win32::Security::WinTrust::WINTRUST_DATA_0 {
            pFile: &raw mut file,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..WINTRUST_DATA::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: all pointers in the official WinTrust layouts remain live for
    // VERIFY and for the mandatory guard-driven CLOSE below.
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &raw mut action,
            (&raw mut data).cast(),
        )
    };
    let close = TrustCloseGuard {
        data: &raw mut data,
    };
    let result = match status {
        0 => Fact::present(true),
        TRUST_E_NOSIGNATURE => Fact::absent(EMBEDDED_SIGNATURE_ABSENT),
        CERT_E_UNTRUSTEDROOT
        | TRUST_E_BAD_DIGEST
        | CERT_E_EXPIRED
        | TRUST_E_EXPLICIT_DISTRUST
        | CERT_E_CHAINING => Fact::present(false),
        _ => Fact::unavailable(format!(
            "signature-validation-failed-with-HRESULT-0x{:08x}",
            status as u32
        )),
    };
    drop(close);
    result
}

fn signing_facts(path: &Path, options: AppFactsOptions) -> Result<SigningFacts, AppFactsError> {
    let signature_requested = options.signing || options.verify;
    let embedded = if signature_requested {
        embedded_signature(path)?
    } else {
        None
    };
    let signature = if signature_requested {
        embedded
            .clone()
            .map_or_else(|| Fact::absent(EMBEDDED_SIGNATURE_ABSENT), Fact::present)
    } else {
        Fact::not_requested("signing-was-not-requested")
    };
    let verified = if options.verify {
        if embedded.is_some() {
            verify_signature(path)
        } else {
            // This cut deliberately does not inspect catalog signatures. Do
            // not let WinVerifyTrust's catalog fallback publish `true` while
            // the independently read embedded-signature fact is absent.
            Fact::absent(EMBEDDED_SIGNATURE_ABSENT)
        }
    } else {
        Fact::not_requested("signature-verification-was-not-requested")
    };
    Ok(SigningFacts {
        signature,
        verified,
    })
}

fn revalidate(candidate: &Candidate) -> Result<(), AppFactsError> {
    let current = FileIdentity::read(&candidate.path);
    if !matches!(current, Ok(identity) if identity == candidate.identity) {
        return Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            "Windows executable identity changed during application-facts observation",
        ));
    }
    if let Some(registration) = &candidate.registration {
        let path = format!("{UNINSTALL_KEY}\\{}", registration.origin.key);
        let Some(key) =
            open_registry_key(registration.origin.root, &path, registration.origin.view)?
        else {
            return Err(error(
                AppFactsErrorKind::IdentityDrift,
                "app_facts_identity_drift",
                "Windows Uninstall registration disappeared during application-facts observation",
            ));
        };
        let current = read_registration(
            registration.origin.root,
            registration.origin.view,
            &registration.origin.key,
            key.0,
        )?;
        if &current != registration {
            return Err(error(
                AppFactsErrorKind::IdentityDrift,
                "app_facts_identity_drift",
                "Windows Uninstall registration changed during application-facts observation",
            ));
        }
    }
    Ok(())
}

pub(crate) fn query(selector: &str, options: AppFactsOptions) -> Result<AppFacts, AppFactsError> {
    let candidate = resolve(selector)?;
    let version_resource = version_resource(&candidate.path)?;
    let name = version_resource
        .product_name
        .or_else(|| candidate.registration.as_ref()?.display_name.clone())
        .or_else(|| {
            candidate
                .path
                .file_stem()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_non_utf8_path",
                "Windows executable name could not be decoded",
            )
        })?;
    let version = version_resource.version.map_or_else(
        || {
            candidate
                .registration
                .as_ref()
                .and_then(|registration| registration.display_version.clone())
                .map_or_else(
                    || Fact::absent("executable-declares-no-version-resource"),
                    Fact::present,
                )
        },
        Fact::present,
    );
    let running = running_fact(&candidate.path)?;
    let signing = signing_facts(&candidate.path, options)?;
    revalidate(&candidate)?;
    let path = path_text(&candidate.path);
    Ok(AppFacts {
        schema_version: 1,
        platform: "windows".to_owned(),
        selector: selector.to_owned(),
        desktop_entry_id: Fact::not_applicable("windows-applications-have-no-xdg-desktop-entry"),
        name: Fact::present(name),
        bundle: candidate.registration.as_ref().map_or_else(
            || Fact::absent("no-uninstall-registration-lookup-by-path"),
            |registration| Fact::present(registration.origin.key.clone()),
        ),
        path: Fact::present(path.clone()),
        version,
        executable: Fact::present(path),
        running,
        signature: signing.signature,
        signature_verified: signing.verified,
        entitlements: if options.entitlements {
            Fact::not_applicable("windows-executables-have-no-apple-entitlements")
        } else {
            Fact::not_requested("entitlements-were-not-requested")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_path_classification_is_windows_specific() {
        assert!(path_selector(r"C:\\apps\\thing.exe"));
        assert!(path_selector(r"relative\\thing.exe"));
        assert!(path_selector("relative/thing.exe"));
        assert!(!path_selector("thing.exe"));
        assert!(!path_selector("AgenTerm"));
    }

    #[test]
    fn display_icon_accepts_only_executables_and_strips_numeric_index() {
        assert_eq!(
            display_icon_path(r#" "C:\Program Files\Facts\facts.exe",-2 "#),
            Some(PathBuf::from(r"C:\Program Files\Facts\facts.exe"))
        );
        assert_eq!(display_icon_path(r"C:\Facts\facts.dll,0"), None);
        assert_eq!(display_icon_path(r"C:\Facts\facts.exe,icon"), None);
        assert_eq!(
            display_icon_path(r#""C:\Program, Files\Facts\facts.exe",7"#),
            Some(PathBuf::from(r"C:\Program, Files\Facts\facts.exe"))
        );
    }

    #[test]
    fn canonical_dedup_precedes_ambiguity() {
        let path = PathBuf::from(r"C:\Facts\facts.exe");
        let registration = |key: &str| Registration {
            origin: RegistrationOrigin {
                root: HKEY_CURRENT_USER,
                view: KEY_WOW64_64KEY,
                key: key.to_owned(),
            },
            display_name: Some("Facts".to_owned()),
            display_version: None,
            executable: Some(path.clone()),
        };
        let matches = [registration("one"), registration("two")];
        let unique = matches
            .iter()
            .filter_map(|item| item.executable.as_ref())
            .map(|path| path_key(path))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 1);
    }
}
