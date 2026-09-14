//! Repository source-boundary regression tests.

use std::fs;
use std::path::{Path, PathBuf};

#[path = "adapters/linux/contract_manifest.rs"]
mod linux_adapter_contract;
#[path = "adapters/macos/contract_manifest.rs"]
mod macos_adapter_contract;
#[path = "adapters/windows/contract_manifest.rs"]
mod windows_adapter_contract;

const NATIVE_BOUNDARY_MARKERS: &[&str] = &[
    "target_os",
    "target_family",
    "windows_sys",
    "std::os::windows",
    "std::os::unix",
    "libc::",
    "objc2::",
    "core_foundation::",
    "rmux_pty::",
    "softbuffer::",
    "winit::",
    "raw_window_handle::",
    // A bare `#[link(name = "kernel32")]` in an adapter is compiled on every
    // host, because `adapters/mod.rs` declares each adapter module
    // unconditionally. It then breaks the link on the other two platforms with
    // `ld: library '<name>' not found` while `cargo build --lib` still passes,
    // so nothing notices until someone links a test or binary.
    //
    // This marker earns its keep on the host that *can* link the symbol: there
    // the suite fails by name and points at the offending line, instead of the
    // breakage surfacing only on someone else's machine. On a host that cannot
    // link it the linker still wins first, so treat this as a fast review
    // signal rather than the sole defence.
    "#[link(",
];

/// Product source coupling: naming the product crate or its modules. Banned
/// everywhere in `agenterm-platform`, adapters included: the crate is embedded
/// by other applications and must not compile against the product.
const PRODUCT_SOURCE_COUPLING_MARKERS: &[&str] = &[
    "crate::client",
    "crate::commands",
    "crate::control_center",
    "crate::fleet",
    "crate::instances",
    "crate::theme",
    "crate::ui_",
    "agenterm::",
];

/// Product-prefixed environment names. The adapters read a few of them by
/// convention for the products that embed this crate, so they are allowed under
/// `src/adapters/`; anywhere else — a contract, a facade, a service — the prefix
/// is a leak of product naming into code that must work for every consumer.
const PRODUCT_ENV_PREFIX: &str = "AGENTERM_";

const PLATFORM_CRATE: &str = "crates/agenterm-platform";

/// Raw OS mechanisms: calling the OS directly, naming an OS binding crate, or
/// linking a native library. Inside `agenterm-platform` these belong to the
/// adapters and to the one selector; a facade that has to name a target must
/// still go through them rather than reaching the OS itself.
const RAW_OS_MECHANISM_MARKERS: &[&str] = &[
    "windows_sys::",
    "std::os::windows",
    "std::os::unix",
    "libc::",
    "objc2::",
    "core_foundation::",
    "rmux_pty::",
    "softbuffer::",
    "winit::",
    "raw_window_handle::",
    "#[link(",
];

const SUBSYSTEM_ENTRYPOINTS: &[&str] = &["src/bin/agenterm.rs", "src/bin/agenterm-cc.rs"];
const WINDOWS_SUBSYSTEM_ATTRIBUTE: &str = "#![cfg_attr(windows, windows_subsystem = \"windows\")]";

/// The CUI trampoline is a native Win32 entrypoint BY DESIGN: `no_std`,
/// a custom `mainCRTStartup` export, and direct `#[link]` kernel32 imports.
/// It cannot delegate to the platform crate (the crate's runtime would blow
/// the trampoline's 64KiB staged-size budget), so the whole file is exempt.
///
/// The lightweight host used to hold the second exemption here for the same
/// structural reason — its `startup.rs` declares the linker-visible entry
/// symbol and the `.CRT$X*` initializer arrays *of its own binary*. That
/// product left for the `minicon` repository, taking the exemption with it.
const NATIVE_ENTRYPOINT_EXEMPTIONS: &[&str] = &["src/bin/agenterm-com.rs"];

#[test]
fn product_binary_inventory_matches_manifest_source_tree_and_architecture() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read root manifest");
    let architecture =
        fs::read_to_string(root.join("plan/ARCHITECTURE.md")).expect("read architecture SSOT");
    let executable_entries = architecture
        .split_once("## 2. 可执行入口（bins）")
        .expect("architecture has executable-entry section")
        .1
        .split_once("\n## ")
        .expect("architecture executable-entry section is bounded")
        .0;

    let mut manifest_paths = manifest
        .lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("path = \"")?.strip_suffix('"')?;
            value.starts_with("src/bin/").then(|| value.to_owned())
        })
        .collect::<Vec<_>>();
    manifest_paths.sort();

    let mut source_paths = fs::read_dir(root.join("src/bin"))
        .expect("read product bin directory")
        .filter_map(|entry| {
            let entry = entry.expect("read product bin entry");
            entry
                .file_type()
                .expect("read product bin file type")
                .is_file()
                .then(|| format!("src/bin/{}", entry.file_name().to_string_lossy()))
        })
        .filter(|path| path.ends_with(".rs"))
        .collect::<Vec<_>>();
    source_paths.sort();

    let mut architecture_paths = executable_entries
        .lines()
        .filter_map(|line| {
            let start = line.find("`src/bin/")? + 1;
            let tail = &line[start..];
            let end = tail.find('`')?;
            Some(tail[..end].to_owned())
        })
        .collect::<Vec<_>>();
    architecture_paths.sort();
    architecture_paths.dedup();

    assert_eq!(
        manifest_paths, source_paths,
        "root manifest and src/bin drifted"
    );
    assert_eq!(
        architecture_paths, source_paths,
        "plan/ARCHITECTURE.md executable-entry table drifted from src/bin"
    );
}

