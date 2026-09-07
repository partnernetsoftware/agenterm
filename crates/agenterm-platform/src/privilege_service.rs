//! Typed lifecycle for a bundled macOS privilege service.
//!
//! Status is observation only. Registration and unregistration are separate,
//! explicit mutations. The native adapter owns SMAppService and Authorization
//! Services calls; callers own the product-specific label, right and fixed
//! executable location.

use std::{
    fmt,
    path::{Component, Path},
};

#[cfg(target_os = "macos")]
#[path = "adapters/macos/privilege_service.rs"]
mod native;

#[cfg(any(target_os = "macos", test))]
const AUTHORIZATION_RULE: &str = "authenticate-admin";
const MAX_NATIVE_NAME_BYTES: usize = 255;

/// Product-owned identifiers projected into the platform lifecycle.
#[derive(Clone, Copy, Debug)]
pub struct PrivilegeServiceDefinition<'a> {
    pub daemon_plist_name: &'a str,
    pub authorization_right: &'a str,
    pub expected_executable: &'a Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrivilegeDaemonStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrivilegeRightStatus {
    Missing,
    Expected,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivilegeServiceStatus {
    pub daemon: PrivilegeDaemonStatus,
    pub authorization_right: PrivilegeRightStatus,
    pub fixed_executable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrivilegeServiceErrorKind {
    Unsupported,
    InvalidDefinition,
    InvalidExecutableLocation,
    AuthorizationRightConflict,
    NativeFailure,
    IncompleteTransition,
    RollbackFailed,
}

#[derive(Debug)]
pub struct PrivilegeServiceError {
    kind: PrivilegeServiceErrorKind,
    message: String,
}

impl PrivilegeServiceError {
    fn new(kind: PrivilegeServiceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PrivilegeServiceErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PrivilegeServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PrivilegeServiceError {}

pub type PrivilegeServiceResult<T> = Result<T, PrivilegeServiceError>;

/// Observe the service, right, and fixed executable location without mutation.
pub fn status(
    definition: &PrivilegeServiceDefinition<'_>,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    validate_definition(definition)?;
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|error| {
            PrivilegeServiceError::new(
                PrivilegeServiceErrorKind::NativeFailure,
                format!("cannot resolve the current executable: {error}"),
            )
        })?;
        let mut backend = native::MacosPrivilegeServiceBackend;
        observe(&mut backend, definition, &executable)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// Install the exact right if absent, then register the bundled daemon.
///
/// `RequiresApproval` is returned as an observable daemon state. It is not
/// promoted to `Enabled`, and this function never opens System Settings.
pub fn register(
    definition: &PrivilegeServiceDefinition<'_>,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    validate_definition(definition)?;
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|error| {
            PrivilegeServiceError::new(
                PrivilegeServiceErrorKind::NativeFailure,
                format!("cannot resolve the current executable: {error}"),
            )
        })?;
        let mut backend = native::MacosPrivilegeServiceBackend;
        register_with(&mut backend, definition, &executable)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// Unregister the daemon completely before removing the exact right.
///
/// A conflicting right is never overwritten or deleted. If daemon removal
/// does not complete, the right remains installed so no partial teardown can
/// make a still-registered provider lose its authorization contract.
pub fn unregister(
    definition: &PrivilegeServiceDefinition<'_>,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    validate_definition(definition)?;
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|error| {
            PrivilegeServiceError::new(
                PrivilegeServiceErrorKind::NativeFailure,
                format!("cannot resolve the current executable: {error}"),
            )
        })?;
        let mut backend = native::MacosPrivilegeServiceBackend;
        unregister_with(&mut backend, definition, &executable)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

fn validate_definition(definition: &PrivilegeServiceDefinition<'_>) -> PrivilegeServiceResult<()> {
    if !valid_plist_name(definition.daemon_plist_name) {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::InvalidDefinition,
            "daemon plist name must be one bounded leaf ending in .plist",
        ));
    }
    if !valid_right_name(definition.authorization_right) {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::InvalidDefinition,
            "authorization right must be one bounded operation-scoped dotted identifier",
        ));
    }
    if !definition.expected_executable.is_absolute() {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::InvalidDefinition,
            "expected privilege-service executable location must be absolute",
        ));
    }
    if !valid_fixed_app_executable(definition.expected_executable) {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::InvalidDefinition,
            "expected executable must be one normalized /Applications/<bundle>.app/Contents/MacOS/<leaf> path",
        ));
    }
    Ok(())
}

