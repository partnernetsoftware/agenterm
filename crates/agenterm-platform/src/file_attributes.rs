//! Identity-bound file mode and extended-attribute mechanisms.
//!
//! Product policy and path selection stay with the caller. Every operation in
//! this module acts on an already-open regular file, revalidates its stable
//! identity, and refuses stale mutation plans.

use std::{fmt, fs::File, io, path::Path};

use sha2::{Digest as _, Sha256};

use crate::file_identity::{FileIdentity, file_identity};

#[cfg(target_os = "linux")]
#[path = "adapters/linux/file_attributes.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/file_attributes.rs"]
mod native;
#[cfg(windows)]
#[path = "adapters/windows/file_attributes.rs"]
mod native;

const MAX_XATTR_NAME_BYTES: usize = 255;
const MAX_XATTR_ATTRIBUTES: usize = 4_096;
const MAX_XATTR_LIST_BYTES: usize = 1024 * 1024;
const MAX_XATTR_MUTATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_XATTR_INSPECT_VALUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_XATTR_INSPECT_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_MODE: u32 = 0o7777;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XattrInspectLimits {
    pub max_attributes: usize,
    pub max_name_bytes: usize,
    pub max_value_bytes: usize,
    pub max_total_value_bytes: usize,
    /// Include raw values in the result. Digests and lengths are always returned.
    pub include_values: bool,
}

