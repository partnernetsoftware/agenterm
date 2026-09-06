//! OS-neutral process facts and typed failures consumed by facade services.

pub use super::process_observation::ProcessObservation;

/// Maximum native source bytes accepted for one initial-environment snapshot.
pub const PROCESS_ENVIRONMENT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Maximum bytes accepted from one process cgroup membership file.
pub const PROCESS_CGROUP_MEMBERSHIP_MAX_BYTES: usize = 64 * 1024;
/// Maximum bytes accepted from one cgroup v2 control or accounting file.
pub const PROCESS_CGROUP_FIELD_MAX_BYTES: usize = 1024 * 1024;
/// Maximum controller or counter names accepted from one cgroup snapshot.
pub const PROCESS_CGROUP_MAX_COUNTERS: usize = 256;
/// Maximum device rows accepted from `io.stat`.
pub const PROCESS_CGROUP_MAX_IO_DEVICES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessCgroupLimit {
    Max,
    Value(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupCounter {
    pub name: String,
    pub value: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupCpuMax {
    pub quota: ProcessCgroupLimit,
    pub period_microseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupIoDevice {
    pub major: u32,
    pub minor: u32,
    pub counters: Vec<ProcessCgroupCounter>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessCgroupUnavailableKind {
    NotPresent,
    PermissionDenied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupUnavailableField {
    pub field: &'static str,
    pub kind: ProcessCgroupUnavailableKind,
}

/// One identity-bound, bounded Linux cgroup v2 point-in-time observation.
///
/// `path` preserves the native membership bytes. `directory_device` and
/// `directory_inode` identify the opened cgroup directory used for every
/// point read; they are revalidated before publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupV2Snapshot {
    pub provider: &'static str,
    pub process_id: u32,
    pub start_identity: String,
    pub path: Vec<u8>,
    pub directory_device: u64,
    pub directory_inode: u64,
    pub controllers: Vec<String>,
    pub subtree_control: Vec<String>,
    pub cpu_max: Option<ProcessCgroupCpuMax>,
    pub cpu_weight: Option<u64>,
    pub cpu_stat: Vec<ProcessCgroupCounter>,
    pub memory_current_bytes: Option<u64>,
    pub memory_high_bytes: Option<ProcessCgroupLimit>,
    pub memory_max_bytes: Option<ProcessCgroupLimit>,
    pub memory_swap_current_bytes: Option<u64>,
    pub memory_swap_max_bytes: Option<ProcessCgroupLimit>,
    pub memory_events: Vec<ProcessCgroupCounter>,
    pub pids_current: Option<u64>,
    pub pids_max: Option<ProcessCgroupLimit>,
    pub pids_events: Vec<ProcessCgroupCounter>,
    pub cgroup_events: Vec<ProcessCgroupCounter>,
    pub populated: Option<bool>,
    pub frozen: Option<bool>,
    pub io: Vec<ProcessCgroupIoDevice>,
    pub unavailable: Vec<ProcessCgroupUnavailableField>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProcessCgroupErrorKind {
    IdOutOfRange,
    NotFound,
    PermissionDenied,
    NotApplicable,
    V2Unavailable,
    InventoryTooLarge,
    InvalidData,
    IdentityChanged,
    MembershipChanged,
    DirectoryChanged,
    Inspect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCgroupError {
    kind: ProcessCgroupErrorKind,
    detail: String,
}

impl ProcessCgroupError {
    pub(crate) fn new(kind: ProcessCgroupErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> ProcessCgroupErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for ProcessCgroupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "process cgroup {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ProcessCgroupError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    pub id: u32,
    pub parent_id: u32,
    pub executable_name: String,
}

/// One bounded native inventory with scan completeness kept separate from
/// caller-side result pagination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInspection<T> {
    pub items: Vec<T>,
    pub visited_count: usize,
    pub read_errors: usize,
    pub truncated_scan: bool,
}

/// One process-local descriptor. Native path bytes are retained without lossy
/// decoding so the product boundary can refuse or encode them deliberately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessFileDescriptor {
    pub descriptor: i32,
    pub kind: String,
    pub target: Option<Vec<u8>>,
    pub open_flags: Option<u32>,
    pub status_flags: Option<u32>,
    pub offset_bytes: Option<i64>,
    pub file_type: Option<u32>,
    pub guard_flags: Option<u32>,
}

/// One virtual-memory region. Addresses and sizes stay integers inside the
/// mechanism layer; JSON callers may render them as strings without precision
/// loss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessMemoryRegion {
    pub start_address: u64,
    pub size_bytes: u64,
    pub offset_bytes: u64,
    pub permissions: String,
    pub max_permissions: Option<String>,
    pub sharing: String,
    pub path: Option<Vec<u8>>,
    pub device: Option<String>,
    pub inode: Option<u64>,
    pub flags: Option<u32>,
    pub user_tag: Option<u32>,
    pub depth: Option<u32>,
    pub resident_pages: Option<u32>,
    pub private_resident_pages: Option<u32>,
    pub shared_resident_pages: Option<u32>,
    pub swapped_pages: Option<u32>,
    pub dirtied_pages: Option<u32>,
}

/// One native thread snapshot. Time counters deliberately remain raw and name
/// bytes remain lossless; `time_unit` states how to interpret the counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessThreadInfo {
    pub id: u64,
    pub name: Option<Vec<u8>>,
    pub state: String,
    pub state_raw: String,
    pub user_time_raw: u64,
    pub system_time_raw: u64,
    pub time_unit: &'static str,
    pub cpu_usage_tenths_percent: Option<i32>,
    pub policy: Option<i32>,
    pub flags: Option<i32>,
    pub sleep_seconds: Option<i32>,
    pub current_priority: Option<i32>,
    pub priority: Option<i32>,
    pub max_priority: Option<i32>,
    pub nice: Option<i32>,
    pub processor: Option<i32>,
}

/// One process-owned native socket. Endpoint bytes stay undecoded because a
/// Unix-domain socket path is not required to be UTF-8.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSocketInfo {
    pub descriptor: i32,
    pub family: String,
    pub protocol: String,
    pub local: Option<Vec<u8>>,
    pub remote: Option<Vec<u8>>,
    pub endpoint: Vec<u8>,
    pub state: Option<String>,
    pub inode: Option<u64>,
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
    fn process_cgroup_error_preserves_typed_kind_and_diagnostic() {
        let error = ProcessCgroupError::new(
            ProcessCgroupErrorKind::MembershipChanged,
            "membership changed",
        );
        assert_eq!(error.kind(), ProcessCgroupErrorKind::MembershipChanged);
        assert_eq!(error.detail(), "membership changed");
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