#[test]
fn production_sources_use_platform_crate_as_the_only_native_boundary() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    collect_rust_sources(&root.join("src"), &mut sources);
    sources.sort();

    let mut violations = Vec::new();
    for path in sources {
        let relative = path
            .strip_prefix(&root)
            .expect("source is below manifest root")
            .to_string_lossy()
            .replace('\\', "/");
        if NATIVE_ENTRYPOINT_EXEMPTIONS.contains(&relative.as_str()) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("read Rust source");
        // Product-specific frontends and Control Center/Fleet extensions remain
        // ordinary main-crate code: they may call the public platform API, but
        // they do not receive an exemption for target selection or native types.
        let source = if SUBSYSTEM_ENTRYPOINTS.contains(&relative.as_str()) {
            source.replacen(WINDOWS_SUBSYSTEM_ATTRIBUTE, "", 1)
        } else {
            source
        };
        let production = mask_test_items(&mask_comments_and_strings(&source));
        for marker in NATIVE_BOUNDARY_MARKERS {
            if let Some(position) = production.find(marker) {
                let line = production[..position]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                violations.push(format!(
                    "{relative}:{line}: forbidden platform marker `{marker}`"
                ));
            }
        }
        // Subsystem entrypoint bins exist to bridge the windows-subsystem /
        // console divide, so they may BRANCH on the target — but the native
        // marker check above still bars them from raw native types or
        // links; mechanics must come through the platform crate's API
        // (e.g. `process::StdHandle`, `process::ScopedConsole`).
        if SUBSYSTEM_ENTRYPOINTS.contains(&relative.as_str()) {
            continue;
        }
        if let Some((position, target)) = find_cfg_target(&production) {
            let line = production[..position]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            violations.push(format!(
                "{relative}:{line}: forbidden platform cfg target `{target}`"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "production OS boundaries must stay in crates/agenterm-platform/** (apart from the exact subsystem attributes removed above):\n{}",
        violations.join("\n")
    );
}

#[test]
fn platform_crate_native_mechanics_stay_in_selected_and_adapters() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let platform_root = root.join(PLATFORM_CRATE).join("src");
    let mut sources = Vec::new();
    collect_rust_sources(&platform_root, &mut sources);
    sources.sort();

    let mut violations = Vec::new();
    for path in sources {
        let relative = path
            .strip_prefix(&root)
            .expect("source is below manifest root")
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).expect("read Rust source");
        let production = mask_test_items(&mask_comments_and_strings(&source));
        let owner = platform_selection_owner(&relative);
        // Routing is read with string literals kept: a facade hands the choice
        // to its adapter with `#[path = "adapters/…"]`, and blanking the literal
        // would hide exactly the delegation this gate is looking for.
        let routes = routes_to_selection_owner(&mask_test_items(&mask_comments(&source)));
        // Raw OS mechanics belong to the adapters and to the one selector. A
        // facade may name a target, but it must not reach the OS itself.
        if !owner {
            for marker in RAW_OS_MECHANISM_MARKERS {
                if let Some(position) = production.find(marker) {
                    violations.push(format!(
                        "{relative}:{}: raw OS mechanism `{marker}` must stay in selected.rs or adapters",
                        line_of(&production, position)
                    ));
                }
            }
        }
        // A target predicate outside those owners has to be the thin gate of a
        // file that visibly delegates (`crate::selected`, its own `selected`
        // submodule, or an adapter path), a helper that only exists for the
        // platforms that have the mechanism (`any(target_os = .., test)`), or a
        // named exception below. Anything else is a new selector, and a new
        // selector is a leak rather than a stale test.
        if owner {
            continue;
        }
        for selection in cfg_predicates(&production) {
            if !predicate_selects_a_target(&selection.predicate) {
                continue;
            }
            if routes
                || predicate_requires_test(&selection.predicate)
                || listed_selection_exception(&relative)
            {
                continue;
            }
            violations.push(format!(
                "{relative}:{}: target predicate `{}` is not a gate over selected.rs or adapters",
                line_of(&production, selection.predicate_at),
                selection.predicate.trim()
            ));
        }
    }

    for (relative, reason) in NATIVE_SELECTION_EXCEPTIONS {
        let path = root.join(relative);
        assert!(
            path.is_file(),
            "named selection exception {relative} must still exist ({reason})"
        );
        let source = fs::read_to_string(&path).expect("read named exception");
        let production = mask_test_items(&mask_comments_and_strings(&source));
        assert!(
            cfg_predicates(&production)
                .iter()
                .any(|selection| predicate_selects_a_target(&selection.predicate)),
            "named selection exception {relative} no longer selects a target; drop the entry ({reason})"
        );
    }

    assert!(
        violations.is_empty(),
        "platform contracts/services contain native mechanics or OS selection:\n{}",
        violations.join("\n")
    );
}