impl Default for XattrInspectLimits {
    fn default() -> Self {
        Self {
            max_attributes: 128,
            max_name_bytes: 32 * 1024,
            max_value_bytes: 1024 * 1024,
            max_total_value_bytes: 4 * 1024 * 1024,
            include_values: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XattrObservation {
    pub name: String,
    pub namespace: String,
    pub value_bytes: usize,
    pub value_sha256: String,
    pub value: Option<Vec<u8>>,
}

/// Proof that a path was a regular non-symlink entry naming the opened object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegularFileBinding {
    identity: FileIdentity,
}

impl RegularFileBinding {
    #[must_use]
    pub const fn identity(self) -> FileIdentity {
        self.identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModePlan {
    pub identity: FileIdentity,
    pub before_mode: u32,
    pub requested_mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModeMutationResult {
    pub identity: FileIdentity,
    pub before_mode: u32,
    pub after_mode: u32,
    pub rollback: ModePlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XattrAction {
    Set(Vec<u8>),
    Remove,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XattrState {
    pub value: Vec<u8>,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XattrPlan {
    pub identity: FileIdentity,
    pub name: String,
    pub expected_before: Option<XattrState>,
    pub action: XattrAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XattrMutationResult {
    pub identity: FileIdentity,
    pub name: String,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
    pub rollback: XattrPlan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FileAttributeErrorKind {
    Unsupported,
    NotRegularFile,
    IdentityChanged,
    InvalidName,
    InvalidMode,
    BudgetExceeded,
    PreconditionChanged,
    ReadbackMismatch,
    Native,
}

#[derive(Debug)]
pub struct FileAttributeError {
    pub kind: FileAttributeErrorKind,
    pub operation: &'static str,
    pub message: String,
    pub source: Option<io::Error>,
}

impl FileAttributeError {
    fn new(
        kind: FileAttributeErrorKind,
        operation: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn native(operation: &'static str, source: io::Error) -> Self {
        Self {
            kind: FileAttributeErrorKind::Native,
            operation,
            message: source.to_string(),
            source: Some(source),
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub(crate) fn unsupported(operation: &'static str) -> Self {
        Self::new(
            FileAttributeErrorKind::Unsupported,
            operation,
            "Unix file modes and extended attributes are unsupported on this host",
        )
    }
}

impl fmt::Display for FileAttributeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.message)
    }
}

impl std::error::Error for FileAttributeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// Bind an opened file to a caller path without following a final symlink.
///
/// Later operations use the handle and revalidate the returned identity, so a
/// path replacement after this call cannot redirect mutation.
pub fn bind_regular_file(
    path: &Path,
    file: &File,
) -> Result<RegularFileBinding, FileAttributeError> {
    let entry = std::fs::symlink_metadata(path)
        .map_err(|error| FileAttributeError::native("file-attribute-bind", error))?;
    if entry.file_type().is_symlink() || !entry.file_type().is_file() {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::NotRegularFile,
            "file-attribute-bind",
            "file attributes refuse symbolic links and non-regular entries",
        ));
    }
    let opened_identity = file_identity(file)
        .map_err(|error| FileAttributeError::native("file-attribute-bind", error))?;
    let path_identity = native::path_entry_identity(path)
        .map_err(|error| FileAttributeError::native("file-attribute-bind", error))?;
    if !opened_identity.same_object(path_identity) {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::IdentityChanged,
            "file-attribute-bind",
            "caller path and opened file refer to different objects",
        ));
    }
    Ok(RegularFileBinding {
        identity: opened_identity,
    })
}

pub fn inspect_xattrs(
    file: &File,
    binding: RegularFileBinding,
    limits: XattrInspectLimits,
) -> Result<Vec<XattrObservation>, FileAttributeError> {
    validate_regular_identity(file, binding.identity, "file-xattr-inspect")?;
    validate_limits(limits)?;
    let names = native::list_xattrs(file, limits.max_name_bytes)?;
    if names.len() > limits.max_attributes {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::BudgetExceeded,
            "file-xattr-inspect",
            "extended-attribute count exceeds the configured limit",
        ));
    }
    let mut total_value_bytes = 0usize;
    let mut observations = Vec::with_capacity(names.len());
    for name in names {
        validate_name(&name)?;
        let value = native::get_xattr(file, &name, limits.max_value_bytes)?.ok_or_else(|| {
            FileAttributeError::new(
                FileAttributeErrorKind::PreconditionChanged,
                "file-xattr-inspect",
                "extended attribute disappeared during inspection",
            )
        })?;
        total_value_bytes = total_value_bytes.checked_add(value.len()).ok_or_else(|| {
            FileAttributeError::new(
                FileAttributeErrorKind::BudgetExceeded,
                "file-xattr-inspect",
                "extended-attribute byte count overflow",
            )
        })?;
        if total_value_bytes > limits.max_total_value_bytes {
            return Err(FileAttributeError::new(
                FileAttributeErrorKind::BudgetExceeded,
                "file-xattr-inspect",
                "extended-attribute values exceed the aggregate limit",
            ));
        }
        observations.push(XattrObservation {
            namespace: namespace(&name),
            name,
            value_bytes: value.len(),
            value_sha256: sha256_hex(&value),
            value: limits.include_values.then_some(value),
        });
    }
    observations.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(observations)
}

pub fn plan_mode(
    file: &File,
    binding: RegularFileBinding,
    requested_mode: u32,
) -> Result<ModePlan, FileAttributeError> {
    validate_mode(requested_mode)?;
    validate_regular_identity(file, binding.identity, "file-mode-plan")?;
    Ok(ModePlan {
        identity: binding.identity,
        before_mode: native::mode(file)?,
        requested_mode,
    })
}

pub fn apply_mode(file: &File, plan: &ModePlan) -> Result<ModeMutationResult, FileAttributeError> {
    validate_mode(plan.requested_mode)?;
    validate_regular_identity(file, plan.identity, "file-mode-apply")?;
    let before = native::mode(file)?;
    if before != plan.before_mode {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::PreconditionChanged,
            "file-mode-apply",
            "file mode changed after planning",
        ));
    }
    native::set_mode(file, plan.requested_mode)?;
    let after = native::mode(file)?;
    if after != plan.requested_mode {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::ReadbackMismatch,
            "file-mode-apply",
            "file mode did not match the requested mode after mutation",
        ));
    }
    Ok(ModeMutationResult {
        identity: plan.identity,
        before_mode: before,
        after_mode: after,
        rollback: ModePlan {
            identity: plan.identity,
            before_mode: after,
            requested_mode: before,
        },
    })
}

pub fn plan_xattr_set(
    file: &File,
    binding: RegularFileBinding,
    name: &str,
    value: Vec<u8>,
    max_existing_value_bytes: usize,
) -> Result<XattrPlan, FileAttributeError> {
    plan_xattr(
        file,
        binding,
        name,
        XattrAction::Set(value),
        max_existing_value_bytes,
    )
}

pub fn plan_xattr_remove(
    file: &File,
    binding: RegularFileBinding,
    name: &str,
    max_existing_value_bytes: usize,
) -> Result<XattrPlan, FileAttributeError> {
    plan_xattr(
        file,
        binding,
        name,
        XattrAction::Remove,
        max_existing_value_bytes,
    )
}

