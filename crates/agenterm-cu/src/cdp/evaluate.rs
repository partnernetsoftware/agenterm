//! `page-js`: `Runtime.evaluate` on one page target. The expression runs
//! in the browser's own isolate, never in this process (no MAIN-world
//! Function constructor here). A background tab is evaluated in place.

use serde_json::{Value, json};

use super::targets::{TargetSelector, connect_target};
use super::ws::{Session, Transport};
use super::{CdpError, MAX_EXPRESSION_BYTES, MAX_RESULT_BYTES, backend};

/// Evaluate `expression` through CDP on `127.0.0.1:port`, on the page
/// target `selector` names (an empty selector keeps the first page).
pub fn evaluate(port: u16, expression: &str, selector: &TargetSelector) -> Result<Value, CdpError> {
    if expression.is_empty() {
        return Err(CdpError::typed(
            "invalid_input",
            "page-js requires --expression EXPR",
        ));
    }
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(CdpError::typed(
            "invalid_input",
            format!("page-js --expression must be 1..={MAX_EXPRESSION_BYTES} bytes"),
        ));
    }
    let (target, mut session) = connect_target(port, selector)?;
    let value = evaluate_on(&mut session, expression)?;
    Ok(json!({
        "backend": backend(),
        "port": port,
        "via": "Runtime.evaluate",
        "target": target.identity_json(),
        "selector": selector.json(),
        "focus_changed": false,
        "value": value,
    }))
}

/// One `Runtime.evaluate` (by value, promises not awaited) whose reply is
/// bounded by the `page-js` 64 KiB contract.
pub fn evaluate_on<T: Transport>(
    session: &mut Session<T>,
    expression: &str,
) -> Result<Value, CdpError> {
    let before = session.largest_message;
    let result = session
        .call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": false,
            }),
        )
        .map_err(|error| {
            if error.code == "cdp_method_failed" {
                let message = format!(
                    "CDP Runtime.evaluate error: {}",
                    error.detail["cdp_message"].as_str().unwrap_or("unknown")
                );
                error.recode("unsupported", message)
            } else {
                error
            }
        })?;
    if session.largest_message > MAX_RESULT_BYTES && session.largest_message > before {
        return Err(CdpError::typed(
            "unsupported",
            "CDP Runtime.evaluate result exceeds 64KiB",
        ));
    }
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(CdpError::typed(
            "unsupported",
            format!(
                "CDP Runtime.evaluate threw: {}",
                exception["exception"]["description"]
                    .as_str()
                    .or_else(|| exception["text"].as_str())
                    .unwrap_or("exception")
            ),
        )
        .with_detail(json!({ "exception": exception })));
    }
    Ok(result["result"]["value"].clone())
}

#[cfg(test)]
mod tests {
    use super::super::ws::fake;
    use super::*;

    #[test]
    fn missing_listener_is_typed_without_main_world() {
        let err = evaluate(1, "1+1", &TargetSelector::default()).expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert_eq!(err.detail["backend"], "debugger-runtime-evaluate");
        assert!(err.message.contains("remote-debugging-port"));
        assert_eq!(
            evaluate(1, "", &TargetSelector::default())
                .expect_err("empty")
                .code,
            "invalid_input"
        );
        let long = "x".repeat(MAX_EXPRESSION_BYTES + 1);
        assert_eq!(
            evaluate(1, &long, &TargetSelector::default())
                .expect_err("long")
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn evaluate_on_returns_the_value_and_types_exceptions() {
        let mut session = fake::session(|method, params| {
            assert_eq!(method, "Runtime.evaluate");
            assert_eq!(params["returnByValue"], true);
            match params["expression"].as_str() {
                Some("document.title") => {
                    Ok(json!({ "result": { "type": "string", "value": "B" } }))
                }
                Some("boom") => Ok(json!({
                    "result": { "type": "object", "subtype": "error" },
                    "exceptionDetails": { "text": "Uncaught", "exception": { "description": "ReferenceError: boom" } }
                })),
                _ => Err("Cannot find context".into()),
            }
        });
        assert_eq!(
            evaluate_on(&mut session, "document.title").expect("value"),
            "B"
        );
        let err = evaluate_on(&mut session, "boom").expect_err("thrown");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("ReferenceError"));
        let err = evaluate_on(&mut session, "other").expect_err("method error");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("Cannot find context"));
    }
}