/// The crate's selection surface: the one selector, any per-feature selector,
/// and the adapters. Only these may hold raw OS mechanics or name a target.
fn platform_selection_owner(relative: &str) -> bool {
    let prefix = format!("{PLATFORM_CRATE}/src/");
    let Some(within) = relative.strip_prefix(&prefix) else {
        return false;
    };
    within == "selected.rs" || within.ends_with("/selected.rs") || within.starts_with("adapters/")
}

/// A file that hands the choice to an owner instead of implementing it.
fn routes_to_selection_owner(production: &str) -> bool {
    production.contains("crate::selected")
        || production.contains("mod selected;")
        || production.contains("adapters/")
}

/// `cfg(test)` and `cfg(all(test, ..))` can only hold under `cargo test`.
/// `cfg(any(target_os = "linux", test))` stays product-visible on Linux, so it
/// is not test-only and must be classified like any other target predicate.
fn predicate_requires_test(predicate: &str) -> bool {
    match predicate_operator(predicate) {
        "test" => true,
        "all" => split_top_level_arguments(&predicate_after_operator(predicate))
            .iter()
            .any(|argument| predicate_requires_test(argument)),
        _ => false,
    }
}

/// The operator of one predicate: `test`, `all`, `any`, or an empty string when
/// the predicate has no argument list.
fn predicate_operator(predicate: &str) -> &str {
    let trimmed = predicate.trim();
    match trimmed.find('(') {
        Some(open) => trimmed[..open].trim(),
        None => trimmed,
    }
}

/// The argument list of `all(..)` / `any(..)`, without the operator itself.
fn predicate_after_operator(predicate: &str) -> String {
    let Some(open) = predicate.find('(') else {
        return String::new();
    };
    let Some(close) = predicate.rfind(')') else {
        return String::new();
    };
    if close <= open {
        return String::new();
    }
    predicate[open + 1..close].to_owned()
}

/// Split one predicate list on its top-level commas.
fn split_top_level_arguments(predicate: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut depth = 0_u32;
    let mut start = 0;
    for (index, character) in predicate.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(predicate[start..index].to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }
    arguments.push(predicate[start..].to_owned());
    arguments
        .into_iter()
        .map(|argument| argument.trim().to_owned())
        .filter(|argument| !argument.is_empty())
        .collect()
}

/// One compile-time predicate found in masked source.
struct CfgPredicate {
    /// Byte offset of the predicate's first character.
    predicate_at: usize,
    /// Byte offset of the `#` that starts the attribute, when this predicate is
    /// a `#[cfg(..)]` attribute rather than a `cfg!(..)` expression or a
    /// `#[cfg_attr(..)]` attribute.
    attribute_at: Option<usize>,
    predicate: String,
}

/// Every `cfg(..)` / `cfg_attr(..)` / `cfg!(..)` predicate in masked source.
fn cfg_predicates(source: &str) -> Vec<CfgPredicate> {
    let bytes = source.as_bytes();
    let mut predicates = Vec::new();
    for name in ["cfg", "cfg_attr"] {
        let mut cursor = 0;
        while let Some(relative) = source[cursor..].find(name) {
            let start = cursor + relative;
            let before_is_identifier =
                start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
            let after_name = start + name.len();
            let after_is_identifier = after_name < bytes.len()
                && (bytes[after_name].is_ascii_alphanumeric() || bytes[after_name] == b'_');
            if before_is_identifier || after_is_identifier {
                cursor = after_name;
                continue;
            }
            let mut open = after_name;
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            let mut expression = false;
            if name == "cfg" && open < bytes.len() && bytes[open] == b'!' {
                expression = true;
                open += 1;
                while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                    open += 1;
                }
            }
            if open >= bytes.len() || bytes[open] != b'(' {
                cursor = after_name;
                continue;
            }
            let mut depth = 0_u32;
            let mut end = open;
            for (offset, byte) in bytes[open..].iter().copied().enumerate() {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            predicates.push(CfgPredicate {
                predicate_at: open + 1,
                attribute_at: if expression || name == "cfg_attr" {
                    None
                } else {
                    attribute_start(bytes, start)
                },
                predicate: source[open + 1..end].to_owned(),
            });
            cursor = (end + 1).max(after_name);
        }
    }
    predicates.sort_by_key(|selection| selection.predicate_at);
    predicates
}

