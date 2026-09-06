use std::{
    ffi::CStr,
    io::Read,
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::service::{
    SERVICE_FIELD_MAX_BYTES, SERVICE_OUTPUT_MAX_BYTES, ServiceError, ServiceErrorKind,
    ServiceIdentity, ServiceInstanceIdentity, ServiceList, ServiceListBudget,
    ServiceMutationRequest, ServiceOperation, ServiceScope, ServiceSnapshot, ServiceState,
};

const LAUNCHCTL: &str = "/bin/launchctl";
const PLUTIL: &str = "/usr/bin/plutil";

pub(crate) fn validate_identity(identity: &ServiceIdentity) -> Result<(), ServiceError> {
    let scope_valid = match identity.scope {
        ServiceScope::System => identity.provider_scope == "system",
        ServiceScope::User => identity
            .provider_scope
            .strip_prefix("gui/")
            .or_else(|| identity.provider_scope.strip_prefix("user/"))
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit())),
    };
    if identity.provider != "launchd" || !scope_valid {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service identity does not name the selected launchd scope",
        ));
    }
    Ok(())
}

pub(crate) fn identity(
    scope: ServiceScope,
    name: &str,
    deadline: Duration,
) -> Result<ServiceIdentity, ServiceError> {
    let provider_scope = match scope {
        ServiceScope::User => format!("gui/{}", manager_uid(deadline)?),
        ServiceScope::System => "system".into(),
    };
    Ok(ServiceIdentity {
        scope,
        provider: "launchd",
        provider_scope,
        name: name.into(),
    })
}

pub(crate) fn definition_name(path: &Path, deadline: Duration) -> Result<String, ServiceError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            format!("launchd definition metadata is unavailable: {error}"),
        )
    })?;
    // SAFETY: geteuid has no pointer arguments and cannot violate memory
    // safety. The value is compared only with st_uid from the same host.
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file() || metadata.uid() != uid {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "launchd definition must be a current-user-owned regular file",
        ));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            format!("launchd definition path cannot be resolved: {error}"),
        )
    })?;
    let home = current_home(uid)?;
    if canonical.extension().and_then(|value| value.to_str()) != Some("plist")
        || canonical == home
        || !canonical.starts_with(&home)
    {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "launchd definition must be a .plist below the current account home",
        ));
    }
    let path_text = canonical.to_str().ok_or_else(|| {
        ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "launchd definition path is not representable as UTF-8",
        )
    })?;
    let output = run_program(
        PLUTIL,
        &["-extract", "Label", "raw", "-o", "-", path_text],
        deadline,
    )?;
    if !output.success {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            bounded_diagnostic("plutil could not read the launchd Label", &output.stderr),
        ));
    }
    let name = std::str::from_utf8(&output.stdout)
        .map_err(|_| {
            ServiceError::new(
                ServiceErrorKind::InvalidNativeValue,
                "launchd Label is not UTF-8",
            )
        })?
        .trim();
    validate_field(name)?;
    Ok(name.into())
}

fn current_home(uid: libc::uid_t) -> Result<PathBuf, ServiceError> {
    let mut capacity = 16 * 1024;
    while capacity <= 1024 * 1024 {
        let mut record = MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; capacity];
        // SAFETY: record and buffer remain alive for the call, buffer length is
        // exact, and result is checked before any field is dereferenced.
        let rc = unsafe {
            libc::getpwuid_r(
                uid,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            capacity *= 2;
            continue;
        }
        if rc != 0 || result.is_null() {
            return Err(ServiceError::new(
                ServiceErrorKind::QueryFailed,
                "current account home is unavailable from the native account database",
            ));
        }
        // SAFETY: getpwuid_r succeeded and result points at initialized record
        // storage whose strings live inside buffer until this conversion ends.
        let home = unsafe { CStr::from_ptr((*result).pw_dir) };
        return Ok(PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes())));
    }
    Err(ServiceError::new(
        ServiceErrorKind::InventoryTooLarge,
        "current account record exceeded its bounded native buffer",
    ))
}

pub(crate) fn list(
    scope: ServiceScope,
    budget: ServiceListBudget,
) -> Result<ServiceList, ServiceError> {
    let end = Instant::now() + budget.deadline;
    let domains = match scope {
        ServiceScope::User => {
            let uid = manager_uid(remaining(end)?)?;
            vec![format!("gui/{uid}"), format!("user/{uid}")]
        }
        ServiceScope::System => vec!["system".into()],
    };
    let mut services = Vec::new();
    for domain in domains {
        let output = run(&["print", &domain], remaining(end)?)?;
        services.extend(parse_print_domain(&output.stdout, scope, &domain)?);
    }
    if let Some(needle) = budget.match_text.as_deref() {
        let needle = needle.to_lowercase();
        services.retain(|service| service.identity.name.to_lowercase().contains(&needle));
    }
    services.sort_by(|left, right| {
        left.identity
            .provider_scope
            .cmp(&right.identity.provider_scope)
            .then_with(|| left.identity.name.cmp(&right.identity.name))
    });
    let visited = services.len();
    let complete = visited <= budget.max_items;
    services.truncate(budget.max_items);
    Ok(ServiceList {
        services,
        complete,
        visited,
    })
}

