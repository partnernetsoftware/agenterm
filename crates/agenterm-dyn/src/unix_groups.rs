//! Bounded, pointer-free snapshots of a Unix process's supplementary groups.

use std::fmt;

/// Maximum supplementary-group identifiers accepted from the host.
pub const MAX_SUPPLEMENTARY_GROUPS: usize = 65_536;

#[cfg(any(unix, test))]
const MAX_SNAPSHOT_ATTEMPTS: usize = 3;

/// Failure to acquire a complete supplementary-group snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplementaryGroupsError {
    /// Supplementary groups are available only on Unix hosts.
    Unsupported,
    /// `getgroups` failed with the captured OS error code.
    Os(i32),
    /// The host reported more groups than the public allocation bound.
    TooManyGroups { reported: usize, limit: usize },
    /// The group list kept growing between the count and fetch calls.
    ChangedDuringSnapshot { attempts: usize },
}

impl fmt::Display for SupplementaryGroupsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("getgroups is unsupported on this host"),
            Self::Os(code) => write!(formatter, "getgroups failed with OS error {code}"),
            Self::TooManyGroups { reported, limit } => write!(
                formatter,
                "getgroups reported {reported} groups, exceeding the limit of {limit}"
            ),
            Self::ChangedDuringSnapshot { attempts } => write!(
                formatter,
                "the supplementary group list changed during {attempts} snapshot attempts"
            ),
        }
    }
}

impl std::error::Error for SupplementaryGroupsError {}

/// One complete, pointer-free snapshot of supplementary group identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplementaryGroups {
    groups: Vec<u32>,
}

impl SupplementaryGroups {
    /// Acquire the current process's supplementary groups.
    #[cfg(unix)]
    pub fn acquire() -> Result<Self, SupplementaryGroupsError> {
        snapshot_with(&SystemGroupSource).map(|groups| Self { groups })
    }

    /// Return an honest typed failure on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn acquire() -> Result<Self, SupplementaryGroupsError> {
        Err(SupplementaryGroupsError::Unsupported)
    }

    /// Borrow the copied native group identifiers.
    pub fn as_slice(&self) -> &[u32] {
        &self.groups
    }

    /// Consume the snapshot and return its pointer-free storage.
    pub fn into_vec(self) -> Vec<u32> {
        self.groups
    }
}

#[cfg(any(unix, test))]
trait GroupSource {
    fn count(&self) -> Result<usize, i32>;
    fn fetch(&self, groups: &mut [u32]) -> Result<usize, i32>;
    fn changed_error(&self, code: i32) -> bool;
}

#[cfg(any(unix, test))]
fn snapshot_with(source: &impl GroupSource) -> Result<Vec<u32>, SupplementaryGroupsError> {
    for attempt in 1..=MAX_SNAPSHOT_ATTEMPTS {
        let count = source.count().map_err(SupplementaryGroupsError::Os)?;
        if count > MAX_SUPPLEMENTARY_GROUPS {
            return Err(SupplementaryGroupsError::TooManyGroups {
                reported: count,
                limit: MAX_SUPPLEMENTARY_GROUPS,
            });
        }

        let mut groups = vec![0; count];
        match source.fetch(&mut groups) {
            Ok(written) if written <= groups.len() => {
                groups.truncate(written);
                return Ok(groups);
            }
            Ok(written) => {
                return Err(SupplementaryGroupsError::TooManyGroups {
                    reported: written,
                    limit: groups.len(),
                });
            }
            Err(code) if source.changed_error(code) => {
                if attempt == MAX_SNAPSHOT_ATTEMPTS {
                    return Err(SupplementaryGroupsError::ChangedDuringSnapshot {
                        attempts: MAX_SNAPSHOT_ATTEMPTS,
                    });
                }
            }
            Err(code) => return Err(SupplementaryGroupsError::Os(code)),
        }
    }
    unreachable!("the bounded snapshot loop always returns")
}

#[cfg(unix)]
struct SystemGroupSource;

#[cfg(unix)]
impl GroupSource for SystemGroupSource {
    fn count(&self) -> Result<usize, i32> {
        // SAFETY: a zero capacity permits a null output pointer and only queries
        // the number of supplementary groups.
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        native_count(count)
    }