/// The `#` that opens the attribute containing `name_at`, if this occurrence is
/// an attribute. The gap may hold `[`, `!` (inner attribute) and whitespace.
fn attribute_start(bytes: &[u8], name_at: usize) -> Option<usize> {
    let mut cursor = name_at;
    while cursor > 0 {
        cursor -= 1;
        match bytes[cursor] {
            b'[' | b'!' => continue,
            byte if byte.is_ascii_whitespace() => continue,
            b'#' => return Some(cursor),
            _ => return None,
        }
    }
    None
}

/// True when a predicate names the target it compiles for.
fn predicate_selects_a_target(predicate: &str) -> bool {
    identifier_tokens(predicate).any(|(_, token)| {
        // `target_arch` is deliberately absent: an architecture-specialized SIMD
        // path is a performance choice inside a neutral module, not OS selection.
        matches!(token, "target_os" | "target_family" | "windows" | "unix")
    })
}

/// Selection debt that has no owner to route to yet. Each entry names the file
/// and why the predicate is std-only platform semantics rather than a mechanism;
/// the caller verifies every entry still exists and still selects a target.
const NATIVE_SELECTION_EXCEPTIONS: &[(&str, &str)] = &[
    (
        "crates/agenterm-platform/src/device_inventory.rs",
        "cfg-confined std-only locator type",
    ),
    (
        "crates/agenterm-platform/src/filesystem_create.rs",
        "cfg-confined std-only Windows path semantics",
    ),
    (
        "crates/agenterm-platform/src/contract/host_pressure.rs",
        "contract helper that exists only on the platform with the mechanism and under test; it holds no raw OS call, and moving it into the owning facade is the recorded next step",
    ),
    (
        "crates/agenterm-platform/src/contract/login_session.rs",
        "contract helper that exists only on the platform with the mechanism and under test; it holds no raw OS call, and moving it into the owning facade is the recorded next step",
    ),
];

fn listed_selection_exception(relative: &str) -> bool {
    NATIVE_SELECTION_EXCEPTIONS
        .iter()
        .any(|(path, _)| *path == relative)
}

