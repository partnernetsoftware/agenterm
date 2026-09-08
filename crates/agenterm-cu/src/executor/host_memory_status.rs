//! Current-host physical memory geometry and availability observation.

use agenterm_platform::host_memory::{HostMemoryError, HostMemoryErrorKind};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn host_memory_status_payload() -> Result<Value, CuError> {
    let (facts, availability) =
        agenterm_platform::host_memory::observed().map_err(memory_error)?;
    Ok(json!({
        "page_size": facts.page_size.get(),
        "allocation_granularity": facts.allocation_granularity.get(),
        "physical_bytes": facts.physical_bytes.get(),
        "available_physical_bytes": availability.available_physical_bytes,
        "availability_semantics": availability.semantics.as_str(),
        "atomic_snapshot": false,
    }))
}

fn memory_error(error: HostMemoryError) -> CuError {
    let code = match error.kind() {
        HostMemoryErrorKind::InvalidValue => "host_memory_invalid",
        HostMemoryErrorKind::Overflow => "host_memory_overflow",
        HostMemoryErrorKind::Query | _ => "host_memory_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_reports_positive_physical_memory_and_bounded_availability() {
        let value = host_memory_status_payload().expect("host memory payload");
        assert!(value["page_size"].as_u64().unwrap_or(0) > 0);
        assert!(value["allocation_granularity"].as_u64().unwrap_or(0) > 0);
        let physical = value["physical_bytes"].as_u64().unwrap_or(0);
        assert!(physical > 0);
        let available = value["available_physical_bytes"].as_u64().unwrap_or(0);
        assert!(available <= physical);
        assert!(
            value["availability_semantics"]
                .as_str()
                .is_some_and(|semantics| !semantics.is_empty())
        );
        assert_eq!(value["atomic_snapshot"], false);
    }
}