pub fn plan_clear_quarantine(
    file: &File,
    binding: RegularFileBinding,
    max_existing_value_bytes: usize,
) -> Result<XattrPlan, FileAttributeError> {
    native::quarantine_plan_supported()?;
    plan_xattr_remove(
        file,
        binding,
        "com.apple.quarantine",
        max_existing_value_bytes,
    )
}

pub fn apply_xattr(
    file: &File,
    plan: &XattrPlan,
) -> Result<XattrMutationResult, FileAttributeError> {
    validate_name(&plan.name)?;
    validate_regular_identity(file, plan.identity, "file-xattr-apply")?;
    let max_before = plan
        .expected_before
        .as_ref()
        .map_or(0, |state| state.value.len());
    let before = read_precondition_value(file, &plan.name, max_before, "file-xattr-apply")?;
    if !state_matches(before.as_deref(), plan.expected_before.as_ref()) {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::PreconditionChanged,
            "file-xattr-apply",
            "extended attribute changed after planning",
        ));
    }
    match &plan.action {
        XattrAction::Set(value) => native::set_xattr(file, &plan.name, value)?,
        XattrAction::Remove => {
            if before.is_some() {
                native::remove_xattr(file, &plan.name)?;
            }
        }
    }
    let expected_after = match &plan.action {
        XattrAction::Set(value) => Some(value.as_slice()),
        XattrAction::Remove => None,
    };
    let after_limit = expected_after.map_or(0, <[u8]>::len);
    let after = native::get_xattr(file, &plan.name, after_limit).map_err(|error| {
        if error.kind == FileAttributeErrorKind::BudgetExceeded {
            FileAttributeError::new(
                FileAttributeErrorKind::ReadbackMismatch,
                "file-xattr-readback",
                "extended-attribute length differs from the state just applied",
            )
        } else {
            error
        }
    })?;
    if after.as_deref() != expected_after {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::ReadbackMismatch,
            "file-xattr-apply",
            "extended attribute did not match the planned state after mutation",
        ));
    }
    let rollback = XattrPlan {
        identity: plan.identity,
        name: plan.name.clone(),
        expected_before: after.as_ref().map(|value| state(value.clone())),
        action: match before.as_ref() {
            Some(value) => XattrAction::Set(value.clone()),
            None => XattrAction::Remove,
        },
    };
    Ok(XattrMutationResult {
        identity: plan.identity,
        name: plan.name.clone(),
        before_sha256: before.as_deref().map(sha256_hex),
        after_sha256: after.as_deref().map(sha256_hex),
        rollback,
    })
}

fn plan_xattr(
    file: &File,
    binding: RegularFileBinding,
    name: &str,
    action: XattrAction,
    max_existing_value_bytes: usize,
) -> Result<XattrPlan, FileAttributeError> {
    validate_name(name)?;
    if max_existing_value_bytes > MAX_XATTR_MUTATION_BYTES {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::BudgetExceeded,
            "file-xattr-plan",
            "existing extended-attribute read limit exceeds the fixed ceiling",
        ));
    }
    if matches!(&action, XattrAction::Set(value) if value.len() > MAX_XATTR_MUTATION_BYTES) {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::BudgetExceeded,
            "file-xattr-plan",
            "extended-attribute mutation exceeds the fixed value limit",
        ));
    }
    validate_regular_identity(file, binding.identity, "file-xattr-plan")?;
    let before = native::get_xattr(file, name, max_existing_value_bytes)?;
    Ok(XattrPlan {
        identity: binding.identity,
        name: name.to_owned(),
        expected_before: before.map(state),
        action,
    })
}

fn read_precondition_value(
    file: &File,
    name: &str,
    max_bytes: usize,
    operation: &'static str,
) -> Result<Option<Vec<u8>>, FileAttributeError> {
    native::get_xattr(file, name, max_bytes).map_err(|error| {
        if error.kind == FileAttributeErrorKind::BudgetExceeded {
            FileAttributeError::new(
                FileAttributeErrorKind::PreconditionChanged,
                operation,
                "extended-attribute existence or length changed after planning",
            )
        } else {
            error
        }
    })
}

