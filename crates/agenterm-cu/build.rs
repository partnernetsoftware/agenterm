fn main() {
    if std::env::var_os("AGENTERM_CU_RESOURCE_CHILD").is_some() {
        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"),
        )
        .join("windows-resource");
        std::fs::create_dir_all(&out_dir).expect("create agenterm-cu resource directory");
        winresource::WindowsResource::new()
            .set("ProductName", "AgenTerm")
            .set("ProductVersion", env!("CARGO_PKG_VERSION"))
            .set("FileDescription", "AgenTerm computer-use host")
            .set("OriginalFilename", "agenterm-cu.exe")
            .set_output_directory(out_dir.to_str().expect("resource path must be UTF-8"))
            .compile()
            .expect("failed to compile agenterm-cu.exe VERSIONINFO");
        return;
    }

    build_verb_catalog();

    println!("cargo:rerun-if-env-changed=RC_PATH");
    let target_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !target_msvc {
        return;
    }

    // winresource emits a global link argument. Capture it from a child and
    // project it only to the package's public executable.
    let output =
        std::process::Command::new(std::env::current_exe().expect("agenterm-cu build script path"))
            .env("AGENTERM_CU_RESOURCE_CHILD", "1")
            .output()
            .expect("agenterm-cu resource child failed to run");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("failed to compile agenterm-cu.exe resource child");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resource_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("cargo:rustc-link-arg="))
        .map(str::trim)
        .expect("winresource emitted no agenterm-cu.exe resource link arg");
    println!("cargo:rustc-link-arg-bin=agenterm-cu={resource_path}");
}