    fn fetch(&self, groups: &mut [u32]) -> Result<usize, i32> {
        let capacity = libc::c_int::try_from(groups.len()).map_err(|_| libc::EINVAL)?;
        // SAFETY: `groups` owns writable storage for `capacity` gid_t values;
        // on supported Unix targets libc::gid_t is u32.
        let written = unsafe { libc::getgroups(capacity, groups.as_mut_ptr().cast()) };
        native_count(written)
    }

    fn changed_error(&self, code: i32) -> bool {
        code == libc::EINVAL
    }
}

#[cfg(unix)]
fn native_count(value: libc::c_int) -> Result<usize, i32> {
    if value < 0 {
        Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(value))
    } else {
        usize::try_from(value).map_err(|_| libc::EOVERFLOW)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::{
        GroupSource, MAX_SNAPSHOT_ATTEMPTS, MAX_SUPPLEMENTARY_GROUPS, SupplementaryGroupsError,
        snapshot_with,
    };

    struct ScriptedSource {
        counts: RefCell<VecDeque<Result<usize, i32>>>,
        fetches: RefCell<VecDeque<Result<Vec<u32>, i32>>>,
        changed_code: i32,
    }

    impl GroupSource for ScriptedSource {
        fn count(&self) -> Result<usize, i32> {
            self.counts.borrow_mut().pop_front().expect("count step")
        }

        fn fetch(&self, groups: &mut [u32]) -> Result<usize, i32> {
            match self.fetches.borrow_mut().pop_front().expect("fetch step") {
                Ok(values) => {
                    let written = values.len();
                    for (slot, value) in groups.iter_mut().zip(values) {
                        *slot = value;
                    }
                    Ok(written)
                }
                Err(code) => Err(code),
            }
        }

        fn changed_error(&self, code: i32) -> bool {
            code == self.changed_code
        }
    }

    #[test]
    fn shrinking_snapshot_returns_only_the_complete_written_prefix() {
        let source = ScriptedSource {
            counts: RefCell::new(VecDeque::from([Ok(4)])),
            fetches: RefCell::new(VecDeque::from([Ok(vec![11, 17])])),
            changed_code: 22,
        };
        assert_eq!(snapshot_with(&source), Ok(vec![11, 17]));
    }

    #[test]
    fn growth_requeries_and_returns_the_later_complete_snapshot() {
        let source = ScriptedSource {
            counts: RefCell::new(VecDeque::from([Ok(1), Ok(3)])),
            fetches: RefCell::new(VecDeque::from([Err(22), Ok(vec![3, 5, 8])])),
            changed_code: 22,
        };
        assert_eq!(snapshot_with(&source), Ok(vec![3, 5, 8]));
    }

    #[test]
    fn allocation_bound_is_checked_before_fetch() {
        let source = ScriptedSource {
            counts: RefCell::new(VecDeque::from([Ok(MAX_SUPPLEMENTARY_GROUPS + 1)])),
            fetches: RefCell::new(VecDeque::new()),
            changed_code: 22,
        };
        assert_eq!(
            snapshot_with(&source),
            Err(SupplementaryGroupsError::TooManyGroups {
                reported: MAX_SUPPLEMENTARY_GROUPS + 1,
                limit: MAX_SUPPLEMENTARY_GROUPS,
            })
        );
    }

    #[test]
    fn repeated_growth_fails_without_returning_partial_groups() {
        let source = ScriptedSource {
            counts: RefCell::new(VecDeque::from(vec![Ok(1); MAX_SNAPSHOT_ATTEMPTS])),
            fetches: RefCell::new(VecDeque::from(vec![Err(22); MAX_SNAPSHOT_ATTEMPTS])),
            changed_code: 22,
        };
        assert_eq!(
            snapshot_with(&source),
            Err(SupplementaryGroupsError::ChangedDuringSnapshot {
                attempts: MAX_SNAPSHOT_ATTEMPTS,
            })
        );
    }

    #[test]
    fn non_growth_os_error_is_preserved() {
        let source = ScriptedSource {
            counts: RefCell::new(VecDeque::from([Ok(1)])),
            fetches: RefCell::new(VecDeque::from([Err(5)])),
            changed_code: 22,
        };
        assert_eq!(snapshot_with(&source), Err(SupplementaryGroupsError::Os(5)));
    }
}