fn validate_regular_identity(
    file: &File,
    expected: FileIdentity,
    operation: &'static str,
) -> Result<(), FileAttributeError> {
    let metadata = file
        .metadata()
        .map_err(|error| FileAttributeError::native(operation, error))?;
    if !metadata.file_type().is_file() {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::NotRegularFile,
            operation,
            "file attributes require an opened regular file",
        ));
    }
    let actual =
        file_identity(file).map_err(|error| FileAttributeError::native(operation, error))?;
    if !actual.same_object(expected) {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::IdentityChanged,
            operation,
            "opened file identity does not match the caller-bound identity",
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), FileAttributeError> {
    if name.is_empty() || name.len() > MAX_XATTR_NAME_BYTES || name.as_bytes().contains(&0) {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::InvalidName,
            "file-xattr-validate",
            "extended-attribute name is empty, too long, or contains NUL",
        ));
    }
    Ok(())
}

fn validate_mode(mode: u32) -> Result<(), FileAttributeError> {
    if mode > MAX_MODE {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::InvalidMode,
            "file-mode-validate",
            "mode contains bits outside the supported Unix permission mask",
        ));
    }
    Ok(())
}

fn validate_limits(limits: XattrInspectLimits) -> Result<(), FileAttributeError> {
    if limits.max_attributes == 0
        || limits.max_name_bytes == 0
        || limits.max_value_bytes == 0
        || limits.max_total_value_bytes == 0
    {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::BudgetExceeded,
            "file-xattr-inspect",
            "extended-attribute inspection limits must be nonzero",
        ));
    }
    if limits.max_attributes > MAX_XATTR_ATTRIBUTES
        || limits.max_name_bytes > MAX_XATTR_LIST_BYTES
        || limits.max_value_bytes > MAX_XATTR_INSPECT_VALUE_BYTES
        || limits.max_total_value_bytes > MAX_XATTR_INSPECT_TOTAL_BYTES
    {
        return Err(FileAttributeError::new(
            FileAttributeErrorKind::BudgetExceeded,
            "file-xattr-inspect",
            "extended-attribute inspection limit exceeds its fixed ceiling",
        ));
    }
    Ok(())
}

fn namespace(name: &str) -> String {
    name.split_once('.')
        .map_or_else(|| "unqualified".to_owned(), |(prefix, _)| prefix.to_owned())
}

fn state(value: Vec<u8>) -> XattrState {
    XattrState {
        sha256: sha256_hex(&value),
        value,
    }
}