/// The 1-based line a byte offset falls on.
fn line_of(source: &str, position: usize) -> usize {
    source[..position]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

#[test]
fn platform_crate_has_no_agenterm_product_dependency_or_source_coupling() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let platform_root = root.join(PLATFORM_CRATE);
    let manifest = fs::read_to_string(platform_root.join("Cargo.toml"))
        .expect("read agenterm-platform manifest");
    let manifest_without_comments = manifest
        .lines()
        .map(|line| line.split_once('#').map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    let mut violations = Vec::new();

    for (line_number, line) in manifest_without_comments.lines().enumerate() {
        let trimmed = line.trim();
        let compact = trimmed.split_whitespace().collect::<String>();
        if compact.starts_with("agenterm=")
            || compact.starts_with("agenterm.")
            || (compact.starts_with('[') && compact.ends_with(".agenterm]"))
            || compact.contains("package=\"agenterm\"")
            || compact.contains("path=\"../..\"")
        {
            violations.push(format!(
                "{PLATFORM_CRATE}/Cargo.toml:{}: reverse dependency on the Agenterm product crate",
                line_number + 1
            ));
        }
    }

    let mut sources = Vec::new();
    collect_rust_sources(&platform_root.join("src"), &mut sources);
    sources.sort();
    for path in sources {
        let relative = path
            .strip_prefix(&root)
            .expect("source is below manifest root")
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).expect("read Rust source");
        let production = mask_test_items(&mask_comments(&source));
        for marker in PRODUCT_SOURCE_COUPLING_MARKERS {
            if let Some(position) = production.find(marker) {
                let line = production[..position]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                violations.push(format!(
                    "{relative}:{line}: product coupling marker `{marker}`"
                ));
            }
        }
        let in_adapters = relative.contains("/src/adapters/");
        if !in_adapters && production.contains(PRODUCT_ENV_PREFIX) {
            let position = production
                .find(PRODUCT_ENV_PREFIX)
                .expect("prefix is present");
            let line = production[..position]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            violations.push(format!(
                "{relative}:{line}: `{PRODUCT_ENV_PREFIX}` environment name outside the adapters"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "agenterm-platform must stay product-neutral outside its adapters:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            collect_rust_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn find_cfg_target(source: &str) -> Option<(usize, &'static str)> {
    let bytes = source.as_bytes();
    for name in ["cfg", "cfg_attr"] {
        let mut cursor = 0;
        while let Some(relative) = source[cursor..].find(name) {
            let start = cursor + relative;
            let before_is_identifier =
                start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
            let after_name = start + name.len();
            let after_is_identifier = after_name < bytes.len()
                && (bytes[after_name].is_ascii_alphanumeric() || bytes[after_name] == b'_');
            if before_is_identifier || after_is_identifier {
                cursor = after_name;
                continue;
            }

            let mut open = after_name;
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            if name == "cfg" && open < bytes.len() && bytes[open] == b'!' {
                open += 1;
                while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                    open += 1;
                }
            }
            if open >= bytes.len() || bytes[open] != b'(' {
                cursor = after_name;
                continue;
            }

            let mut depth = 0_u32;
            let mut end = open;
            for (offset, byte) in bytes[open..].iter().copied().enumerate() {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            let expression = &source[open + 1..end];
            for (offset, token) in identifier_tokens(expression) {
                if token == "windows" {
                    return Some((open + 1 + offset, "windows"));
                }
                if token == "unix" {
                    return Some((open + 1 + offset, "unix"));
                }
            }
            cursor = end.max(after_name);
        }
    }
    None
}

fn identifier_tokens(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let bytes = source.as_bytes();
        while cursor < bytes.len()
            && !(bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return None;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        Some((start, &source[start..cursor]))
    })
}

/// Mask every item that exists only under `cargo test`.
///
/// The original marker was the literal `#[cfg(test)]`; the crate also gates test
/// code as `#[cfg(all(test, ..))]` and as a plain `#[test] fn` outside a test
/// module, whose fixtures are not product code either.
/// `cfg(any(target_os = .., test))` is deliberately NOT masked: that item is
/// compiled for real on the named target and stays classified as product code.
/// `#[cfg_attr(test, ..)]` only adds an attribute under test, so its item stays
/// visible too.
fn mask_test_items(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut starts = test_only_item_starts(source);
    for selection in cfg_predicates(source) {
        if !predicate_requires_test(&selection.predicate) {
            continue;
        }
        if let Some(start) = selection.attribute_at {
            starts.push(start);
        }
    }
    starts.sort_unstable();
    starts.dedup();
    for start in starts {
        let Some(open) = bytes[start..]
            .iter()
            .position(|byte| *byte == b'{')
            .map(|offset| start + offset)
        else {
            continue;
        };
        let mut depth = 0_u32;
        let mut end = bytes.len();
        for (offset, byte) in bytes[open..].iter().copied().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("mask preserves UTF-8")
}

/// The `#` of every attribute that marks its item as test-only: `#[test]` and
/// its path-qualified forms, e.g. `#[tokio::test]`.
fn test_only_item_starts(source: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    let mut starts = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find("#[") {
        let hash = cursor + relative;
        let Some(close) = bytes[hash..].iter().position(|byte| *byte == b']') else {
            break;
        };
        let attribute = &source[hash + 2..hash + close];
        if attribute == "test" || attribute.ends_with("::test") {
            starts.push(hash);
        }
        cursor = hash + close + 1;
    }
    starts
}

fn mask_comments_and_strings(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"//") {
            let end = bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset);
            mask_non_newlines(&mut bytes[cursor..end]);
            cursor = end;
        } else if bytes[cursor..].starts_with(b"/*") {
            let end = bytes[cursor + 2..]
                .windows(2)
                .position(|window| window == b"*/")
                .map_or(bytes.len(), |offset| cursor + 2 + offset + 2);
            mask_non_newlines(&mut bytes[cursor..end]);
            cursor = end;
        } else if bytes[cursor] == b'"' {
            let mut end = cursor + 1;
            while end < bytes.len() {
                if bytes[end] == b'\\' {
                    end = (end + 2).min(bytes.len());
                } else if bytes[end] == b'"' {
                    end += 1;
                    break;
                } else {
                    end += 1;
                }
            }
            mask_non_newlines(&mut bytes[cursor..end]);
            cursor = end;
        } else {
            cursor += 1;
        }
    }
    String::from_utf8(bytes).expect("mask preserves UTF-8")
}

fn mask_comments(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"//") {
            let end = bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset);
            mask_non_newlines(&mut bytes[cursor..end]);
            cursor = end;
        } else if bytes[cursor..].starts_with(b"/*") {
            let end = bytes[cursor + 2..]
                .windows(2)
                .position(|window| window == b"*/")
                .map_or(bytes.len(), |offset| cursor + 2 + offset + 2);
            mask_non_newlines(&mut bytes[cursor..end]);
            cursor = end;
        } else if bytes[cursor] == b'"' {
            cursor += 1;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' {
                    cursor = (cursor + 2).min(bytes.len());
                } else if bytes[cursor] == b'"' {
                    cursor += 1;
                    break;
                } else {
                    cursor += 1;
                }
            }
        } else {
            cursor += 1;
        }
    }
    String::from_utf8(bytes).expect("mask preserves UTF-8")
}

