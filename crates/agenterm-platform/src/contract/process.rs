//! OS-neutral process facts and typed failures consumed by facade services.

pub use super::process_observation::ProcessObservation;

/// Maximum native source bytes accepted for one initial-environment snapshot.
pub const PROCESS_ENVIRONMENT_MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    pub id: u32,
    pub parent_id: u32,
    pub executable_name: String,
}

/// One raw, NUL-delimited entry from a process's initial environment block.
///
/// The bytes are deliberately not decoded here: Unix environment names and
/// values need not be UTF-8, and replacing invalid sequences would collapse
/// distinct native entries at the product boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessEnvironmentEntry {
    pub bytes: Vec<u8>,
}

/// A bounded snapshot of the environment block installed by `exec`.
///
/// This is not a view of later `setenv`/`putenv` mutations. `source_bytes`
/// counts the exact native environment bytes consumed, including NUL
/// separators, while `entries` retains each raw field without lossy decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessEnvironmentSnapshot {
    pub entries: Vec<ProcessEnvironmentEntry>,
    pub source_bytes: usize,
}

impl ProcessEnvironmentSnapshot {
    #[allow(dead_code)] // The selected Windows adapter refuses before parsing a Unix block.
    pub(crate) fn from_nul_delimited(bytes: &[u8]) -> Self {
        let source_bytes = bytes.len();
        if bytes.is_empty() {
            return Self {
                entries: Vec::new(),
                source_bytes,
            };
        }
        let mut fields = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
        // A single final NUL terminates the block rather than describing an
        // additional entry. Interior empty fields remain visible so callers
        // can account for malformed native data without the adapter hiding it.
        if bytes.last() == Some(&0) {
            fields.pop();
        }
        Self {
            entries: fields
                .into_iter()
                .map(|field| ProcessEnvironmentEntry {
                    bytes: field.to_vec(),
                })
                .collect(),
            source_bytes,
        }
    }
}

#[allow(dead_code)] // Consumed only by the selected Unix process adapters.
pub(crate) fn transitive_descendant_ids(root_id: u32, processes: &[ProcessInfo]) -> Vec<u32> {
    use std::collections::HashSet;

    let mut seen = HashSet::from([root_id]);
    let mut parents = vec![root_id];
    let mut descendants = Vec::new();
    while let Some(parent_id) = parents.pop() {
        for process in processes {
            if process.parent_id == parent_id && seen.insert(process.id) {
                descendants.push(process.id);
                parents.push(process.id);
            }
        }
    }
    descendants.reverse();
    descendants
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipeProbeToken(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PipeProbeError {
    Closed,
    Failed,
}

#[allow(dead_code)] // A target builds the full three-adapter error contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProcessErrorKind {
    IdOutOfRange,
    NotFound,
    PermissionDenied,
    Unavailable,
    InvalidData,
    Inventory,
    InventoryTooLarge,
    Inspect,
    KillOpen,
    Kill,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessError {
    kind: ProcessErrorKind,
    detail: String,
}

impl ProcessError {
    pub(crate) fn new(kind: ProcessErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> ProcessErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "process {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ProcessError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_error_preserves_typed_kind_and_diagnostic() {
        let error = ProcessError::new(ProcessErrorKind::Unsupported, "adapter unavailable");
        assert_eq!(error.kind, ProcessErrorKind::Unsupported);
        assert_eq!(error.detail, "adapter unavailable");
    }

    #[test]
    fn environment_snapshot_preserves_raw_non_utf8_empty_and_malformed_entries() {
        let raw = b"NAME=value\0EMPTY=\0BAD\0NON_UTF8=\xff\0\0";
        let snapshot = ProcessEnvironmentSnapshot::from_nul_delimited(raw);
        assert_eq!(snapshot.source_bytes, raw.len());
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|entry| entry.bytes.as_slice())
                .collect::<Vec<_>>(),
            vec![
                &b"NAME=value"[..],
                &b"EMPTY="[..],
                &b"BAD"[..],
                &b"NON_UTF8=\xff"[..],
                &b""[..],
            ]
        );
    }

    #[test]
    fn empty_environment_snapshot_has_no_synthetic_entry() {
        let snapshot = ProcessEnvironmentSnapshot::from_nul_delimited(&[]);
        assert_eq!(snapshot.source_bytes, 0);
        assert!(snapshot.entries.is_empty());
    }

    #[test]
    fn descendant_walk_is_transitive_deepest_first_and_cycle_safe() {
        let processes = [
            ProcessInfo {
                id: 20,
                parent_id: 10,
                executable_name: "child".to_owned(),
            },
            ProcessInfo {
                id: 30,
                parent_id: 20,
                executable_name: "grandchild".to_owned(),
            },
            ProcessInfo {
                id: 40,
                parent_id: 99,
                executable_name: "unrelated".to_owned(),
            },
            ProcessInfo {
                id: 10,
                parent_id: 30,
                executable_name: "cycle".to_owned(),
            },
        ];
        assert_eq!(transitive_descendant_ids(10, &processes), vec![30, 20]);
    }
}
