//! Logical-byte accounting for caller-selected filesystem trees.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Hard ceiling for one bounded tree summary. Callers may choose less.
pub const TREE_SUMMARY_HARD_MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeBucketSummary {
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeSummary {
    pub entries: usize,
    pub files: u64,
    pub bytes: u64,
    pub oldest_modified_ms: Option<u64>,
    pub newest_modified_ms: Option<u64>,
    pub buckets: Vec<TreeBucketSummary>,
}

#[derive(Default)]
struct BucketTotals {
    files: u64,
    bytes: u64,
}

/// Recursively summarize regular files under a real directory.
///
/// The first path component below `root` owns each bucket; files directly in
/// `root` use `(root)`. Symbolic links, Windows reparse points, and other
/// non-regular entries are not followed or counted. The walk fails without a
/// partial result when it would inspect more than `max_entries` directory
/// entries. This is a robustness bound, not a filesystem authorization rule.
pub fn regular_tree_summary_bounded(root: &Path, max_entries: usize) -> io::Result<TreeSummary> {
    if max_entries == 0 || max_entries > TREE_SUMMARY_HARD_MAX_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("max_entries must be in 1..={TREE_SUMMARY_HARD_MAX_ENTRIES}"),
        ));
    }
    let root_metadata = std::fs::symlink_metadata(root)?;
    if !crate::filesystem_entry::metadata_is_real_directory(&root_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tree summary root is not a real directory",
        ));
    }

    let mut pending: Vec<(PathBuf, String, bool)> =
        vec![(root.to_path_buf(), "(root)".to_owned(), true)];
    let mut buckets: BTreeMap<String, BucketTotals> = BTreeMap::new();
    let mut entries = 0usize;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut oldest_modified_ms: Option<u64> = None;
    let mut newest_modified_ms: Option<u64> = None;

    while let Some((directory, bucket, is_root)) = pending.pop() {
        for child in std::fs::read_dir(&directory)? {
            let child = child?;
            entries = entries
                .checked_add(1)
                .ok_or_else(|| io::Error::other("tree summary entry count exceeds usize"))?;
            if entries > max_entries {
                return Err(io::Error::other(format!(
                    "tree summary entry limit {max_entries} exceeded"
                )));
            }
            let metadata = std::fs::symlink_metadata(child.path())?;
            let child_bucket = if is_root && metadata.is_dir() {
                child.file_name().to_string_lossy().into_owned()
            } else if is_root {
                "(root)".to_owned()
            } else {
                bucket.clone()
            };
            if crate::filesystem_entry::metadata_is_real_directory(&metadata) {
                pending.push((child.path(), child_bucket, false));
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            let len = metadata.len();
            files = files
                .checked_add(1)
                .ok_or_else(|| io::Error::other("tree summary file count exceeds u64"))?;
            bytes = bytes
                .checked_add(len)
                .ok_or_else(|| io::Error::other("tree summary byte count exceeds u64"))?;
            let totals = buckets.entry(child_bucket).or_default();
            totals.files = totals
                .files
                .checked_add(1)
                .ok_or_else(|| io::Error::other("tree summary bucket file count exceeds u64"))?;
            totals.bytes = totals
                .bytes
                .checked_add(len)
                .ok_or_else(|| io::Error::other("tree summary bucket bytes exceed u64"))?;
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
            {
                let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
                oldest_modified_ms = Some(oldest_modified_ms.map_or(millis, |old| old.min(millis)));
                newest_modified_ms = Some(newest_modified_ms.map_or(millis, |new| new.max(millis)));
            }
        }
    }

    Ok(TreeSummary {
        entries,
        files,
        bytes,
        oldest_modified_ms,
        newest_modified_ms,
        buckets: buckets
            .into_iter()
            .map(|(name, totals)| TreeBucketSummary {
                name,
                files: totals.files,
                bytes: totals.bytes,
            })
            .collect(),
    })
}

