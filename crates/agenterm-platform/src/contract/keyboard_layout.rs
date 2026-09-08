//! Platform-neutral keyboard layout observation contract.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardLayoutObservation {
    pub provider: String,
    pub rules: String,
    pub model: String,
    pub layout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variant: String,
    /// Stable XKB layout identifier (`layout` or `layout+variant`).
    pub id: String,
    /// Human-oriented layout name for receipts and smoke read-back.
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardLayoutObserveUnsupported {
    pub reason: String,
    pub required_mechanism: String,
    pub alternatives: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyboardLayoutObserveResult {
    Ok(KeyboardLayoutObservation),
    Unsupported(KeyboardLayoutObserveUnsupported),
}
