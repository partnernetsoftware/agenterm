use std::{path::PathBuf, sync::OnceLock, time::Duration};

use tokio::time::timeout;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

use crate::service::{
    SERVICE_FIELD_MAX_BYTES, ServiceError, ServiceErrorKind, ServiceIdentity,
    ServiceInstanceIdentity, ServiceList, ServiceListBudget, ServiceMutationRequest,
    ServiceOperation, ServiceRollback, ServiceScope, ServiceSnapshot, ServiceState,
};

const SYSTEMD_DEST: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

type UnitRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn validate_identity(identity: &ServiceIdentity) -> Result<(), ServiceError> {
    let expected_scope = match identity.scope {
        ServiceScope::User => "user",
        ServiceScope::System => "system",
    };
    if identity.provider != "systemd" || identity.provider_scope != expected_scope {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidRequest,
            "service identity does not name the selected systemd scope",
        ));
    }
    Ok(())
}

pub(crate) fn identity(
    scope: ServiceScope,
    name: &str,
    _deadline: Duration,
) -> Result<ServiceIdentity, ServiceError> {
    Ok(ServiceIdentity {
        scope,
        provider: "systemd",
        provider_scope: match scope {
            ServiceScope::User => "user",
            ServiceScope::System => "system",
        }
        .into(),
        name: name.into(),
    })
}

pub(crate) fn definition_name(
    _path: &std::path::Path,
    _deadline: Duration,
) -> Result<String, ServiceError> {
    Err(ServiceError::new(
        ServiceErrorKind::Unsupported,
        "systemd enable/disable needs a separate typed unit-file contract",
    ))
}

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for systemd D-Bus")
    })
}

pub(crate) fn list(
    scope: ServiceScope,
    budget: ServiceListBudget,
) -> Result<ServiceList, ServiceError> {
    runtime().block_on(async {
        timeout(
            budget.deadline,
            list_async(scope, budget.max_items, budget.match_text.as_deref()),
        )
        .await
        .map_err(|_| {
            ServiceError::new(
                ServiceErrorKind::TimedOut,
                "systemd service list exceeded its deadline",
            )
        })?
    })
}

pub(crate) fn status(
    identity: &ServiceIdentity,
    deadline: Duration,
) -> Result<ServiceSnapshot, ServiceError> {
    runtime().block_on(async {
        timeout(deadline, status_async(identity))
            .await
            .map_err(|_| {
                ServiceError::new(
                    ServiceErrorKind::TimedOut,
                    "systemd service status exceeded its deadline",
                )
            })?
    })
}

pub(crate) fn mutate(request: &ServiceMutationRequest) -> Result<(), ServiceError> {
    if matches!(
        request.operation,
        ServiceOperation::Bootstrap | ServiceOperation::Bootout
    ) {
        return Err(ServiceError::new(
            ServiceErrorKind::Unsupported,
            "systemd has no identity-safe equivalent of launchd bootstrap/bootout in this provider",
        ));
    }
    runtime().block_on(async {
        timeout(request.deadline, mutate_async(request))
            .await
            .map_err(|_| {
                ServiceError::new(
                    ServiceErrorKind::TimedOut,
                    "systemd lifecycle request exceeded its deadline",
                )
                .after_effect(ServiceRollback::NotNeeded, None)
            })?
    })
}

async fn connection(scope: ServiceScope) -> Result<Connection, ServiceError> {
    let result = match scope {
        ServiceScope::User => Connection::session().await,
        ServiceScope::System => Connection::system().await,
    };
    result.map_err(|error| {
        ServiceError::new(
            ServiceErrorKind::QueryFailed,
            format!("could not connect to systemd D-Bus: {error}"),
        )
    })
}

async fn manager(connection: &Connection) -> Result<Proxy<'_>, ServiceError> {
    Proxy::new(connection, SYSTEMD_DEST, SYSTEMD_PATH, MANAGER_IFACE)
        .await
        .map_err(query_error)
}

async fn list_async(
    scope: ServiceScope,
    max_items: usize,
    match_text: Option<&str>,
) -> Result<ServiceList, ServiceError> {
    let connection = connection(scope).await?;
    let manager = manager(&connection).await?;
    let rows: Vec<UnitRow> = manager.call("ListUnits", &()).await.map_err(query_error)?;
    let matches = |name: &str| {
        name.ends_with(".service")
            && match_text.is_none_or(|needle| name.to_lowercase().contains(&needle.to_lowercase()))
    };
    let visited = rows.iter().filter(|row| matches(&row.0)).count();
    let complete = visited <= max_items;
    let mut services = Vec::with_capacity(visited.min(max_items));
    for row in rows
        .into_iter()
        .filter(|row| matches(&row.0))
        .take(max_items)
    {
        validate_field(&row.0)?;
        validate_field(&row.1)?;
        services.push(snapshot_from_row(scope, row)?);
    }
    Ok(ServiceList {
        services,
        complete,
        visited,
    })
}

