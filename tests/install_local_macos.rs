#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fixture executable");
    let mut permissions = fs::metadata(path).expect("fixture metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

#[test]
fn local_build_installs_a_dock_safe_app_bundle() {
    let unique = format!(
        "agenterm-local-install-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    let binaries = root.join("build");
    let install = root.join("install");
    let bin = root.join("bin");
    let applications = root.join("applications");
    fs::create_dir_all(&binaries).expect("create fixture build");

    write_executable(
        &binaries.join("agenterm"),
        &format!("#!/bin/sh\necho 'agenterm cli {CURRENT_VERSION}'\n"),
    );
    write_executable(
        &binaries.join("agenterm-cu"),
        r#"#!/bin/sh
printf '%s\n' '{"ok":true,"data":{"checks":{"abi":{"status":"available","detail":{"major":1,"minor":0,"required_major":1,"required_minor":0,"required_symbols":1}}}}}'
"#,
    );
    fs::write(binaries.join("libagenterm.dylib"), b"fixture ABI")
        .expect("write fixture ABI library");
    fs::write(
        binaries.join("agenterm-cu-provider.dylib"),
        b"fixture CU provider",
    )
    .expect("write fixture CU provider");

    let install_once = || {
        Command::new("bash")
            .arg("install.sh")
            .arg("--local-build")
            .arg(&binaries)
            .env("AGENTERM_INSTALL_DIR", &install)
            .env("AGENTERM_BIN_DIR", &bin)
            .env("AGENTERM_APPLICATIONS_DIR", &applications)
            .env("AGENTERM_NO_LAUNCH", "1")
            .output()
            .expect("run local installer")
    };
    let output = install_once();
    assert!(
        output.status.success(),
        "local installer failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        applications
            .join("AgenTerm.app/Contents/MacOS/AgenTerm")
            .exists(),
        "app executable is missing"
    );
    assert!(
        applications
            .join("AgenTerm.app/Contents/Info.plist")
            .is_file(),
        "app Info.plist is missing"
    );
    assert!(
        applications
            .join("AgenTerm.app/Contents/Resources/AgenTerm.icns")
            .is_file(),
        "app icon is missing"
    );
    let plist = fs::read_to_string(applications.join("AgenTerm.app/Contents/Info.plist"))
        .expect("read app Info.plist");
    assert!(
        plist.contains("<key>CFBundleIconFile</key>")
            && plist.contains("<string>AgenTerm.icns</string>"),
        "app icon is not declared"
    );
    assert!(bin.join("agenterm").exists(), "agenterm link is missing");
    let cu_metadata =
        fs::symlink_metadata(bin.join("agenterm-cu")).expect("installed agenterm-cu metadata");
    assert!(
        cu_metadata.file_type().is_file() && !cu_metadata.file_type().is_symlink(),
        "agenterm-cu must be a regular PATH executable"
    );
    assert!(
        bin.join("agenterm-cu-provider.dylib").exists(),
        "CU provider sibling is missing"
    );
    let reinstall = install_once();
    assert!(
        reinstall.status.success(),
        "installer must replace its own regular CU copy:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&reinstall.stdout),
        String::from_utf8_lossy(&reinstall.stderr)
    );
    assert!(
        fs::symlink_metadata(bin.join("agenterm-cu"))
            .expect("reinstalled agenterm-cu metadata")
            .file_type()
            .is_file(),
        "a reinstall must retain the regular executable layout"
    );

    write_executable(&bin.join("agenterm-cu"), "#!/bin/sh\nexit 9\n");
    let unmanaged = install_once();
    assert!(
        !unmanaged.status.success()
            && String::from_utf8_lossy(&unmanaged.stderr)
                .contains("refusing to replace an unmanaged agenterm-cu file"),
        "installer must not overwrite an unmanaged regular file:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&unmanaged.stdout),
        String::from_utf8_lossy(&unmanaged.stderr)
    );
    assert!(install.join("current").exists(), "current link is missing");
    let installed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(install.join("current/installed.json")).expect("read installed.json"),
    )
    .expect("parse installed.json");
    assert_eq!(installed["channel"], "local-build");
    assert_eq!(installed["distribution"], "local");
    assert_eq!(
        installed["variant"],
        format!("macos-{}-local", std::env::consts::ARCH)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("distribution local"),
        "install output does not report the distribution"
    );
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => panic!("unexpected macOS architecture: {other}"),
    };
    assert!(
        install
            .join(format!(
                "releases/{CURRENT_VERSION}-local-macos-{architecture}/agenterm"
            ))
            .exists(),
        "versioned local payload is missing"
    );

    fs::remove_dir_all(&root).expect("remove fixture root");
}