fn remaining(end: Instant) -> Result<Duration, ServiceError> {
    let remaining = end.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(ServiceError::new(
            ServiceErrorKind::TimedOut,
            "launchctl service list exceeded its deadline",
        ))
    } else {
        Ok(remaining)
    }
}

pub(crate) fn status(
    identity: &ServiceIdentity,
    deadline: Duration,
) -> Result<ServiceSnapshot, ServiceError> {
    let domain = identity.provider_scope.clone();
    let target = format!("{domain}/{}", identity.name);
    let output = run_allow_failure(&["print", &target], deadline)?;
    if !output.success {
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        if diagnostic.contains("Could not find service") || diagnostic.contains("Bad request") {
            return Ok(missing(identity.clone()));
        }
        return Err(ServiceError::new(
            ServiceErrorKind::QueryFailed,
            bounded_diagnostic("launchctl print failed", &output.stderr),
        ));
    }
    parse_print_service(&output.stdout, identity.clone())
}

pub(crate) fn mutate(request: &ServiceMutationRequest) -> Result<(), ServiceError> {
    let domain = request.expected_before.identity.provider_scope.clone();
    let target = format!("{domain}/{}", request.expected_before.identity.name);
    let definition = request
        .definition
        .as_ref()
        .or(request.expected_before.definition.as_ref());
    let args: Vec<&str> = match request.operation {
        ServiceOperation::Start => vec!["kickstart", &target],
        ServiceOperation::Stop => vec!["kill", "SIGTERM", &target],
        ServiceOperation::Restart => vec!["kickstart", "-k", &target],
        ServiceOperation::Bootstrap => vec![
            "bootstrap",
            &domain,
            definition.and_then(|path| path.to_str()).ok_or_else(|| {
                ServiceError::new(
                    ServiceErrorKind::InvalidRequest,
                    "launchd definition path is not representable as UTF-8",
                )
            })?,
        ],
        ServiceOperation::Bootout => vec!["bootout", &target],
    };
    let output = run_allow_failure(&args, request.deadline).map_err(|error| {
        if error.effect() == crate::service::ServiceEffect::PossiblyApplied {
            error
        } else {
            error.after_effect(crate::service::ServiceRollback::NotNeeded, None)
        }
    })?;
    if output.success {
        Ok(())
    } else {
        Err(ServiceError::new(
            ServiceErrorKind::MutationFailed,
            bounded_diagnostic("launchctl rejected lifecycle request", &output.stderr),
        )
        .after_effect(crate::service::ServiceRollback::NotNeeded, None))
    }
}

fn manager_uid(deadline: Duration) -> Result<String, ServiceError> {
    let output = run(&["manageruid"], deadline)?;
    let uid = std::str::from_utf8(&output.stdout)
        .map_err(|_| {
            ServiceError::new(
                ServiceErrorKind::InvalidNativeValue,
                "launchctl manageruid returned non-UTF-8 output",
            )
        })?
        .trim();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "launchctl manageruid returned an invalid uid",
        ));
    }
    Ok(uid.into())
}

struct Output {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    success: bool,
}

fn run(args: &[&str], deadline: Duration) -> Result<Output, ServiceError> {
    let output = run_allow_failure(args, deadline)?;
    if output.success {
        Ok(output)
    } else {
        Err(ServiceError::new(
            ServiceErrorKind::QueryFailed,
            bounded_diagnostic("launchctl query failed", &output.stderr),
        ))
    }
}

fn run_allow_failure(args: &[&str], deadline: Duration) -> Result<Output, ServiceError> {
    run_program(LAUNCHCTL, args, deadline)
}

fn run_program(program: &str, args: &[&str], deadline: Duration) -> Result<Output, ServiceError> {
    let helper = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("native service helper");
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ServiceError::new(
                ServiceErrorKind::QueryFailed,
                format!("could not start fixed native service helper: {error}"),
            )
        })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out = thread::spawn(move || read_bounded(stdout));
    let err = thread::spawn(move || read_bounded(stderr));
    let end = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err(ServiceError::new(
                    ServiceErrorKind::TimedOut,
                    format!("{helper} exceeded its deadline"),
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err(ServiceError::new(
                    ServiceErrorKind::QueryFailed,
                    format!("{helper} status failed: {error}"),
                ));
            }
        }
    };
    let stdout = out.join().map_err(|_| {
        ServiceError::new(
            ServiceErrorKind::QueryFailed,
            format!("{helper} stdout reader panicked"),
        )
    })??;
    let stderr = err.join().map_err(|_| {
        ServiceError::new(
            ServiceErrorKind::QueryFailed,
            format!("{helper} stderr reader panicked"),
        )
    })??;
    Ok(Output {
        stdout,
        stderr,
        success: status.success(),
    })
}