async fn status_async(identity: &ServiceIdentity) -> Result<ServiceSnapshot, ServiceError> {
    let connection = connection(identity.scope).await?;
    let manager = manager(&connection).await?;
    let path: OwnedObjectPath = match manager.call("GetUnit", &(identity.name.as_str(),)).await {
        Ok(path) => path,
        Err(error) if is_no_such_unit(&error) => return Ok(missing(identity.clone())),
        Err(error) => return Err(query_error(error)),
    };
    let unit = Proxy::new(&connection, SYSTEMD_DEST, path.as_str(), UNIT_IFACE)
        .await
        .map_err(query_error)?;
    let id: String = unit.get_property("Id").await.map_err(query_error)?;
    if id != identity.name {
        return Err(ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "systemd GetUnit returned a different unit identity",
        ));
    }
    let description: String = unit
        .get_property("Description")
        .await
        .map_err(query_error)?;
    let active: String = unit
        .get_property("ActiveState")
        .await
        .map_err(query_error)?;
    let substate: String = unit.get_property("SubState").await.map_err(query_error)?;
    let fragment: String = unit.get_property("FragmentPath").await.unwrap_or_default();
    validate_field(&description)?;
    validate_field(&active)?;
    validate_field(&substate)?;
    let invocation: Vec<u8> = unit.get_property("InvocationID").await.unwrap_or_default();
    Ok(ServiceSnapshot {
        identity: identity.clone(),
        instance: (!invocation.is_empty()).then(|| ServiceInstanceIdentity {
            provider: "systemd",
            opaque: hex(&invocation),
        }),
        state: map_state(&active),
        substate,
        description,
        definition: (!fragment.is_empty()).then(|| PathBuf::from(fragment)),
    })
}

async fn mutate_async(request: &ServiceMutationRequest) -> Result<(), ServiceError> {
    let connection = connection(ServiceScope::User).await?;
    let manager = manager(&connection).await?;
    let method = match request.operation {
        ServiceOperation::Start => "StartUnit",
        ServiceOperation::Stop => "StopUnit",
        ServiceOperation::Restart => "RestartUnit",
        ServiceOperation::Bootstrap | ServiceOperation::Bootout => unreachable!(),
    };
    let result: Result<OwnedObjectPath, zbus::Error> = manager
        .call(
            method,
            &(request.expected_before.identity.name.as_str(), "replace"),
        )
        .await;
    result.map(|_| ()).map_err(|error| {
        ServiceError::new(
            ServiceErrorKind::MutationFailed,
            format!("systemd {method} failed: {error}"),
        )
        .after_effect(ServiceRollback::NotNeeded, None)
    })
}

fn snapshot_from_row(scope: ServiceScope, row: UnitRow) -> Result<ServiceSnapshot, ServiceError> {
    Ok(ServiceSnapshot {
        identity: ServiceIdentity {
            scope,
            provider: "systemd",
            provider_scope: match scope {
                ServiceScope::User => "user",
                ServiceScope::System => "system",
            }
            .into(),
            name: row.0,
        },
        // ListUnits exposes a transient job identity, not the service's
        // incarnation. Only status publishes systemd's InvocationID.
        instance: None,
        state: map_state(&row.3),
        substate: row.4,
        description: row.1,
        definition: None,
    })
}

fn map_state(active: &str) -> ServiceState {
    match active {
        "active" | "reloading" => ServiceState::Running,
        "activating" => ServiceState::Activating,
        "deactivating" => ServiceState::Deactivating,
        "inactive" => ServiceState::LoadedInactive,
        "failed" => ServiceState::Failed,
        _ => ServiceState::Unknown,
    }
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
    if value.len() > SERVICE_FIELD_MAX_BYTES || value.chars().any(char::is_control) {
        Err(ServiceError::new(
            ServiceErrorKind::InvalidNativeValue,
            "systemd returned an oversized or control-bearing field",
        ))
    } else {
        Ok(())
    }
}

fn query_error(error: zbus::Error) -> ServiceError {
    ServiceError::new(
        ServiceErrorKind::QueryFailed,
        format!("systemd D-Bus query failed: {error}"),
    )
}

fn is_no_such_unit(error: &zbus::Error) -> bool {
    let text = error.to_string();
    text.contains("NoSuchUnit") || text.contains("not loaded")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_systemd_states_without_guessing_unknown_values() {
        assert_eq!(map_state("active"), ServiceState::Running);
        assert_eq!(map_state("failed"), ServiceState::Failed);
        assert_eq!(map_state("future-state"), ServiceState::Unknown);
    }

    #[test]
    fn invocation_identity_hex_is_exact() {
        assert_eq!(hex(&[0, 0x7f, 0x80, 0xff]), "007f80ff");
    }
}
