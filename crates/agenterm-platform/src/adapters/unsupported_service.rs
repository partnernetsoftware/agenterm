use std::time::Duration;

use crate::service::{
    ServiceError, ServiceErrorKind, ServiceIdentity, ServiceList, ServiceListBudget,
    ServiceMutationRequest, ServiceScope, ServiceSnapshot,
};

pub(crate) fn identity(
    _scope: ServiceScope,
    _name: &str,
    _deadline: Duration,
) -> Result<ServiceIdentity, ServiceError> {
    Err(unsupported())
}

pub(crate) fn definition_name(
    _path: &std::path::Path,
    _deadline: Duration,
) -> Result<String, ServiceError> {
    Err(unsupported())
}

pub(crate) fn validate_identity(_identity: &ServiceIdentity) -> Result<(), ServiceError> {
    Err(unsupported())
}

pub(crate) fn list(
    _scope: ServiceScope,
    _budget: ServiceListBudget,
) -> Result<ServiceList, ServiceError> {
    Err(unsupported())
}

pub(crate) fn status(
    _identity: &ServiceIdentity,
    _deadline: std::time::Duration,
) -> Result<ServiceSnapshot, ServiceError> {
    Err(unsupported())
}

pub(crate) fn mutate(_request: &ServiceMutationRequest) -> Result<(), ServiceError> {
    Err(unsupported())
}

fn unsupported() -> ServiceError {
    ServiceError::new(
        ServiceErrorKind::Unsupported,
        "native service lifecycle is unsupported on this platform",
    )
}
