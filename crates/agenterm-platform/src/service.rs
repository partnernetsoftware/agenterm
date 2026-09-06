//! Product-neutral service lifecycle facade.
//!
//! System services are observable here, but privileged mutation belongs to an
//! upper-layer privilege plan and provider.

#[path = "contract/service.rs"]
mod contract;

pub use contract::*;

#[cfg(target_os = "linux")]
#[path = "adapters/linux/service.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/service.rs"]
mod native;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "adapters/unsupported_service.rs"]
mod native;

use std::time::{Duration, Instant};

pub fn list(scope: ServiceScope, budget: ServiceListBudget) -> Result<ServiceList, ServiceError> {
    validate_budget(budget.max_items, budget.deadline)?;
    if budget.match_text.as_ref().is_some_and(|value| {
        value.len() > SERVICE_FIELD_MAX_BYTES || value.chars().any(char::is_control)
    }) {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service match text is oversized or contains control characters",
        ));
    }
    native::list(scope, budget)
}

/// Resolve a provider-qualified identity in the current native authority
/// domain without performing a service query or mutation.
pub fn identity(
    scope: ServiceScope,
    name: &str,
    deadline: Duration,
) -> Result<ServiceIdentity, ServiceError> {
    let placeholder = ServiceIdentity {
        scope,
        provider: "",
        provider_scope: String::new(),
        name: name.to_owned(),
    };
    if placeholder.name.is_empty()
        || placeholder.name.len() > SERVICE_FIELD_MAX_BYTES
        || placeholder.name.chars().any(char::is_control)
    {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service name is empty, oversized, or contains control characters",
        ));
    }
    validate_deadline(deadline)?;
    native::identity(scope, name, deadline)
}

/// Validate a native service definition and return its declared service name.
/// This is intentionally provider-owned because launchd plist admission is a
/// macOS filesystem/account contract, not product parsing logic.
pub fn definition_name(path: &std::path::Path, deadline: Duration) -> Result<String, ServiceError> {
    validate_deadline(deadline)?;
    native::definition_name(path, deadline)
}

pub fn status(
    identity: &ServiceIdentity,
    deadline: Duration,
) -> Result<ServiceSnapshot, ServiceError> {
    validate_identity(identity)?;
    validate_deadline(deadline)?;
    native::status(identity, deadline)
}