fn valid_plist_name(value: &str) -> bool {
    let Some(label) = value.strip_suffix(".plist") else {
        return false;
    };
    !label.is_empty()
        && value.len() <= MAX_NATIVE_NAME_BYTES
        && label.split('.').count() >= 3
        && label.split('.').all(valid_identifier_part)
}

fn valid_right_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NATIVE_NAME_BYTES
        && !value.as_bytes().contains(&0)
        && !value.contains('*')
        && value.split('.').count() >= 4
        && value.split('.').all(valid_identifier_part)
}

fn valid_identifier_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_fixed_app_executable(path: &Path) -> bool {
    let components: Vec<_> = path.components().collect();
    matches!(
        components.as_slice(),
        [
            Component::RootDir,
            Component::Normal(applications),
            Component::Normal(bundle),
            Component::Normal(contents),
            Component::Normal(macos),
            Component::Normal(executable)
        ] if *applications == "Applications"
            && bundle.to_str().is_some_and(|value| value.ends_with(".app") && value.len() > 4)
            && *contents == "Contents"
            && *macos == "MacOS"
            && !executable.is_empty()
    )
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> PrivilegeServiceError {
    PrivilegeServiceError::new(
        PrivilegeServiceErrorKind::Unsupported,
        "SMAppService privilege lifecycle is available only on macOS",
    )
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct BackendError(String);

#[cfg(any(target_os = "macos", test))]
trait Backend {
    fn daemon_status(&mut self, plist_name: &str) -> Result<PrivilegeDaemonStatus, BackendError>;
    fn right_status(&mut self, right: &str) -> Result<PrivilegeRightStatus, BackendError>;
    fn install_right(&mut self, right: &str, rule: &str) -> Result<(), BackendError>;
    fn remove_right(&mut self, right: &str) -> Result<(), BackendError>;
    fn register_daemon(&mut self, plist_name: &str) -> Result<(), BackendError>;
    fn unregister_daemon(&mut self, plist_name: &str) -> Result<(), BackendError>;
}

#[cfg(any(target_os = "macos", test))]
fn backend_error(operation: &str, error: BackendError) -> PrivilegeServiceError {
    PrivilegeServiceError::new(
        PrivilegeServiceErrorKind::NativeFailure,
        format!("{operation}: {}", error.0),
    )
}

#[cfg(any(target_os = "macos", test))]
fn observe(
    backend: &mut impl Backend,
    definition: &PrivilegeServiceDefinition<'_>,
    executable: &Path,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    let daemon = backend
        .daemon_status(definition.daemon_plist_name)
        .map_err(|error| backend_error("cannot read SMAppService status", error))?;
    let authorization_right = backend
        .right_status(definition.authorization_right)
        .map_err(|error| backend_error("cannot read Authorization Services right", error))?;
    Ok(PrivilegeServiceStatus {
        daemon,
        authorization_right,
        fixed_executable: executable == definition.expected_executable,
    })
}

#[cfg(any(target_os = "macos", test))]
fn require_fixed_location(status: PrivilegeServiceStatus) -> PrivilegeServiceResult<()> {
    if status.fixed_executable {
        Ok(())
    } else {
        Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::InvalidExecutableLocation,
            "privilege-service lifecycle requires the fixed installed app executable",
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
fn require_no_right_conflict(status: PrivilegeServiceStatus) -> PrivilegeServiceResult<()> {
    if status.authorization_right == PrivilegeRightStatus::Conflict {
        Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::AuthorizationRightConflict,
            "the operation-scoped authorization right exists with a different definition",
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(target_os = "macos", test))]
fn register_with(
    backend: &mut impl Backend,
    definition: &PrivilegeServiceDefinition<'_>,
    executable: &Path,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    let before = observe(backend, definition, executable)?;
    require_fixed_location(before)?;
    require_no_right_conflict(before)?;

    let installed_right = before.authorization_right == PrivilegeRightStatus::Missing;
    if installed_right {
        backend
            .install_right(definition.authorization_right, AUTHORIZATION_RULE)
            .map_err(|error| backend_error("cannot install Authorization Services right", error))?;
        match backend.right_status(definition.authorization_right) {
            Ok(PrivilegeRightStatus::Expected) => {}
            Ok(_) => {
                return Err(PrivilegeServiceError::new(
                    PrivilegeServiceErrorKind::IncompleteTransition,
                    "authorization right did not read back as the exact expected definition",
                ));
            }
            Err(error) => {
                return Err(backend_error(
                    "cannot verify the installed Authorization Services right",
                    error,
                ));
            }
        }
    }

    if matches!(
        before.daemon,
        PrivilegeDaemonStatus::NotRegistered | PrivilegeDaemonStatus::NotFound
    ) && let Err(register_error) = backend.register_daemon(definition.daemon_plist_name)
    {
        if installed_right {
            rollback_new_right(backend, definition.authorization_right, &register_error)?;
        }
        return Err(backend_error(
            "cannot register bundled SMAppService daemon",
            register_error,
        ));
    }

    let after = observe(backend, definition, executable)?;
    require_no_right_conflict(after)?;
    if after.authorization_right != PrivilegeRightStatus::Expected
        || matches!(
            after.daemon,
            PrivilegeDaemonStatus::NotRegistered | PrivilegeDaemonStatus::NotFound
        )
    {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::IncompleteTransition,
            "privilege-service registration did not reach an observable registered state",
        ));
    }
    Ok(after)
}

#[cfg(any(target_os = "macos", test))]
fn rollback_new_right(
    backend: &mut impl Backend,
    right: &str,
    register_error: &BackendError,
) -> PrivilegeServiceResult<()> {
    match backend.right_status(right) {
        Ok(PrivilegeRightStatus::Expected) => {
            backend.remove_right(right).map_err(|rollback_error| {
                PrivilegeServiceError::new(
                    PrivilegeServiceErrorKind::RollbackFailed,
                    format!(
                        "daemon registration failed ({}), then exact-right rollback failed ({})",
                        register_error.0, rollback_error.0
                    ),
                )
            })
        }
        Ok(PrivilegeRightStatus::Missing) => Ok(()),
        Ok(PrivilegeRightStatus::Conflict) => Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::RollbackFailed,
            format!(
                "daemon registration failed ({}); authorization right changed and was preserved",
                register_error.0
            ),
        )),
        Err(rollback_error) => Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::RollbackFailed,
            format!(
                "daemon registration failed ({}); exact-right rollback preflight failed ({})",
                register_error.0, rollback_error.0
            ),
        )),
    }
}

