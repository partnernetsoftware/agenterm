//! Fixtures shared by the executor test modules: scratch audit paths,
//! synthetic tree nodes and pre-authorized executors.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) static NEXT_AUDIT_SCRATCH: AtomicU64 = AtomicU64::new(0);

pub(super) fn audit_scratch(label: &str) -> PathBuf {
    let sequence = NEXT_AUDIT_SCRATCH.fetch_add(1, Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!(
        "agenterm-cu-executor-audit-{label}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).expect("create audit scratch root");
    // Resolve the macOS `/var` temp-root symlink before exercising the
    // production store's fail-closed ancestry check.
    std::fs::canonicalize(scratch)
        .expect("canonicalize audit scratch root")
        .join("audit.jsonl")
}

pub(super) fn remove_audit_scratch(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

pub(super) fn node(name: &str, role: &str, states: &[&str]) -> mechanism::A11yNode {
    node_at("/0/1", name, role, states)
}

pub(super) fn node_at(id: &str, name: &str, role: &str, states: &[&str]) -> mechanism::A11yNode {
    mechanism::A11yNode {
        id: id.into(),
        parent_id: Some("/0".into()),
        role: role.into(),
        name: name.into(),
        states: states.iter().map(|state| (*state).to_owned()).collect(),
        bounds: mechanism::A11yBounds {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
        actions: Vec::new(),
        text: None,
        identifier: None,
    }
}

pub(super) fn actuate_executor() -> Executor {
    Executor::new(Authorization::new(
        [Grant::Observe, Grant::Actuate].into_iter().collect(),
    ))
}

pub(super) fn observe_executor() -> Executor {
    Executor::new(Authorization::new([Grant::Observe].into_iter().collect()))
}