fn state_matches(actual: Option<&[u8]>, expected: Option<&XattrState>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            actual.len() == expected.value.len()
                && sha256_hex(actual) == expected.sha256
                && actual == expected.value
        }
        _ => false,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::{
        fs::OpenOptions,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn fixture() -> (
        std::path::PathBuf,
        std::path::PathBuf,
        File,
        RegularFileBinding,
    ) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agenterm-platform-file-attributes-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create file-attribute fixture");
        let path = root.join("fixture");
        std::fs::write(&path, b"fixture").expect("write file-attribute fixture");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open file-attribute fixture");
        let binding = bind_regular_file(&path, &file).expect("bind fixture identity");
        (root, path, file, binding)
    }

    fn test_name() -> &'static str {
        if cfg!(target_os = "linux") {
            "user.agenterm-test"
        } else {
            "org.agenterm.test"
        }
    }

    #[test]
    fn xattr_inspection_is_digest_only_by_default_and_mutations_rollback() {
        let (root, _path, file, binding) = fixture();
        let name = test_name();
        let plan =
            plan_xattr_set(&file, binding, name, b"secret".to_vec(), 1024).expect("plan xattr set");
        let applied = apply_xattr(&file, &plan).expect("apply xattr set");
        assert_eq!(
            applied.after_sha256.as_deref(),
            Some(sha256_hex(b"secret").as_str())
        );

        let observed =
            inspect_xattrs(&file, binding, XattrInspectLimits::default()).expect("inspect xattrs");
        let item = observed
            .iter()
            .find(|item| item.name == name)
            .expect("test xattr");
        assert_eq!(item.value_bytes, 6);
        assert_eq!(item.value_sha256, sha256_hex(b"secret"));
        assert_eq!(item.value, None);

        let revealed = inspect_xattrs(
            &file,
            binding,
            XattrInspectLimits {
                max_value_bytes: 6,
                max_total_value_bytes: 6,
                include_values: true,
                ..XattrInspectLimits::default()
            },
        )
        .expect("inspect explicitly revealed xattrs");
        assert_eq!(
            revealed
                .iter()
                .find(|item| item.name == name)
                .and_then(|item| item.value.as_deref()),
            Some(b"secret".as_slice())
        );

        let error = inspect_xattrs(
            &file,
            binding,
            XattrInspectLimits {
                max_value_bytes: 5,
                ..XattrInspectLimits::default()
            },
        )
        .expect_err("reject oversized xattr inspection");
        assert_eq!(error.kind, FileAttributeErrorKind::BudgetExceeded);

        let rolled_back = apply_xattr(&file, &applied.rollback).expect("rollback xattr set");
        assert_eq!(rolled_back.after_sha256, None);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn mutation_refuses_changed_precondition_and_mode_has_rollback() {
        let (root, _path, file, binding) = fixture();
        let name = test_name();
        let stale = plan_xattr_set(&file, binding, name, b"planned".to_vec(), 1024)
            .expect("plan xattr set");
        native::set_xattr(&file, name, b"raced").expect("simulate xattr race");
        let error = apply_xattr(&file, &stale).expect_err("reject stale xattr plan");
        assert_eq!(error.kind, FileAttributeErrorKind::PreconditionChanged);

        let original = native::mode(&file).expect("read original mode");
        let error = plan_mode(&file, binding, 0o10_000).expect_err("reject invalid mode bits");
        assert_eq!(error.kind, FileAttributeErrorKind::InvalidMode);
        let requested = if original == 0o600 { 0o640 } else { 0o600 };
        let stale_mode = plan_mode(&file, binding, requested).expect("plan stale mode");
        native::set_mode(&file, requested).expect("simulate mode race");
        let error = apply_mode(&file, &stale_mode).expect_err("reject stale mode plan");
        assert_eq!(error.kind, FileAttributeErrorKind::PreconditionChanged);
        native::set_mode(&file, original).expect("restore mode after race fixture");
        let plan = plan_mode(&file, binding, requested).expect("plan mode");
        let applied = apply_mode(&file, &plan).expect("apply mode");
        assert_eq!(applied.after_mode, requested);
        let rolled_back = apply_mode(&file, &applied.rollback).expect("rollback mode");
        assert_eq!(rolled_back.after_mode, original);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn identity_and_regular_file_guards_fail_closed() {
        let (root, path, file, binding) = fixture();
        let replacement = root.join("replacement");
        std::fs::write(&replacement, b"different").expect("write replacement");
        let replacement_file = File::open(&replacement).expect("open replacement");
        let error = inspect_xattrs(&replacement_file, binding, XattrInspectLimits::default())
            .expect_err("reject mismatched handle");
        assert_eq!(error.kind, FileAttributeErrorKind::IdentityChanged);

        let directory = File::open(&root).expect("open directory fixture");
        let error = bind_regular_file(&root, &directory).expect_err("reject directory binding");
        assert_eq!(error.kind, FileAttributeErrorKind::NotRegularFile);

        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).expect("create symlink fixture");
        let linked_file = File::open(&link).expect("open through symlink");
        let error = bind_regular_file(&link, &linked_file).expect_err("reject symlink binding");
        assert_eq!(error.kind, FileAttributeErrorKind::NotRegularFile);
        drop(directory);
        drop(file);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_clear_is_preconditioned_and_rollback_capable() {
        let (root, _path, file, binding) = fixture();
        let seeded = plan_xattr_set(
            &file,
            binding,
            "com.apple.quarantine",
            b"0081;fixture".to_vec(),
            1024,
        )
        .expect("plan quarantine fixture");
        apply_xattr(&file, &seeded).expect("seed quarantine fixture");

        let clear = plan_clear_quarantine(&file, binding, 1024).expect("plan quarantine clear");
        let cleared = apply_xattr(&file, &clear).expect("clear quarantine fixture");
        assert_eq!(cleared.after_sha256, None);
        let restored = apply_xattr(&file, &cleared.rollback).expect("restore quarantine fixture");
        assert_eq!(
            restored.after_sha256.as_deref(),
            Some(sha256_hex(b"0081;fixture").as_str())
        );
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn quarantine_clear_is_typed_unsupported_on_linux() {
        let (root, _path, file, binding) = fixture();
        let error =
            plan_clear_quarantine(&file, binding, 1024).expect_err("quarantine is macOS-specific");
        assert_eq!(error.kind, FileAttributeErrorKind::Unsupported);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }
}
