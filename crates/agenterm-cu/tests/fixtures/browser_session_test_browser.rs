use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    process::{self, Command},
};

const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";
const MODE_ENV: &str = "AGENTERM_CU_BROWSER_FIXTURE_MODE";
const PID_PATH_ENV: &str = "AGENTERM_CU_BROWSER_FIXTURE_PID_PATH";
const LAUNCH_PATH_ENV: &str = "AGENTERM_CU_BROWSER_FIXTURE_LAUNCH_PATH";
const IDENTITY_PATH_ENV: &str = "AGENTERM_CU_BROWSER_FIXTURE_IDENTITY_PATH";
const CU_EXE_ENV: &str = "AGENTERM_CU_BROWSER_FIXTURE_CU_EXE";

#[derive(Clone, Copy)]
enum Mode {
    Ready,
    Malformed,
    Oversize,
    EarlyExit,
}

impl Mode {
    fn from_environment() -> Result<Self, String> {
        match env::var(MODE_ENV).as_deref().unwrap_or("ready") {
            "ready" => Ok(Self::Ready),
            "malformed" => Ok(Self::Malformed),
            "oversize" => Ok(Self::Oversize),
            "early-exit" => Ok(Self::EarlyExit),
            value => Err(format!("unsupported {MODE_ENV} value: {value}")),
        }
    }
}

struct LaunchArguments {
    profile_root: PathBuf,
}

impl LaunchArguments {
    fn parse() -> Result<Self, String> {
        let mut profile_root = None;
        let mut remote_debugging = false;
        let mut no_first_run = false;
        let mut no_default_browser = false;
        let mut no_startup_window = false;

        for argument in env::args().skip(1) {
            if let Some(value) = argument.strip_prefix("--user-data-dir=") {
                if value.is_empty() || profile_root.is_some() {
                    return Err("--user-data-dir must appear exactly once and be non-empty".into());
                }
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err("--user-data-dir must be absolute".into());
                }
                profile_root = Some(path);
            } else if argument == "--remote-debugging-port=0" {
                require_once(&mut remote_debugging, &argument)?;
            } else if argument == "--no-first-run" {
                require_once(&mut no_first_run, &argument)?;
            } else if argument == "--no-default-browser-check" {
                require_once(&mut no_default_browser, &argument)?;
            } else if argument == "--no-startup-window" {
                require_once(&mut no_startup_window, &argument)?;
            } else {
                return Err(format!("unexpected browser argument: {argument}"));
            }
        }

        let profile_root = profile_root.ok_or("missing --user-data-dir")?;
        if !(remote_debugging && no_first_run && no_default_browser && no_startup_window) {
            return Err("missing one or more required production browser arguments".into());
        }
        let metadata = fs::symlink_metadata(&profile_root)
            .map_err(|error| format!("profile root is unavailable: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("profile root must be a direct directory".into());
        }
        Ok(Self { profile_root })
    }
}

fn require_once(seen: &mut bool, argument: &str) -> Result<(), String> {
    if *seen {
        return Err(format!("duplicate browser argument: {argument}"));
    }
    *seen = true;
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no file name"))?;
    let temporary = path.with_file_name(format!(".{name}.fixture-{}", process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn publish_pid() -> Result<(), String> {
    let Ok(value) = env::var(PID_PATH_ENV) else {
        return Ok(());
    };
    let path = PathBuf::from(value);
    write_atomic(&path, format!("{}\n", process::id()).as_bytes())
        .map_err(|error| format!("fixture pid publication failed: {error}"))
}

fn record_launch() -> Result<(), String> {
    let Ok(value) = env::var(LAUNCH_PATH_ENV) else {
        return Ok(());
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(value)
        .map_err(|error| format!("fixture launch journal open failed: {error}"))?;
    writeln!(file, "{}", process::id())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("fixture launch journal write failed: {error}"))
}

fn publish_identity() -> Result<(), String> {
    let Ok(identity_path) = env::var(IDENTITY_PATH_ENV) else {
        return Ok(());
    };
    let executable = env::var(CU_EXE_ENV)
        .map_err(|_| format!("{CU_EXE_ENV} is required with {IDENTITY_PATH_ENV}"))?;
    let output = Command::new(executable)
        .args([
            "--target",
            "current",
            "--grant",
            "observe",
            "process-state",
            "--pid",
            &process::id().to_string(),
        ])
        .output()
        .map_err(|error| format!("fixture identity observer failed to start: {error}"))?;
    if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains("\"ok\":true")
    {
        return Err("fixture identity observer did not return success".into());
    }
    write_atomic(Path::new(&identity_path), &output.stdout)
        .map_err(|error| format!("fixture identity publication failed: {error}"))
}

fn run() -> Result<(), String> {
    let arguments = LaunchArguments::parse()?;
    let mode = Mode::from_environment()?;
    record_launch()?;
    publish_pid()?;
    publish_identity()?;
    if matches!(mode, Mode::EarlyExit) {
        return Ok(());
    }

    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("loopback listener bind failed: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("loopback listener address failed: {error}"))?
        .port();
    let contents = match mode {
        Mode::Ready => format!("{port}\n/devtools/browser/agenterm-synthetic\n").into_bytes(),
        Mode::Malformed => b"not-a-port\n/devtools/browser/agenterm-synthetic\n".to_vec(),
        Mode::Oversize => vec![b'x'; 4097],
        Mode::EarlyExit => unreachable!(),
    };
    write_atomic(&arguments.profile_root.join(ACTIVE_PORT_FILE), &contents)
        .map_err(|error| format!("active port publication failed: {error}"))?;

    // The production owner owns this process tree and terminates it. Blocking
    // in the kernel avoids a fixture timer or polling loop becoming the court.
    loop {
        match listener.accept() {
            Ok((_stream, _peer)) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("loopback accept failed: {error}")),
        }
    }
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.as_slice() == ["--version"] {
        println!("AgenTerm synthetic browser fixture 1");
        return;
    }
    if let Err(error) = run() {
        eprintln!("browser-session-test-browser: {error}");
        process::exit(2);
    }
}