#[cfg(any(target_os = "macos", test))]
fn unregister_with(
    backend: &mut impl Backend,
    definition: &PrivilegeServiceDefinition<'_>,
    executable: &Path,
) -> PrivilegeServiceResult<PrivilegeServiceStatus> {
    let before = observe(backend, definition, executable)?;
    require_fixed_location(before)?;
    require_no_right_conflict(before)?;

    if matches!(
        before.daemon,
        PrivilegeDaemonStatus::Enabled | PrivilegeDaemonStatus::RequiresApproval
    ) {
        backend
            .unregister_daemon(definition.daemon_plist_name)
            .map_err(|error| {
                backend_error("cannot unregister bundled SMAppService daemon", error)
            })?;
    }
    let daemon_after = backend
        .daemon_status(definition.daemon_plist_name)
        .map_err(|error| backend_error("cannot verify SMAppService unregistration", error))?;
    if !matches!(
        daemon_after,
        PrivilegeDaemonStatus::NotRegistered | PrivilegeDaemonStatus::NotFound
    ) {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::IncompleteTransition,
            "SMAppService daemon remains registered; authorization right was preserved",
        ));
    }

    match backend
        .right_status(definition.authorization_right)
        .map_err(|error| backend_error("cannot verify right before removal", error))?
    {
        PrivilegeRightStatus::Expected => backend
            .remove_right(definition.authorization_right)
            .map_err(|error| {
                backend_error("cannot remove exact Authorization Services right", error)
            })?,
        PrivilegeRightStatus::Missing => {}
        PrivilegeRightStatus::Conflict => {
            return Err(PrivilegeServiceError::new(
                PrivilegeServiceErrorKind::AuthorizationRightConflict,
                "authorization right changed during unregistration and was preserved",
            ));
        }
    }

    let after = observe(backend, definition, executable)?;
    if !matches!(
        after.daemon,
        PrivilegeDaemonStatus::NotRegistered | PrivilegeDaemonStatus::NotFound
    ) || after.authorization_right != PrivilegeRightStatus::Missing
    {
        return Err(PrivilegeServiceError::new(
            PrivilegeServiceErrorKind::IncompleteTransition,
            "privilege-service unregistration did not reach the fully removed state",
        ));
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fixture {
        daemon: Option<PrivilegeDaemonStatus>,
        right: Option<PrivilegeRightStatus>,
        events: Vec<&'static str>,
        fail_register: bool,
        fail_unregister: bool,
        fail_remove: bool,
    }

    impl Backend for Fixture {
        fn daemon_status(
            &mut self,
            _plist_name: &str,
        ) -> Result<PrivilegeDaemonStatus, BackendError> {
            self.events.push("daemon-status");
            Ok(self.daemon.unwrap_or(PrivilegeDaemonStatus::NotRegistered))
        }

        fn right_status(&mut self, _right: &str) -> Result<PrivilegeRightStatus, BackendError> {
            self.events.push("right-status");
            Ok(self.right.unwrap_or(PrivilegeRightStatus::Missing))
        }

        fn install_right(&mut self, _right: &str, rule: &str) -> Result<(), BackendError> {
            assert_eq!(rule, AUTHORIZATION_RULE);
            self.events.push("install-right");
            self.right = Some(PrivilegeRightStatus::Expected);
            Ok(())
        }

        fn remove_right(&mut self, _right: &str) -> Result<(), BackendError> {
            self.events.push("remove-right");
            if self.fail_remove {
                return Err(BackendError("fixture remove failed".into()));
            }
            self.right = Some(PrivilegeRightStatus::Missing);
            Ok(())
        }

        fn register_daemon(&mut self, _plist_name: &str) -> Result<(), BackendError> {
            self.events.push("register-daemon");
            if self.fail_register {
                return Err(BackendError("fixture register failed".into()));
            }
            self.daemon = Some(PrivilegeDaemonStatus::RequiresApproval);
            Ok(())
        }

        fn unregister_daemon(&mut self, _plist_name: &str) -> Result<(), BackendError> {
            self.events.push("unregister-daemon");
            if self.fail_unregister {
                return Err(BackendError("fixture unregister failed".into()));
            }
            self.daemon = Some(PrivilegeDaemonStatus::NotRegistered);
            Ok(())
        }
    }

    fn definition<'a>(expected_executable: &'a Path) -> PrivilegeServiceDefinition<'a> {
        PrivilegeServiceDefinition {
            daemon_plist_name: "com.example.product.privilege.plist",
            authorization_right: "com.example.product.operation",
            expected_executable,
        }
    }

    #[test]
    fn status_is_read_only_and_reports_location() {
        let expected = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut fixture = Fixture {
            daemon: Some(PrivilegeDaemonStatus::RequiresApproval),
            right: Some(PrivilegeRightStatus::Expected),
            ..Fixture::default()
        };
        let reply = observe(&mut fixture, &definition(expected), expected).expect("status");
        assert_eq!(reply.daemon, PrivilegeDaemonStatus::RequiresApproval);
        assert!(reply.fixed_executable);
        assert_eq!(fixture.events, ["daemon-status", "right-status"]);
    }

    #[test]
    fn register_installs_right_before_daemon_and_preserves_requires_approval() {
        let executable = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut fixture = Fixture::default();
        let reply = register_with(&mut fixture, &definition(executable), executable)
            .expect("fixture registration");
        assert_eq!(reply.daemon, PrivilegeDaemonStatus::RequiresApproval);
        let right_index = fixture
            .events
            .iter()
            .position(|event| *event == "install-right")
            .expect("right installed");
        let daemon_index = fixture
            .events
            .iter()
            .position(|event| *event == "register-daemon")
            .expect("daemon registered");
        assert!(right_index < daemon_index);
    }

    #[test]
    fn register_failure_rolls_back_only_the_new_exact_right() {
        let executable = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut fixture = Fixture {
            fail_register: true,
            ..Fixture::default()
        };
        let error = register_with(&mut fixture, &definition(executable), executable)
            .expect_err("registration must fail");
        assert_eq!(error.kind(), PrivilegeServiceErrorKind::NativeFailure);
        assert_eq!(fixture.right, Some(PrivilegeRightStatus::Missing));
        assert!(fixture.events.contains(&"remove-right"));
    }

    #[test]
    fn unregister_completes_daemon_removal_before_exact_right_removal() {
        let executable = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut fixture = Fixture {
            daemon: Some(PrivilegeDaemonStatus::Enabled),
            right: Some(PrivilegeRightStatus::Expected),
            ..Fixture::default()
        };
        unregister_with(&mut fixture, &definition(executable), executable)
            .expect("fixture unregistration");
        let unregister_index = fixture
            .events
            .iter()
            .position(|event| *event == "unregister-daemon")
            .expect("daemon unregistered");
        let remove_index = fixture
            .events
            .iter()
            .position(|event| *event == "remove-right")
            .expect("right removed");
        assert!(unregister_index < remove_index);
    }

    #[test]
    fn unregister_failure_preserves_right() {
        let executable = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut fixture = Fixture {
            daemon: Some(PrivilegeDaemonStatus::Enabled),
            right: Some(PrivilegeRightStatus::Expected),
            fail_unregister: true,
            ..Fixture::default()
        };
        let error = unregister_with(&mut fixture, &definition(executable), executable)
            .expect_err("unregistration must fail");
        assert_eq!(error.kind(), PrivilegeServiceErrorKind::NativeFailure);
        assert_eq!(fixture.right, Some(PrivilegeRightStatus::Expected));
        assert!(!fixture.events.contains(&"remove-right"));
    }

    #[test]
    fn conflict_and_non_fixed_location_fail_before_mutation() {
        let expected = Path::new("/Applications/Product.app/Contents/MacOS/product");
        let mut conflict = Fixture {
            right: Some(PrivilegeRightStatus::Conflict),
            ..Fixture::default()
        };
        let error = register_with(&mut conflict, &definition(expected), expected)
            .expect_err("conflict must fail");
        assert_eq!(
            error.kind(),
            PrivilegeServiceErrorKind::AuthorizationRightConflict
        );
        assert_eq!(conflict.events, ["daemon-status", "right-status"]);

        let mut misplaced = Fixture::default();
        let error = register_with(
            &mut misplaced,
            &definition(expected),
            Path::new("/tmp/Product.app/Contents/MacOS/product"),
        )
        .expect_err("misplaced executable must fail");
        assert_eq!(
            error.kind(),
            PrivilegeServiceErrorKind::InvalidExecutableLocation
        );
        assert_eq!(misplaced.events, ["daemon-status", "right-status"]);
    }

    #[test]
    fn definitions_reject_wildcards_paths_and_non_operation_names() {
        let executable = Path::new("/Applications/Product.app/Contents/MacOS/product");
        for definition in [
            PrivilegeServiceDefinition {
                daemon_plist_name: "../daemon.plist",
                authorization_right: "com.example.product.operation",
                expected_executable: executable,
            },
            PrivilegeServiceDefinition {
                daemon_plist_name: "daemon.plist",
                authorization_right: "com.example.*",
                expected_executable: executable,
            },
            PrivilegeServiceDefinition {
                daemon_plist_name: "daemon.plist",
                authorization_right: "operation",
                expected_executable: executable,
            },
        ] {
            assert_eq!(
                validate_definition(&definition)
                    .expect_err("invalid definition")
                    .kind(),
                PrivilegeServiceErrorKind::InvalidDefinition
            );
        }
    }
}
