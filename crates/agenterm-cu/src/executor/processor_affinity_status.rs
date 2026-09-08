//! Current-process or chosen-PID processor-affinity observation.

use agenterm_platform::processor_affinity::{ProcessorAffinityError, ProcessorAffinityErrorKind};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn processor_affinity_status_payload(pid: Option<u32>) -> Result<Value, CuError> {
    let resolved_pid = pid.unwrap_or_else(std::process::id);
    let facts = match pid {
        None => agenterm_platform::processor_affinity::current_process(),
        Some(pid) => agenterm_platform::processor_affinity::process(pid),
    }
    .map_err(affinity_error)?;
    Ok(json!({
        "pid": resolved_pid,
        "processors": facts
            .processors()
            .iter()
            .map(|processor| json!({
                "group": processor.group,
                "index": processor.index,
            }))
            .collect::<Vec<_>>(),
        "count": facts.count().get(),
        "semantics": facts.semantics().as_str(),
        "atomic_snapshot": false,
    }))
}

fn affinity_error(error: ProcessorAffinityError) -> CuError {
    if error.kind() == ProcessorAffinityErrorKind::Unsupported {
        let os = if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "unknown"
        };
        return CuError::new("processor_affinity_unsupported", error.to_string()).with_detail(
            json!({
                "os": os,
                "required_os": "linux-or-windows",
                "mechanism": if cfg!(target_os = "linux") {
                    "sched_getaffinity"
                } else if cfg!(windows) {
                    "GetProcessAffinityMask"
                } else {
                    "none"
                },
                "alternatives": [
                    "taskset -p PID",
                    "grep Cpus_allowed /proc/PID/status",
                ],
            }),
        );
    }
    let code = match error.kind() {
        ProcessorAffinityErrorKind::InvalidValue => "processor_affinity_invalid",
        ProcessorAffinityErrorKind::MalformedNativeData => "processor_affinity_malformed",
        ProcessorAffinityErrorKind::Query | _ => "processor_affinity_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_reports_nonempty_scheduler_allowed_set_for_current_process() {
        let value = processor_affinity_status_payload(None).unwrap_or_else(|error| {
            assert_eq!(error.code, "processor_affinity_unsupported");
            json!({})
        });
        if value.is_object() && !value.as_object().unwrap().is_empty() {
            assert!(value["count"].as_u64().unwrap_or(0) > 0);
            assert_eq!(value["semantics"], "scheduler-allowed");
            assert_eq!(value["atomic_snapshot"], false);
        }
    }
}
