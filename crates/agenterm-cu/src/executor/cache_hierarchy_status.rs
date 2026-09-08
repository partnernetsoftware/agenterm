//! Current-host CPU cache hierarchy observation.

use agenterm_platform::cache_hierarchy::{
    CacheHierarchyError, CacheHierarchyErrorKind,
};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn cache_hierarchy_status_payload() -> Result<Value, CuError> {
    let facts = agenterm_platform::cache_hierarchy::facts().map_err(cache_hierarchy_error)?;
    let geometries = facts
        .geometries
        .iter()
        .map(|cache| {
            json!({
                "level": cache.level.get(),
                "kind": cache.kind.as_str(),
                "size_bytes": cache.size_bytes.get(),
                "line_bytes": cache.line_bytes.get(),
                "instances": cache.instances.map(|value| value.get()),
                "shared_logical_processors": cache.shared_logical_processors.map(|value| value.get()),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "geometries": geometries,
        "max_data_line_bytes": facts.max_data_line_bytes().map(|value| value.get()),
        "atomic_snapshot": false,
    }))
}

fn cache_hierarchy_error(error: CacheHierarchyError) -> CuError {
    let code = match error.kind() {
        CacheHierarchyErrorKind::InvalidValue => "host_cache_hierarchy_invalid",
        CacheHierarchyErrorKind::MalformedNativeData => "host_cache_hierarchy_malformed",
        CacheHierarchyErrorKind::Unavailable => "host_cache_hierarchy_unavailable",
        CacheHierarchyErrorKind::Query | _ => "host_cache_hierarchy_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_reports_non_empty_geometries_on_linux() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let value = cache_hierarchy_status_payload().expect("cache hierarchy payload");
        assert!(value["geometries"].as_array().is_some_and(|rows| !rows.is_empty()));
        assert_eq!(value["atomic_snapshot"], false);
    }
}
