//! Product-neutral native service lifecycle facts and failure receipts.

use std::{path::PathBuf, time::Duration};

pub const SERVICE_MAX_ITEMS: usize = 5_000;
pub const SERVICE_FIELD_MAX_BYTES: usize = 1_024;
pub const SERVICE_OUTPUT_MAX_BYTES: usize = 1024 * 1024;
pub const SERVICE_DEFAULT_DEADLINE: Duration = Duration::from_secs(5);
pub const SERVICE_MAX_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ServiceScope {
    User,
    System,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ServiceIdentity {
    pub scope: ServiceScope,
    pub provider: &'static str,
    /// Stable native authority domain (for example a launchd bootstrap domain).
    pub provider_scope: String,
    /// Canonical provider name: a launchd label or systemd unit name.
    pub name: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ServiceInstanceIdentity {
    pub provider: &'static str,
    /// Provider-owned incarnation key. It is never synthesized when absent.
    pub opaque: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceState {
    Missing,
    LoadedInactive,
    Activating,
    Running,
    Deactivating,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceSnapshot {
    pub identity: ServiceIdentity,
    pub instance: Option<ServiceInstanceIdentity>,
    pub state: ServiceState,
    pub substate: String,
    pub description: String,
    pub definition: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceListBudget {
    pub max_items: usize,
    pub deadline: Duration,
    /// Optional provider-neutral case-insensitive substring filter applied
    /// before truncation.
    pub match_text: Option<String>,
}

impl Default for ServiceListBudget {
    fn default() -> Self {
        Self {
            max_items: SERVICE_MAX_ITEMS,
            deadline: SERVICE_DEFAULT_DEADLINE,
            match_text: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceList {
    pub services: Vec<ServiceSnapshot>,
    pub complete: bool,
    pub visited: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceOperation {
    Start,
    Stop,
    Restart,
    Bootstrap,
    Bootout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceMutationRequest {
    pub operation: ServiceOperation,
    pub expected_before: ServiceSnapshot,
    /// Native definition used only by operations whose provider requires it.
    pub definition: Option<PathBuf>,
    pub deadline: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceMutationReceipt {
    pub operation: ServiceOperation,
    pub before: ServiceSnapshot,
    pub after: ServiceSnapshot,
    pub performed: bool,
    pub verified: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEffect {
    NotPerformed,
    PossiblyApplied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceRollback {
    NotNeeded,
    Verified,
    Failed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServiceErrorKind {
    Unsupported,
    RequiresPrivilege,
    InvalidRequest,
    InvalidNativeValue,
    InventoryTooLarge,
    TimedOut,
    QueryFailed,
    StateChanged,
    MutationFailed,
    VerificationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceError {
    kind: ServiceErrorKind,
    detail: String,
    effect: ServiceEffect,
    rollback: ServiceRollback,
    observed: Option<Box<ServiceSnapshot>>,
}

impl ServiceError {
    pub(crate) fn new(kind: ServiceErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            effect: ServiceEffect::NotPerformed,
            rollback: ServiceRollback::NotNeeded,
            observed: None,
        }
    }

    pub(crate) fn after_effect(
        mut self,
        rollback: ServiceRollback,
        observed: Option<ServiceSnapshot>,
    ) -> Self {
        self.effect = ServiceEffect::PossiblyApplied;
        self.rollback = rollback;
        self.observed = observed.map(Box::new);
        self
    }

    pub const fn kind(&self) -> ServiceErrorKind {
        self.kind
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
    pub const fn effect(&self) -> ServiceEffect {
        self.effect
    }
    pub const fn rollback(&self) -> ServiceRollback {
        self.rollback
    }
    pub fn observed(&self) -> Option<&ServiceSnapshot> {
        self.observed.as_deref()
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "service {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ServiceError {}
