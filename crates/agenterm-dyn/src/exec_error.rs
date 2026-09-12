use thiserror::Error;

/// Failure from the executable-code buffer boundary.
///
/// This error belongs to the retained execution base rather than the legacy
/// list-language evaluator. Keeping it independent lets callers use
/// [`crate::CodeBuffer`] without depending on that evaluator's error taxonomy.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecError {
    /// Mapping, protection, capacity, or entry validation failed.
    #[error("exec base error: {0}")]
    Exec(String),
}
