//! Linux monospace discovery with explicit fontconfig honesty.

use serde_json::Value;

use crate::reply::CuError;

pub(super) fn font_discovery_payload() -> Result<Value, CuError> {
    #[cfg(target_os = "linux")]
    {
        let report = agenterm_platform::font_discovery::discover();
        return Ok(serde_json::json!({
            "source": report.source,
            "primary_family": report.primary_family,
            "primary_path": report.primary_path,
            "fontconfig": match (report.fontconfig_family, report.fontconfig_file) {
                (Some(family), Some(file)) => serde_json::json!({ "family": family, "file": file }),
                _ => Value::Null,
            },
            "alternatives": report.alternatives,
        }));
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(CuError::new(
            "font_discovery_unsupported",
            "monospace discovery is wired on Linux hosts only",
        ))
    }
}
