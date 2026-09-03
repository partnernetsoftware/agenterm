fn main() {
    // Child mode (see below): produce one per-bin resource, letting
    // winresource print its normal `cargo:rustc-link-arg=<resource.res>`
    // line to OUR captured stdout, then exit.
    if let Some(kind) = std::env::var_os("AGENTERM_BUILDRS_RESOURCE_CHILD") {
        let kind = kind.to_string_lossy();
        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"),
        )
        .join(format!("windows-resource-{kind}"));
        std::fs::create_dir_all(&out_dir).expect("create per-bin resource directory");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set("ProductName", "AgenTerm")
            .set("ProductVersion", env!("CARGO_PKG_VERSION"))
            .set_output_directory(out_dir.to_str().expect("resource path must be UTF-8"));
        match kind.as_ref() {
            "icon" => {
                resource
                    .set("FileDescription", "AgenTerm desktop application")
                    .set_icon("assets/agenterm.ico");
            }
            "forwarder" => {
                resource
                    .set("FileDescription", "AgenTerm console forwarder")
                    .set("OriginalFilename", "agenterm.com");
            }
            _ => panic!("unknown AgenTerm resource child kind: {kind}"),
        }
        resource
            .compile()
            .expect("failed to compile AgenTerm resource");
        return;
    }

    println!("cargo:rerun-if-changed=assets/agenterm.ico");
    println!("cargo:rerun-if-changed=assets/skins/fancy/icon.png");
    println!("cargo:rerun-if-changed=assets/skins/fancy/icon.ico");
    println!("cargo:rerun-if-env-changed=RC_PATH");
    // agenterm-com is a no_std/no_main trampoline exporting a custom
    // `mainCRTStartup`. link.exe infers the subsystem from a standard
    // `main`/`WinMain` symbol; with neither present it stops at LNK1561
    // before the custom entry is ever considered, so the subsystem must be
    // stated (lld-link in cross builds refuses outright: "subsystem must be
    // defined"). Once it is, CONSOLE's default entry name resolves to the
    // exported symbol — no explicit /ENTRY needed.
    //
    // Keyed on the TARGET, not `#[cfg(windows)]`: build scripts compile for
    // the host, so a host cfg silently drops these args when cross-compiling
    // to *-pc-windows-msvc from linux (cargo-xwin lanes).
    let target_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if target_msvc {
        println!("cargo:rustc-link-arg-bin=agenterm-com=/SUBSYSTEM:CONSOLE");
        // no_std leaves three externs unresolved on MSVC: core's memcpy/memcmp
        // references and the unwind personality __CxxFrameHandler3. They come
        // from the CRT import libs; pulling them via /DEFAULTLIB cannot clash
        // with the bin's own `mainCRTStartup` because default libraries are
        // only searched for still-unresolved symbols, and the entry is already
        // defined in the bin's object file.
        println!("cargo:rustc-link-arg-bin=agenterm-com=/DEFAULTLIB:vcruntime");
        println!("cargo:rustc-link-arg-bin=agenterm-com=/DEFAULTLIB:ucrt");

        // The icon .rsrc is ~59 KiB and the forwarder's complete staged budget
        // is 64 KiB. Compile a small VERSIONINFO-only resource for agenterm.com
        // and retain the icon resource only on the two GUI bins.
        //
        // winresource emits a global link arg, so run this build script as a
        // child, capture the resource path, and re-emit a per-bin link arg.
        // This runtime TARGET branch works on Windows, Linux and macOS hosts.
        const ICON_BINS: &[&str] = &["agenterm", "agenterm-cc"];
        let me = std::env::current_exe().expect("build script path");
        let compile_resource = |kind: &str| {
            let output = std::process::Command::new(&me)
                .env("AGENTERM_BUILDRS_RESOURCE_CHILD", kind)
                .output()
                .expect("resource child failed to run");
            if !output.status.success() {
                eprintln!("{}", String::from_utf8_lossy(&output.stderr));
                panic!("failed to compile AgenTerm {kind} resource child");
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .find_map(|line| line.strip_prefix("cargo:rustc-link-arg="))
                .map(str::trim)
                .map(str::to_owned)
                .unwrap_or_else(|| panic!("winresource emitted no {kind} resource link arg"))
        };
        let icon_resource = compile_resource("icon");
        let forwarder_resource = compile_resource("forwarder");
        for bin in ICON_BINS {
            println!("cargo:rustc-link-arg-bin={bin}={icon_resource}");
        }
        println!("cargo:rustc-link-arg-bin=agenterm-com={forwarder_resource}");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must provide arch");
        for (environment, soname) in [
            ("AGENTERM_BUNDLED_XKB_X11_PATH", "libxkbcommon-x11.so.0"),
            ("AGENTERM_BUNDLED_XCB_XKB_PATH", "libxcb-xkb.so.1"),
        ] {
            let vendor = format!("vendor/linux/{arch}/{soname}");
            println!("cargo:rerun-if-changed={vendor}");
            let bytes = std::fs::read(&vendor).unwrap_or_else(|error| {
                panic!("missing bundled Linux XKB library at {vendor}: {error}")
            });
            assert!(
                !bytes.is_empty(),
                "bundled Linux XKB library at {vendor} is empty"
            );
            println!("cargo:rustc-env={environment}={vendor}");
        }
    }
}
