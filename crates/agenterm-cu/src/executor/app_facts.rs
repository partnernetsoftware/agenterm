//! Product projection for native application facts.

use super::*;

pub(super) fn app_facts_payload(
    selector: &str,
    signing: bool,
    verify: bool,
    entitlements: bool,
) -> Result<serde_json::Value, CuError> {
    let facts = agenterm_platform::app_facts::query(
        selector,
        agenterm_platform::app_facts::AppFactsOptions {
            signing,
            verify,
            entitlements,
        },
    )
    .map_err(app_facts_error)?;
    serde_json::to_value(facts).map_err(|_| {
        CuError::new(
            "app_facts_serialization_failed",
            "application facts could not be serialized",
        )
    })
}

fn app_facts_error(error: agenterm_platform::app_facts::AppFactsError) -> CuError {
    CuError::new(error.code(), error.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_error_codes_survive_the_product_boundary() {
        let error = agenterm_platform::app_facts::AppFactsError::new(
            agenterm_platform::app_facts::AppFactsErrorKind::Ambiguous,
            "app_facts_ambiguous",
            "more than one app matched",
        );
        let projected = app_facts_error(error);
        assert_eq!(projected.code, "app_facts_ambiguous");
        assert_eq!(projected.message, "more than one app matched");
    }
}