/// Sum logical bytes without traversing host link-like directory entries.
///
/// A missing root contributes zero. Regular directories contribute only their
/// descendants; files, symbolic links, and Windows reparse points contribute
/// their own `symlink_metadata` length. Hard-linked entries are counted once
/// per directory entry. This is neither allocated-byte accounting nor a claim
/// about how many physical bytes deleting the tree would reclaim. The caller
/// must not treat this path-based walk as a defense against concurrent path
/// replacement.
pub fn logical_tree_size(path: &Path) -> io::Result<u64> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if !crate::filesystem_entry::metadata_is_real_directory(&metadata) {
        return Ok(metadata.len());
    }

    let mut total = 0_u64;
    for entry in std::fs::read_dir(path)? {
        total = total
            .checked_add(logical_tree_size(&entry?.path())?)
            .ok_or_else(|| io::Error::other("logical directory size exceeds u64"))?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agenterm-platform-usage-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn sums_nested_files_and_accepts_a_missing_root() {
        let root = fixture("normal");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).expect("create usage fixture");
        fs::write(root.join("a"), b"abc").expect("write usage fixture");
        fs::write(root.join("nested/b"), b"12345").expect("write nested usage fixture");

        assert_eq!(logical_tree_size(&root).expect("measure tree"), 8);
        assert_eq!(
            logical_tree_size(&root.join("missing")).expect("measure missing root"),
            0
        );
        fs::remove_dir_all(root).expect("remove usage fixture");
    }

    #[test]
    fn bounded_summary_groups_first_level_and_refuses_a_partial_answer() {
        let root = fixture("summary");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("debug/deps")).expect("create summary fixture");
        fs::write(root.join("root.bin"), b"x").expect("write root summary fixture");
        fs::write(root.join("debug/debug.bin"), b"abc").expect("write debug summary fixture");
        fs::write(root.join("debug/deps/dependency.bin"), b"12345")
            .expect("write dependency summary fixture");

        let summary = regular_tree_summary_bounded(&root, 5).expect("summarize tree");
        assert_eq!(summary.entries, 5);
        assert_eq!(summary.files, 3);
        assert_eq!(summary.bytes, 9);
        assert_eq!(
            summary.buckets,
            [
                TreeBucketSummary {
                    name: "(root)".to_owned(),
                    files: 1,
                    bytes: 1,
                },
                TreeBucketSummary {
                    name: "debug".to_owned(),
                    files: 2,
                    bytes: 8,
                },
            ]
        );
        let error = regular_tree_summary_bounded(&root, 4).expect_err("limit must refuse");
        assert!(error.to_string().contains("entry limit 4 exceeded"));
        fs::remove_dir_all(root).expect("remove summary fixture");
    }

    #[test]
    fn counts_each_hard_link_directory_entry() {
        let root = fixture("hard-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create hard-link fixture");
        let original = root.join("original");
        fs::write(&original, b"value").expect("write hard-link fixture");
        fs::hard_link(&original, root.join("alias")).expect("create hard-link alias");

        assert_eq!(logical_tree_size(&root).expect("measure hard links"), 10);
        fs::remove_dir_all(root).expect("remove hard-link fixture");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_traverse_symbolic_links() {
        use std::os::unix::fs::symlink;

        let root = fixture("unix-link");
        let outside = fixture("unix-link-outside");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&root).expect("create usage root");
        fs::create_dir_all(&outside).expect("create outside usage fixture");
        fs::write(root.join("local"), b"abc").expect("write local usage file");
        fs::write(outside.join("large"), vec![0_u8; 64 * 1024])
            .expect("write outside usage canary");
        let link = root.join("outside-link");
        symlink(&outside, &link).expect("create outside symlink");
        let link_bytes = fs::symlink_metadata(&link)
            .expect("read link metadata")
            .len();

        assert_eq!(
            logical_tree_size(&root).expect("measure link tree"),
            3 + link_bytes
        );
        fs::remove_dir_all(root).expect("remove usage root");
        fs::remove_dir_all(outside).expect("remove outside usage fixture");
    }

    #[cfg(windows)]
    #[test]
    fn does_not_traverse_directory_reparse_points() {
        let root = fixture("windows-reparse");
        let outside = fixture("windows-reparse-outside");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&root).expect("create usage root");
        fs::create_dir_all(&outside).expect("create outside usage fixture");
        fs::write(root.join("local"), b"abc").expect("write local usage file");
        fs::write(outside.join("large"), vec![0_u8; 64 * 1024])
            .expect("write outside usage canary");
        let junction = root.join("outside-junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink junction fixture");
        assert!(status.success(), "mklink /J fixture failed: {status}");
        let junction_bytes = fs::symlink_metadata(&junction)
            .expect("read junction metadata")
            .len();

        assert_eq!(
            logical_tree_size(&root).expect("measure junction tree"),
            3 + junction_bytes
        );
        fs::remove_dir_all(root).expect("remove usage root");
        fs::remove_dir_all(outside).expect("remove outside usage fixture");
    }
}