fn build_verb_catalog() {
    use std::collections::BTreeSet;

    const CATALOG: &str = "src/bin/cli/verbs-catalog.json";
    println!("cargo:rerun-if-changed={CATALOG}");
    println!("cargo:rerun-if-env-changed=AGENTERM_CU_SIZE_COURT");
    println!("cargo:rerun-if-env-changed=AGENTERM_CU_SIZE_COLD_ROWS");

    let source = std::fs::read(CATALOG).expect("read agenterm-cu verb catalog");
    let mut root: serde_json::Value =
        serde_json::from_slice(&source).expect("parse agenterm-cu verb catalog");
    assert_eq!(root["schema"], 1, "verb catalog schema must be 1");
    let verbs = root["verbs"]
        .as_array()
        .expect("verb catalog verbs must be an array")
        .clone();
    let help = root["help"]
        .as_array()
        .expect("verb catalog help must be an array")
        .clone();
    assert!(!verbs.is_empty(), "verb catalog must not be empty");
    assert_eq!(help.len(), verbs.len(), "one help row per verb is required");

    let mut names = BTreeSet::new();
    let mut spellings = BTreeSet::new();
    let mut help_names = BTreeSet::new();
    for row in &verbs {
        let name = required_str(row, "name");
        let command = required_str(row, "command");
        let grant = required_str(row, "grant");
        let family = required_str(row, "family");
        assert!(names.insert(name), "duplicate verb name {name:?}");
        assert!(spellings.insert(name), "duplicate verb spelling {name:?}");
        assert!(!command.is_empty());
        assert!(matches!(grant, "observe" | "actuate" | "mixed" | "none"));
        if grant == "mixed" {
            let by_shape = row["grant_by_shape"]
                .as_object()
                .expect("mixed grant verb must declare grant_by_shape");
            assert!(!by_shape.is_empty(), "mixed grant map must not be empty");
            for (shape, value) in by_shape {
                assert!(!shape.is_empty(), "mixed grant shape must not be empty");
                assert!(
                    matches!(value.as_str(), Some("observe" | "actuate")),
                    "mixed grant values must be observe or actuate"
                );
            }
        } else {
            assert!(
                row.get("grant_by_shape").is_none(),
                "only mixed grant verbs may declare grant_by_shape"
            );
        }
        assert!(matches!(
            family,
            "system"
                | "windows"
                | "process"
                | "privilege"
                | "network"
                | "file"
                | "terminal"
                | "a11y-observe"
                | "a11y-actuate"
                | "browser"
                | "clipboard"
                | "placement"
                | "transports"
                | "host"
        ));
        assert!(!required_str(row, "summary").is_empty());
        assert!(!required_str(row, "usage").is_empty());
        let aliases = row["aliases"].as_array().expect("aliases must be an array");
        for alias in aliases {
            let alias = alias.as_str().expect("alias must be a string");
            assert!(!alias.is_empty());
            assert!(spellings.insert(alias), "duplicate verb spelling {alias:?}");
        }
        assert!(row["args"].is_array(), "args must be an array");
    }
    for row in &help {
        let name = required_str(row, "name");
        assert!(names.contains(name), "help for unknown verb {name:?}");
        assert!(help_names.insert(name), "duplicate help for {name:?}");
        assert!(!required_str(row, "text").is_empty());
    }
    assert_eq!(help_names, names, "help rows must cover every verb");
    assert!(!required_str(&root, "top_level_text").is_empty());
    assert!(!required_str(&root, "verbs_text").is_empty());

    let court = std::env::var_os("AGENTERM_CU_SIZE_COURT").is_some();
    let rows = std::env::var("AGENTERM_CU_SIZE_COLD_ROWS")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .expect("size-court row count must be numeric")
        })
        .unwrap_or(0);
    assert!(
        court || rows == 0,
        "synthetic cold rows require AGENTERM_CU_SIZE_COURT=1"
    );
    assert!(
        matches!(rows, 0 | 16 | 32),
        "size-court rows must be 0, 16, or 32"
    );
    root["size_probe"] = serde_json::Value::Array(synthetic_cold_rows(&root, rows));

    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let catalog = serde_json::to_vec(&root).expect("serialize validated verb catalog");
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&catalog, 9);
    std::fs::write(out_dir.join("agenterm_cu_verbs_catalog.z"), compressed)
        .expect("write compressed agenterm-cu verb catalog");

    let mut hot = String::from("pub static VERBS: &[VerbSpec] = &[\n");
    for row in &verbs {
        hot.push_str("    VerbSpec { name: ");
        hot.push_str(&format!("{:?}", required_str(row, "name")));
        hot.push_str(", aliases: &[");
        for alias in row["aliases"].as_array().expect("validated aliases") {
            hot.push_str(&format!("{:?},", alias.as_str().expect("validated alias")));
        }
        hot.push_str("], family: Family::");
        hot.push_str(family_variant(required_str(row, "family")));
        hot.push_str(" },\n");
    }
    hot.push_str("];\n");
    hot.push_str(&format!(
        "pub const VERB_CATALOG_BYTES: usize = {};\n",
        catalog.len()
    ));
    std::fs::write(out_dir.join("agenterm_cu_verbs_hot.rs"), hot)
        .expect("write generated agenterm-cu hot verb table");
}

fn required_str<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("verb catalog field {field:?} must be a string"))
}

fn family_variant(family: &str) -> &'static str {
    match family {
        "system" => "System",
        "windows" => "Windows",
        "process" => "Process",
        "privilege" => "Privilege",
        "network" => "Network",
        "file" => "File",
        "terminal" => "Terminal",
        "a11y-observe" => "A11yObserve",
        "a11y-actuate" => "A11yActuate",
        "browser" => "Browser",
        "clipboard" => "Clipboard",
        "placement" => "Placement",
        "transports" => "Transports",
        "host" => "Host",
        _ => unreachable!(),
    }
}

fn synthetic_cold_rows(root: &serde_json::Value, count: usize) -> Vec<serde_json::Value> {
    let template = root["verbs"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "network-probe"))
        .expect("network-probe template row");
    let help = root["help"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "network-probe"))
        .expect("network-probe help template");
    (0..count)
        .map(|index| {
            let mut row = template.clone();
            row["name"] = format!("size-probe-{index:02}").into();
            row["command"] = format!("size-probe-{index:02}").into();
            row["aliases"] = serde_json::json!([format!("sp-{index:02}")]);
            serde_json::json!({"verb": row, "help": help["text"], "ordinal": index})
        })
        .collect()
}
