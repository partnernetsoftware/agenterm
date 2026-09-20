use std::path::PathBuf;

pub(crate) fn binary(name: &str) -> PathBuf {
    if let Some(directory) = std::env::var_os("AGENTERM_TEST_BIN_DIR") {
        let extension = if cfg!(windows) { ".exe" } else { "" };
        let filename = if name == "agenterm-com" && cfg!(windows) {
            "agenterm.com".to_owned()
        } else {
            format!("{name}{extension}")
        };
        return PathBuf::from(directory).join(filename);
    }
    match name {
        "agenterm" => PathBuf::from(env!("CARGO_BIN_EXE_agenterm")),
        "agenterm-com" => PathBuf::from(env!("CARGO_BIN_EXE_agenterm-com")),
        "agenterm-cc" => PathBuf::from(env!("CARGO_BIN_EXE_agenterm-cc")),
        _ => panic!("unknown relocated test binary: {name}"),
    }
}