fn mask_non_newlines(bytes: &mut [u8]) {
    for byte in bytes {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

#[test]
fn boundary_mask_ignores_tests_and_comments_but_not_product_code() {
    let fixture = r#"
// windows_sys in a comment is not product code.
fn product() { windows_sys::native_call(); }
#[cfg(test)]
mod tests {
    #[cfg(windows)]
    fn native_fixture() { std::os::windows::ffi::OsStrExt::encode_wide; }
}
"#;
    let masked = mask_test_items(&mask_comments_and_strings(fixture));
    assert!(masked.contains("windows_sys::native_call"));
    assert!(!masked.contains("std::os::windows"));
    assert!(!masked.contains("windows_sys in a comment"));
}

#[test]
fn boundary_mask_also_ignores_all_test_gated_modules_but_not_product_helpers() {
    let fixture = r#"
#[cfg(all(test, target_os = "macos"))]
mod fixtures {
    fn euid() { let _ = unsafe { libc::geteuid() }; }
}
#[cfg(any(target_os = "linux", test))]
pub(crate) fn checked_window() { let _ = unsafe { libc::geteuid() }; }
#[cfg_attr(test, path = "adapters/macos/probe.rs")]
mod probe;
"#;
    let masked = mask_test_items(&mask_comments_and_strings(fixture));
    assert!(
        !masked.contains("mod fixtures"),
        "a cfg(all(test, ..)) module is test code"
    );
    assert!(
        masked.contains("checked_window"),
        "cfg(any(target_os = .., test)) is product code on Linux"
    );
    assert!(
        masked.contains("mod probe"),
        "cfg_attr(test, ..) does not make its item test-only"
    );
}

#[test]
fn selection_owners_routing_gates_and_test_helpers_are_distinguished() {
    for owner in [
        "crates/agenterm-platform/src/selected.rs",
        "crates/agenterm-platform/src/simulator/selected.rs",
        "crates/agenterm-platform/src/adapters/macos/app_facts.rs",
    ] {
        assert!(
            platform_selection_owner(owner),
            "{owner} is a selection owner"
        );
    }
    assert!(!platform_selection_owner(
        "crates/agenterm-platform/src/audio.rs"
    ));
    assert!(!platform_selection_owner(
        "src/platform/adapters/macos/x.rs"
    ));
    assert!(routes_to_selection_owner(
        "crate::selected::audio::status()"
    ));
    assert!(routes_to_selection_owner(
        "#[path = \"adapters/macos/audio.rs\"] mod native;"
    ));
    assert!(routes_to_selection_owner("mod selected;"));
    assert!(!routes_to_selection_owner("pub fn audio_status() {}"));
    // Only a predicate that cannot hold outside `cargo test` is a test helper.
    assert!(predicate_requires_test("test"));
    assert!(predicate_requires_test("all(test, target_os = \"macos\")"));
    assert!(predicate_requires_test(
        "all(test, any(target_os = \"linux\", target_os = \"macos\"))"
    ));
    assert!(!predicate_requires_test("any(target_os = \"linux\", test)"));
    assert!(!predicate_requires_test(
        "any(all(target_os = \"macos\", feature = \"simulator\"), test)"
    ));
    assert!(!predicate_requires_test("target_os = \"linux\""));
    assert!(predicate_selects_a_target(
        "all(feature = \"native\", windows)"
    ));
    assert!(predicate_selects_a_target("target_family = \"unix\""));
    assert!(!predicate_selects_a_target("feature = \"native\""));
}

#[test]
fn cfg_target_scan_detects_nested_and_spaced_target_predicates() {
    let nested = "#[cfg(all(feature = \"native\", windows))] fn native() {}";
    let spaced = "if cfg ! ( any(unix, feature = \"portable\") ) {}";
    assert_eq!(
        find_cfg_target(nested).map(|(_, target)| target),
        Some("windows")
    );
    assert_eq!(
        find_cfg_target(spaced).map(|(_, target)| target),
        Some("unix")
    );
    assert_eq!(find_cfg_target("let windows = Vec::new();"), None);
}

#[test]
fn product_coupling_mask_keeps_literals_but_ignores_tests_and_comments() {
    let fixture = r#"
// AGENTERM_COMMENT is not product code.
fn product_default() { let _ = "AGENTERM_INSTANCE_DIR"; }
#[cfg(test)]
mod tests {
    fn fixture() { crate::commands::run(); }
}
"#;
    let masked = mask_test_items(&mask_comments(fixture));
    assert!(masked.contains("AGENTERM_INSTANCE_DIR"));
    assert!(!masked.contains("AGENTERM_COMMENT"));
    assert!(!masked.contains("crate::commands"));
}

#[test]
fn all_three_adapters_satisfy_the_same_contract() {
    use crate::platform::contract::adapter::validate_adapter_contract;

    let declarations = [
        (
            &windows_adapter_contract::DECLARATION,
            windows_adapter_contract::unsupported_probe(),
            windows_adapter_contract::failed_probe(),
        ),
        (
            &linux_adapter_contract::DECLARATION,
            linux_adapter_contract::unsupported_probe(),
            linux_adapter_contract::failed_probe(),
        ),
        (
            &macos_adapter_contract::DECLARATION,
            macos_adapter_contract::unsupported_probe(),
            macos_adapter_contract::failed_probe(),
        ),
    ];

    for (declaration, unsupported, failed) in declarations {
        validate_adapter_contract(declaration, unsupported, failed)
            .unwrap_or_else(|error| panic!("adapter contract mismatch: {error}"));
    }
}

/// Every `src/platform/services/*.rs` (except `mod.rs`) must be declared in
/// `services/mod.rs`. Prevents orphan shims like the deleted `frontend.rs`.
#[test]
fn platform_services_have_no_orphan_source_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let services_dir = root.join("src/platform/services");
    let mod_src = fs::read_to_string(services_dir.join("mod.rs")).expect("read services/mod.rs");
    let mut orphans = Vec::new();
    for entry in fs::read_dir(&services_dir).expect("read services/") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("utf-8 stem");
        if name == "mod" {
            continue;
        }
        let declare = format!("mod {name}");
        if !mod_src.contains(&declare) {
            orphans.push(format!("src/platform/services/{name}.rs"));
        }
    }
    assert!(
        orphans.is_empty(),
        "orphan service sources (not declared in services/mod.rs):\n{}\nSee plan/ARCHITECTURE.md",
        orphans.join("\n")
    );
}

