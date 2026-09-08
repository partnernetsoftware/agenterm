//! Typed machine-readable replies.

use serde::{Deserialize, Serialize};

use crate::command::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CuError {
    pub code: String,
    pub message: String,
    /// Present on `a11y_node_ambiguous` so callers can see how many showing
    /// nodes matched without parsing the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Structured effect/recovery context for failures that may follow an
    /// external side effect. Callers must not infer this state from prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl CuError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            count: None,
            detail: None,
        }
    }

    pub fn with_count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CuReply {
    pub ok: bool,
    pub target: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CuError>,
}

impl CuReply {
    pub fn ok(command: &Command, data: serde_json::Value) -> Self {
        Self {
            ok: true,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: Some(data),
            error: None,
        }
    }

    pub fn err(command: &Command, error: CuError) -> Self {
        Self {
            ok: false,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: None,
            error: Some(error),
        }
    }

    /// Process exit code for this reply. `wait` keeps `ok:true` with
    /// `data.met=false` but still exits non-zero so shell gates work.
    pub fn exit_code(&self) -> i32 {
        if self.ok {
            if self.command == "wait"
                && self
                    .data
                    .as_ref()
                    .is_some_and(|data| data.get("met") == Some(&serde_json::Value::Bool(false)))
            {
                return 1;
            }
            0
        } else if self
            .error
            .as_ref()
            .is_some_and(|error| error.code == "usage")
        {
            2
        } else {
            1
        }
    }
}
