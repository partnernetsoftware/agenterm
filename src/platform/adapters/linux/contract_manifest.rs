use crate::platform::{
    contract::adapter::AdapterContractDeclaration, CapabilityKind, CapabilityStatus, PlatformKind,
};

pub(crate) const DECLARATION: AdapterContractDeclaration = AdapterContractDeclaration {
    kind: PlatformKind::Linux,
    revision: 3,
    capabilities: CapabilityKind::ALL,
};

pub(crate) fn unsupported_probe() -> CapabilityStatus {
    CapabilityStatus::Unsupported {
        reason: "linux-adapter-capability-unavailable",
    }
}

pub(crate) fn failed_probe() -> CapabilityStatus {
    CapabilityStatus::Failed {
        code: "linux_adapter_failed",
        message: "Linux native operation failed".to_owned(),
    }
}
