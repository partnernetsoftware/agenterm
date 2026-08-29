//! `scripts/qjs/lib/path.qjs` must answer what rh's `std::path` answers.
//!
//! rh's host implements `std::path::join` as `PathBuf::join` and
//! `std::path::parent` as `Path::parent().unwrap_or("")` -- nothing more
//! (`crates/agenterm-rh/src/host_std.rs`). So the oracle here is not a table
//! someone typed: every expectation is computed by `std::path` in this test,
//! and the library is asked the same questions through the **real CLI**,
//! importing the file from `scripts/qjs/lib/` -- never a copy -- the way a
//! migrated script will. Same pattern as `script_entry_extension_routing.rs`:
//! spawn `agenterm cli script run`, `AGENTERM_SCRIPT_BACKEND` unset, read what
//! came out.
//!
//! The entry lives in a temp dir and `--project-root` points at
//! `scripts/qjs`, which is the documented way for a script to import a
//! library that is not beside it.
//!
//! Unix only, because the semantics being matched are Rust's Unix ones and
//! the library says so.

#![cfg(all(unix, feature = "script-qjswasm"))]

use std::path::{Path, PathBuf};
use std::process::Command;

const AGENTERM_BIN: &str = env!("CARGO_BIN_EXE_agenterm");

/// The paths `parent` is asked about. The three the milestone named --
/// nested, root, single -- plus the shapes Rust's `Components` treats
/// specially: a trailing separator, a doubled one, a `.` in the middle and at
/// the front, `..`, and the empty string.
const PARENT_CASES: &[&str] = &[
    "a/b/c", "/a/b", "/", "a", "a/b/", "/a", "//a", "///a/b", "./a", "a/./b", "a/.", ".", "",
    "a/..", "./.", "/.", "a//", "a//b/c",
];

/// `(base, tail)` pairs for `join`: plain, absolute tail, trailing slash on
/// the base, empty base, empty tail.
const JOIN_CASES: &[(&str, &str)] = &[
    ("a", "b"),
    ("a/b", "/c/d"),
    ("a/", "b"),
    ("/a/", "b"),
    ("", "b"),
    ("a", ""),
    ("/", "b"),
];

fn expected_parent(p: &str) -> String {
    Path::new(p)
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default()
}

fn expected_join(base: &str, tail: &str) -> String {
    PathBuf::from(base).join(tail).display().to_string()
}

fn js_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One script that asks every question and returns the answers as
/// `<answer>` cells, so an empty answer is still visible.
fn probe_script() -> String {
    let mut body = String::from("import * as path from \"lib/path\";\nlet out = \"\";\n");
    for case in PARENT_CASES {
        body.push_str(&format!(
            "out = out + \"<\" + path.parent({}) + \">\";\n",
            js_string(case)
        ));
    }
    body.push_str("out = out + \"|\";\n");
    for (base, tail) in JOIN_CASES {
        body.push_str(&format!(
            "out = out + \"<\" + path.join({}, {}) + \">\";\n",
            js_string(base),
            js_string(tail)
        ));
    }
    body.push_str("return out;\n");
    body
}

fn expected_output() -> String {
    let parents: String = PARENT_CASES
        .iter()
        .map(|case| format!("<{}>", expected_parent(case)))
        .collect();
    let joins: String = JOIN_CASES
        .iter()
        .map(|(base, tail)| format!("<{}>", expected_join(base, tail)))
        .collect();
    format!("{parents}|{joins}")
}

#[test]
fn path_qjs_answers_what_std_path_answers_through_the_cli() {
    let dir = std::env::temp_dir().join(format!("agenterm-qjs-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let entry = dir.join("main.qjs");
    std::fs::write(&entry, probe_script()).expect("fixture is writable");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("qjs");

    let mut command = Command::new(AGENTERM_BIN);
    command
        .args(["cli", "script", "run"])
        .arg(&entry)
        .arg("--project-root")
        .arg(&root)
        .env_remove("AGENTERM_SCRIPT_BACKEND");
    let output = command.output().expect("the CLI binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let expected = expected_output();
    assert!(
        stdout.contains(&expected),
        "path.qjs disagrees with std::path\n  expected: {expected}\n  stdout:   {}\n  stderr:   {stderr}",
        stdout.trim_end()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The oracle itself, pinned, so a reader sees the three named cases in
/// plain text rather than trusting a computed string: nested, root, single,
/// an absolute tail replacing the base, and a base with a trailing slash
/// gaining no second separator.
#[test]
fn the_oracle_says_what_the_milestone_named() {
    assert_eq!(expected_parent("a/b/c"), "a/b");
    assert_eq!(expected_parent("/a/b"), "/a");
    assert_eq!(expected_parent("/"), "");
    assert_eq!(expected_parent("a"), "");
    assert_eq!(expected_parent("a/b/"), "a");
    assert_eq!(expected_join("a/b", "/c/d"), "/c/d");
    assert_eq!(expected_join("a/", "b"), "a/b");
    assert_eq!(expected_join("a", "b"), "a/b");
}