fn read_bounded(mut reader: impl Read) -> Result<Vec<u8>, ServiceError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((SERVICE_OUTPUT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ServiceError::new(
                ServiceErrorKind::QueryFailed,
                format!("launchctl output read failed: {error}"),
            )
        })?;
    if bytes.len() > SERVICE_OUTPUT_MAX_BYTES {
        return Err(ServiceError::new(
            ServiceErrorKind::InventoryTooLarge,
            "launchctl output exceeded 1 MiB",
        ));
    }
    Ok(bytes)
}

fn parse_print_domain(
    bytes: &[u8],
    scope: ServiceScope,
    domain: &str,
) -> Result<Vec<ServiceSnapshot>, ServiceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "launchctl print returned non-UTF-8 output",
        )
    })?;
    let mut services = Vec::new();
    let mut in_services = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "services = {" {
            in_services = true;
            continue;
        }
        if !in_services {
            continue;
        }
        if trimmed == "}" {
            break;
        }
        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let name = fields[fields.len() - 1];
        validate_field(name)?;
        let pid = fields[0].parse::<u32>().ok().filter(|pid| *pid != 0);
        let failed = fields[fields.len() - 2]
            .parse::<i32>()
            .is_ok_and(|status| status != 0);
        services.push(ServiceSnapshot {
            identity: ServiceIdentity {
                scope,
                provider: "launchd",
                provider_scope: domain.into(),
                name: name.into(),
            },
            instance: pid.map(|pid| ServiceInstanceIdentity {
                provider: "launchd",
                opaque: format!("pid:{pid}"),
            }),
            state: if pid.is_some() {
                ServiceState::Running
            } else if failed {
                ServiceState::Failed
            } else {
                ServiceState::LoadedInactive
            },
            substate: fields[fields.len() - 2].into(),
            description: String::new(),
            definition: None,
        });
    }
    Ok(services)
}

fn parse_print_service(
    bytes: &[u8],
    identity: ServiceIdentity,
) -> Result<ServiceSnapshot, ServiceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "launchctl print returned non-UTF-8 output",
        )
    })?;
    let value = |key: &str| {
        text.lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix(key)
                    .and_then(|tail| tail.strip_prefix(" = "))
            })
            .map(str::trim)
    };
    let pid = value("pid").and_then(|value| value.parse::<u32>().ok());
    let runs = value("runs").unwrap_or("unknown");
    let state_text = value("state").unwrap_or("unknown");
    let state = match state_text {
        "running" => ServiceState::Running,
        "waiting" | "spawn scheduled" => ServiceState::Activating,
        "exited" => ServiceState::LoadedInactive,
        _ => ServiceState::Unknown,
    };
    let path = value("path").map(PathBuf::from);
    Ok(ServiceSnapshot {
        identity,
        instance: pid.map(|pid| ServiceInstanceIdentity {
            provider: "launchd",
            opaque: format!("pid:{pid};runs:{runs}"),
        }),
        state,
        substate: state_text.into(),
        description: String::new(),
        definition: path,
    })
}

fn missing(identity: ServiceIdentity) -> ServiceSnapshot {
    ServiceSnapshot {
        identity,
        instance: None,
        state: ServiceState::Missing,
        substate: String::new(),
        description: String::new(),
        definition: None,
    }
}

fn validate_field(value: &str) -> Result<(), ServiceError> {
    if value.is_empty()
        || value.len() > SERVICE_FIELD_MAX_BYTES
        || value.chars().any(char::is_control)
    {
        Err(ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "launchd returned an invalid service field",
        ))
    } else {
        Ok(())
    }
}

fn bounded_diagnostic(prefix: &str, bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    format!(
        "{prefix}: {}",
        text.trim().chars().take(512).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_launchctl_domain_fixture() {
        let rows =
            b"gui/501 = {\nservices = {\n42 0 com.example.live\n0 1 com.example.failed\n}\n}\n";
        let parsed = parse_print_domain(rows, ServiceScope::User, "gui/501").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].state, ServiceState::Running);
        assert_eq!(parsed[1].state, ServiceState::Failed);
    }

    #[test]
    fn parses_launchctl_print_without_inventing_instance() {
        let state = parse_print_service(
            b"state = waiting\nruns = 2\n",
            ServiceIdentity {
                scope: ServiceScope::User,
                provider: "launchd",
                provider_scope: "gui/501".into(),
                name: "x".into(),
            },
        )
        .unwrap();
        assert_eq!(state.state, ServiceState::Activating);
        assert_eq!(state.instance, None);
    }
}
