use std::path::PathBuf;

fn main() {
    if std::env::var_os("AGENTERM_CU_PROVIDER_RESOURCE_CHILD").is_some() {
        let out_dir =
            PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"))
                .join("windows-resource");
        std::fs::create_dir_all(&out_dir).expect("create provider resource directory");
        winresource::WindowsResource::new()
            .set("ProductName", "AgenTerm")
            .set("ProductVersion", env!("CARGO_PKG_VERSION"))
            .set("FileDescription", "AgenTerm ACU qjs provider")
            .set("OriginalFilename", "agenterm-cu-provider.dll")
            .set_output_directory(out_dir.to_str().expect("resource path must be UTF-8"))
            .compile()
            .expect("failed to compile agenterm-cu-provider.dll VERSIONINFO");
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    println!("cargo:rerun-if-env-changed=RC_PATH");
    let output =
        std::process::Command::new(std::env::current_exe().expect("provider build script path"))
            .env("AGENTERM_CU_PROVIDER_RESOURCE_CHILD", "1")
            .output()
            .expect("provider resource child failed to run");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("failed to compile agenterm-cu-provider.dll resource child");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let resource_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("cargo:rustc-link-arg="))
        .map(str::trim)
        .expect("winresource emitted no provider resource link arg");
    println!("cargo:rustc-cdylib-link-arg={resource_path}");
}
