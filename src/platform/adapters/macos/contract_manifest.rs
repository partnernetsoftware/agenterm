use crate::platform::{
    contract::adapter::AdapterContractDeclaration, CapabilityKind, CapabilityStatus, PlatformKind,
};

pub(crate) const DECLARATION: AdapterContractDeclaration = AdapterContractDeclaration {
    kind: PlatformKind::Macos,
    revision: 3,
    capabilities: CapabilityKind::ALL,
};

pub(crate) fn unsupported_probe() -> CapabilityStatus {
    CapabilityStatus::Unsupported {
        reason: "macos-adapter-capability-unavailable",
    }
}

pub(crate) fn failed_probe() -> CapabilityStatus {
    CapabilityStatus::Failed {
        code: "macos_adapter_failed",
        message: "macOS native operation failed".to_owned(),
    }
}
