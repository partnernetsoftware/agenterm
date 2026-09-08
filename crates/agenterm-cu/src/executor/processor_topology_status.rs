//! Current-host processor and NUMA topology observation.

use agenterm_platform::processor_topology::{
    ProcessorTopologyError, ProcessorTopologyErrorKind,
};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn processor_topology_status_payload() -> Result<Value, CuError> {
    let facts = agenterm_platform::processor_topology::facts().map_err(topology_error)?;
    Ok(json!({
        "online_logical_processors": facts.system_logical_processors.get(),
        "physical_cores": facts.physical_cores.map(|value| value.get()),
        "packages": facts.packages.map(|value| value.get()),
        "numa_nodes": facts.numa_nodes.map(|value| value.get()),
        "processor_groups": facts.processor_groups.map(|value| value.get()),
        "uniform_threads_per_core": facts.uniform_threads_per_core().map(|value| value.get()),
        "atomic_snapshot": false,
    }))
}

fn topology_error(error: ProcessorTopologyError) -> CuError {
    let code = match error.kind() {
        ProcessorTopologyErrorKind::InvalidValue => "host_processor_topology_invalid",
        ProcessorTopologyErrorKind::MalformedNativeData => "host_processor_topology_malformed",
        ProcessorTopologyErrorKind::Query | _ => "host_processor_topology_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_reports_positive_online_logical_processors() {
        let value = processor_topology_status_payload().expect("processor topology payload");
        assert!(value["online_logical_processors"].as_u64().unwrap_or(0) > 0);
        assert_eq!(value["atomic_snapshot"], false);
    }
}