pub fn mutate(request: &ServiceMutationRequest) -> Result<ServiceMutationReceipt, ServiceError> {
    if request.expected_before.identity.scope == ServiceScope::System {
        return Err(ServiceError::new(
            ServiceErrorKind::RequiresPrivilege,
            "system service mutation requires an upper-layer privilege plan",
        ));
    }
    validate_request(request)?;
    let operation_deadline = Instant::now() + request.deadline;
    let before = native::status(
        &request.expected_before.identity,
        remaining(
            operation_deadline,
            "service before-state read exceeded its deadline",
        )?,
    )?;
    if before != request.expected_before {
        return Err(ServiceError::new(
            ServiceErrorKind::StateChanged,
            "service state or instance identity changed before mutation",
        ));
    }
    if operation_already_satisfied(request.operation, &before) {
        return Ok(ServiceMutationReceipt {
            operation: request.operation,
            before: before.clone(),
            after: before,
            performed: false,
            verified: true,
        });
    }

    let native_request = ServiceMutationRequest {
        operation: request.operation,
        expected_before: request.expected_before.clone(),
        definition: request.definition.clone(),
        deadline: remaining(
            operation_deadline,
            "service mutation exceeded its total deadline before dispatch",
        )?,
    };
    native::mutate(&native_request).map_err(|error| {
        if error.effect() == ServiceEffect::PossiblyApplied {
            rollback_after_effect(request, &before, error)
        } else {
            error
        }
    })?;

    let observed = loop {
        let status_budget = match remaining(
            operation_deadline,
            "service mutation readback exceeded its total deadline",
        ) {
            Ok(budget) => budget,
            Err(error) => {
                return Err(rollback_after_effect(
                    request,
                    &before,
                    error.after_effect(ServiceRollback::NotNeeded, None),
                ));
            }
        };
        match native::status(&before.identity, status_budget) {
            Ok(snapshot) if postcondition(request.operation, &before, &snapshot) => break snapshot,
            Ok(snapshot) if Instant::now() >= operation_deadline => {
                let error = ServiceError::new(
                    ServiceErrorKind::VerificationFailed,
                    "service mutation did not reach the requested state",
                );
                return Err(rollback_after_effect(
                    request,
                    &before,
                    error.after_effect(ServiceRollback::NotNeeded, Some(snapshot)),
                ));
            }
            Err(error) if Instant::now() >= operation_deadline => {
                return Err(rollback_after_effect(
                    request,
                    &before,
                    error.after_effect(ServiceRollback::NotNeeded, None),
                ));
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    Ok(ServiceMutationReceipt {
        operation: request.operation,
        before,
        after: observed,
        performed: true,
        verified: true,
    })
}

fn rollback_after_effect(
    request: &ServiceMutationRequest,
    before: &ServiceSnapshot,
    error: ServiceError,
) -> ServiceError {
    // A transport timeout only proves that the outcome is unknown.  Observe
    // before compensating: blindly applying the inverse can create the very
    // state change that the timed-out request never made.
    let rollback_deadline = Instant::now() + request.deadline;
    let observed_before_rollback = remaining(
        rollback_deadline,
        "service rollback observation deadline expired",
    )
    .and_then(|deadline| native::status(&before.identity, deadline))
    .ok();
    if observed_before_rollback.as_ref() == Some(before) {
        return error.after_effect(ServiceRollback::NotNeeded, observed_before_rollback);
    }

    let inverse = match request.operation {
        ServiceOperation::Start => Some(ServiceOperation::Stop),
        ServiceOperation::Stop => Some(ServiceOperation::Start),
        ServiceOperation::Bootstrap => Some(ServiceOperation::Bootout),
        ServiceOperation::Bootout => Some(ServiceOperation::Bootstrap),
        ServiceOperation::Restart => None,
    };
    let Some(operation) = inverse else {
        let observed = error.observed().cloned();
        return error.after_effect(ServiceRollback::Unsupported, observed);
    };
    // Compensation receives one separate bounded window because the requested
    // operation deadline may already have expired after dispatch. Verification
    // requires the complete old snapshot, including instance identity: a
    // Stop→Start that creates a new incarnation is therefore honestly Failed,
    // never reported as a verified restoration.
    let rollback_request = ServiceMutationRequest {
        operation,
        // The compensating call is bound to what we actually observed after
        // the uncertain request, not to the stale pre-mutation snapshot.
        expected_before: observed_before_rollback
            .clone()
            .unwrap_or_else(|| before.clone()),
        definition: request
            .definition
            .clone()
            .or_else(|| before.definition.clone()),
        deadline: match remaining(rollback_deadline, "service rollback deadline expired") {
            Ok(deadline) => deadline,
            Err(_) => {
                return error.after_effect(ServiceRollback::Failed, None);
            }
        },
    };
    let rollback = if native::mutate(&rollback_request).is_ok()
        && remaining(
            rollback_deadline,
            "service rollback readback deadline expired",
        )
        .and_then(|deadline| native::status(&before.identity, deadline))
        .is_ok_and(|state| state == *before)
    {
        ServiceRollback::Verified
    } else {
        ServiceRollback::Failed
    };
    let observed = remaining(
        rollback_deadline,
        "service rollback observation deadline expired",
    )
    .and_then(|deadline| native::status(&before.identity, deadline))
    .ok();
    error.after_effect(rollback, observed)
}

fn validate_request(request: &ServiceMutationRequest) -> Result<(), ServiceError> {
    validate_identity(&request.expected_before.identity)?;
    validate_deadline(request.deadline)?;
    if matches!(request.operation, ServiceOperation::Bootstrap)
        && request
            .definition
            .as_ref()
            .or(request.expected_before.definition.as_ref())
            .is_none()
    {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "bootstrap requires an explicit native definition path",
        ));
    }
    Ok(())
}

fn validate_identity(identity: &ServiceIdentity) -> Result<(), ServiceError> {
    if identity.name.is_empty()
        || identity.name.len() > SERVICE_FIELD_MAX_BYTES
        || identity.name.chars().any(char::is_control)
    {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service name is empty, oversized, or contains control characters",
        ));
    }
    native::validate_identity(identity)
}