/// Tracked L1 debt: `src/frontend/mod.rs` must not compile adapters via `#[path]`.
/// Budget 0 after L1 ownership fix.
const FRONTEND_PATH_ATTR_BUDGET: usize = 0;

#[test]
fn frontend_path_attr_debt_does_not_grow() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/frontend/mod.rs")).expect("read frontend.rs");
    let count = source.matches("#[path").count();
    assert_eq!(
        count, FRONTEND_PATH_ATTR_BUDGET,
        "src/frontend/mod.rs has {count} #[path] attrs (budget {FRONTEND_PATH_ATTR_BUDGET}). \
         Do not add more; L1 removes them — see plan/ARCHITECTURE.md §4"
    );
}

/// A frozen surface may only name paths that exist in this checkout. A gate
/// that accepts a stale `crates/.../native.rs` reports a green over a file
/// nobody can open, which is worse than a missing gate: it hides the very
/// migration that moved the path.
///
/// A trailing `/` is this manifest's directory convention, so a name without
/// one must be a regular file.
fn chassis_l1_surface_path_shape(field: &str, value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err(format!("{field} contains an empty path"));
    }
    if value.starts_with('/') || value.starts_with('~') || value.starts_with('\\') {
        return Err(format!(
            "{field} names {value:?}, which is absolute or home-expanded; the surface \
             must name repository-relative paths"
        ));
    }
    if value.contains('\\') || value.contains(':') {
        return Err(format!(
            "{field} names {value:?}, which uses a non-portable separator or drive prefix"
        ));
    }
    if Path::new(value)
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "{field} names {value:?}, which escapes the repository root"
        ));
    }
    Ok(PathBuf::from(value))
}

/// Checks one manifest entry against the checkout rooted at `root`.
fn chassis_l1_surface_path_exists(root: &Path, field: &str, value: &str) -> Result<(), String> {
    let relative = chassis_l1_surface_path_shape(field, value)?;
    let wants_directory = value.ends_with('/');
    let full = root.join(&relative);
    let found = if wants_directory {
        full.is_dir()
    } else {
        full.is_file()
    };
    if found {
        return Ok(());
    }
    let kind = if wants_directory {
        "directory"
    } else {
        "regular file"
    };
    Err(format!(
        "{field} names {value:?}, but this checkout has no such {kind}"
    ))
}

