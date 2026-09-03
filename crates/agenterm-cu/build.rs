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