fn validate_budget(max_items: usize, deadline: Duration) -> Result<(), ServiceError> {
    if !(1..=SERVICE_MAX_ITEMS).contains(&max_items) {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service max_items must be in 1..=5000",
        ));
    }
    validate_deadline(deadline)
}

fn validate_deadline(deadline: Duration) -> Result<(), ServiceError> {
    if deadline.is_zero() || deadline > SERVICE_MAX_DEADLINE {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service deadline must be in 1ns..=30s",
        ));
    }
    Ok(())
}

fn remaining(deadline: Instant, detail: &'static str) -> Result<Duration, ServiceError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(ServiceError::new(ServiceErrorKind::TimedOut, detail))
    } else {
        Ok(remaining)
    }
}

fn operation_already_satisfied(operation: ServiceOperation, state: &ServiceSnapshot) -> bool {
    matches!(operation, ServiceOperation::Start) && state.state == ServiceState::Running
        || matches!(
            operation,
            ServiceOperation::Stop | ServiceOperation::Bootout
        ) && state.state == ServiceState::Missing
        || matches!(operation, ServiceOperation::Bootstrap) && state.state != ServiceState::Missing
}

fn postcondition(
    operation: ServiceOperation,
    before: &ServiceSnapshot,
    state: &ServiceSnapshot,
) -> bool {
    match operation {
        ServiceOperation::Start => state.state == ServiceState::Running,
        ServiceOperation::Restart => {
            state.state == ServiceState::Running
                && state.instance.is_some()
                && state.instance != before.instance
        }
        ServiceOperation::Stop => !matches!(
            state.state,
            ServiceState::Running | ServiceState::Activating | ServiceState::Deactivating
        ),
        ServiceOperation::Bootstrap => state.state != ServiceState::Missing,
        ServiceOperation::Bootout => state.state == ServiceState::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(scope: ServiceScope, state: ServiceState) -> ServiceSnapshot {
        ServiceSnapshot {
            identity: ServiceIdentity {
                scope,
                provider: "fixture",
                provider_scope: "fixture-user".into(),
                name: "example.service".into(),
            },
            instance: None,
            state,
            substate: String::new(),
            description: String::new(),
            definition: None,
        }
    }

    #[test]
    fn rejects_unbounded_queries_before_native_work() {
        let error = list(
            ServiceScope::User,
            ServiceListBudget {
                max_items: SERVICE_MAX_ITEMS + 1,
                deadline: Duration::from_secs(1),
                match_text: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), ServiceErrorKind::InvalidRequest);
        assert_eq!(error.effect(), ServiceEffect::NotPerformed);
    }

    #[test]
    fn system_mutation_requires_privileged_provider() {
        let request = ServiceMutationRequest {
            operation: ServiceOperation::Start,
            expected_before: snapshot(ServiceScope::System, ServiceState::LoadedInactive),
            definition: None,
            deadline: Duration::from_secs(1),
        };
        assert_eq!(
            mutate(&request).unwrap_err().kind(),
            ServiceErrorKind::RequiresPrivilege
        );
    }

    #[test]
    fn restart_is_never_collapsed_to_running_noop() {
        assert!(!operation_already_satisfied(
            ServiceOperation::Restart,
            &snapshot(ServiceScope::User, ServiceState::Running)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_launchd_list_and_status_are_read_only_and_bounded() {
        let inventory = list(
            ServiceScope::User,
            ServiceListBudget {
                max_items: SERVICE_MAX_ITEMS,
                deadline: Duration::from_secs(5),
                match_text: None,
            },
        )
        .expect("bounded current-user launchd inventory");
        let identity = inventory
            .services
            .first()
            .expect("launchd user domain has at least one service")
            .identity
            .clone();
        let observed =
            status(&identity, Duration::from_secs(5)).expect("read-only launchd status lookup");
        assert_eq!(observed.identity, identity);
    }
}