#[test]
fn chassis_l1_surface_names_the_six_cell_candidate_tax() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("plan/chassis-l1-surface.json");
    let raw = fs::read_to_string(&path).expect("plan/chassis-l1-surface.json");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("chassis-l1-surface JSON");
    assert_eq!(value["schema"], 2);
    let reasons = value["l1_reasons"].as_object().expect("l1_reasons");
    let reason_keys: std::collections::BTreeSet<_> = reasons.keys().map(String::as_str).collect();
    let expected_reason_keys = std::collections::BTreeSet::from(["loader", "window", "pty", "ipc"]);
    assert_eq!(
        reason_keys, expected_reason_keys,
        "only the named loader/window/pty/ipc tax may justify a six-cell Candidate"
    );
    let not_l1 = value["explicitly_not_l1"]
        .as_array()
        .expect("explicitly_not_l1");
    let mut l1_paths = std::collections::BTreeSet::new();
    for (reason, surface) in reasons {
        let prefixes = surface["path_prefixes"].as_array().expect("path_prefixes");
        let exact = surface["exact_paths"].as_array().expect("exact_paths");
        assert!(
            !prefixes.is_empty() || !exact.is_empty(),
            "named L1 reason {reason} must contain at least one path"
        );
        for path in prefixes.iter().chain(exact.iter()) {
            let path = path.as_str().expect("L1 path must be a string");
            assert!(!path.is_empty(), "L1 paths must not be empty");
            assert!(l1_paths.insert(path), "duplicate L1 path: {path}");
        }
        for path in prefixes.iter().filter_map(|v| v.as_str()) {
            let field = format!("l1_reasons.{reason}.path_prefixes");
            if let Err(problem) = chassis_l1_surface_path_exists(&root, &field, path) {
                panic!("{problem}");
            }
        }
        for path in exact.iter().filter_map(|v| v.as_str()) {
            let field = format!("l1_reasons.{reason}.exact_paths");
            if let Err(problem) = chassis_l1_surface_path_exists(&root, &field, path) {
                panic!("{problem}");
            }
        }
    }
    assert!(
        not_l1.iter().any(|p| p.as_str() == Some("src/frontend/")),
        "product frontend must stay outside L1"
    );
    assert!(
        not_l1
            .iter()
            .any(|p| p.as_str() == Some("crates/agenterm-cu/")),
        "computer-use must stay an L2 product, not L1"
    );
    for excluded in not_l1.iter().filter_map(|v| v.as_str()) {
        assert!(
            !l1_paths.contains(excluded),
            "{excluded} cannot be both L1 and explicitly-not-L1"
        );
        if let Err(problem) = chassis_l1_surface_path_exists(&root, "explicitly_not_l1", excluded) {
            panic!("{problem}");
        }
    }
    let notes = value["notes"].as_array().expect("notes");
    assert!(
        notes.iter().filter_map(|note| note.as_str()).any(|note| {
            note.contains("frozen loader surface")
                && note.contains("not a claim")
                && note.contains("workbench PE")
        }),
        "surface must state that the named Candidate tax does not claim the workbench PE is thin"
    );
}

#[test]
fn chassis_l1_surface_rejects_a_missing_repo_relative_path() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let missing_file = chassis_l1_surface_path_exists(
        &root,
        "l1_reasons.loader.exact_paths",
        "crates/agenterm-chassis/src/no_such_surface_file.rs",
    )
    .expect_err("a named file that does not exist must be refused");
    assert!(
        missing_file.contains("l1_reasons.loader.exact_paths")
            && missing_file.contains("no_such_surface_file.rs")
            && missing_file.contains("regular file"),
        "the refusal must name the JSON field and the missing value: {missing_file}"
    );
    let missing_directory = chassis_l1_surface_path_exists(
        &root,
        "l1_reasons.loader.path_prefixes",
        "crates/agenterm-chassis/src/no_such_surface_dir/",
    )
    .expect_err("a named directory that does not exist must be refused");
    assert!(
        missing_directory.contains("l1_reasons.loader.path_prefixes")
            && missing_directory.contains("no_such_surface_dir")
            && missing_directory.contains("directory"),
        "the refusal must name the JSON field and the missing value: {missing_directory}"
    );
}

#[test]
fn chassis_l1_surface_path_shape_accepts_the_names_the_manifest_uses() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(chassis_l1_surface_path_shape("field", "rust-toolchain.toml").is_ok());
    assert!(chassis_l1_surface_path_shape("field", "crates/agenterm-chassis/src/loader/").is_ok());
    assert!(chassis_l1_surface_path_exists(&root, "field", "rust-toolchain.toml").is_ok());
    assert!(
        chassis_l1_surface_path_exists(&root, "field", "crates/agenterm-chassis/src/loader/")
            .is_ok()
    );
    // A file named without the directory convention must not pass as a directory.
    assert!(chassis_l1_surface_path_exists(&root, "field", "rust-toolchain.toml/").is_err());
}

#[test]
fn chassis_l1_surface_refuses_paths_that_leave_the_repository() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for value in [
        "/etc/passwd",
        "~/secret",
        "C:/windows/system32",
        "crates/../../etc/passwd",
        "crates\\agenterm-cu\\src\\lib.rs",
        "",
    ] {
        let problem = match chassis_l1_surface_path_exists(&root, "explicitly_not_l1", value) {
            Ok(()) => panic!("{value:?} must be refused"),
            Err(problem) => problem,
        };
        assert!(
            problem.contains("explicitly_not_l1"),
            "the refusal must name the JSON field: {problem}"
        );
    }
}
