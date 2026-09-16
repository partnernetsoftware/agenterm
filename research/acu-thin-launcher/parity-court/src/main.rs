//! Paired black-box parity court for the ACU monolith and thin launcher.

use std::{
    env,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STREAM_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    argv: &'static [&'static str],
    expectation: Expectation,
}

#[derive(Clone, Copy)]
enum Expectation {
    Version,
    EntryModeRefusal,
    VerbsUsage,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    NativeMessagingInvocationRefusal,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    NativeMessagingOriginRefusal,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    PrivilegeBrokerRefusal,
    Capabilities,
    Refusal,
    #[cfg(target_os = "linux")]
    HostUnsupported,
}

const CASES: &[Case] = &[
    Case {
        name: "version",
        argv: &["--version"],
        expectation: Expectation::Version,
    },
    Case {
        name: "capabilities",
        argv: &["--target", "current", "--grant", "observe", "capabilities"],
        expectation: Expectation::Capabilities,
    },
    Case {
        name: "ordinary-refusal",
        argv: &["--target", "current", "capabilities"],
        expectation: Expectation::Refusal,
    },
    Case {
        name: "version-extra",
        argv: &["--version", "extra"],
        expectation: Expectation::EntryModeRefusal,
    },
    Case {
        name: "network-worker-extra",
        argv: &["--agenterm-cu-internal-network-probe-worker", "extra"],
        expectation: Expectation::EntryModeRefusal,
    },
    Case {
        name: "managed-job-owner-extra",
        argv: &["--agenterm-cu-internal-managed-job-owner", "extra"],
        expectation: Expectation::EntryModeRefusal,
    },
    Case {
        name: "device-lease-owner-extra",
        argv: &["--agenterm-cu-internal-device-lease-owner", "extra"],
        expectation: Expectation::EntryModeRefusal,
    },
    Case {
        name: "privilege-broker-extra",
        argv: &["--agenterm-cu-internal-privilege-broker", "extra"],
        expectation: Expectation::EntryModeRefusal,
    },
    Case {
        name: "verbs-help",
        argv: &["verbs", "--help"],
        expectation: Expectation::VerbsUsage,
    },
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Case {
        name: "native-messaging-parent-window-on-non-windows",
        argv: &[
            "chrome-extension://knofdkmmpkbnjhdkcjddbakbpmgpmjpe/",
            "--parent-window=1",
        ],
        expectation: Expectation::NativeMessagingInvocationRefusal,
    },
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Case {
        name: "native-messaging-foreign-origin",
        argv: &["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"],
        expectation: Expectation::NativeMessagingOriginRefusal,
    },
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Case {
        name: "privilege-broker-without-activation",
        argv: &["--agenterm-cu-internal-privilege-broker"],
        expectation: Expectation::PrivilegeBrokerRefusal,
    },
    #[cfg(target_os = "linux")]
    Case {
        name: "host-unsupported",
        argv: &["host"],
        expectation: Expectation::HostUnsupported,
    },
    #[cfg(target_os = "linux")]
    Case {
        name: "hotkeys-alias-unsupported",
        argv: &["hotkeys"],
        expectation: Expectation::HostUnsupported,
    },
];

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct Options {
    monolith: PathBuf,
    launcher: PathBuf,
    abi_library: PathBuf,
    bad_abi: PathBuf,
    selected_case: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("acu-thin-launcher-parity:{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args_os().skip(1))?;
    require_file("monolith", &options.monolith)?;
    require_file("launcher", &options.launcher)?;
    require_file("abi_library", &options.abi_library)?;
    require_file("bad_abi", &options.bad_abi)?;
    let monolith_identity = fs::canonicalize(&options.monolith)
        .map_err(|error| format!("canonicalize_failed:monolith:{error}"))?;
    let launcher_identity = fs::canonicalize(&options.launcher)
        .map_err(|error| format!("canonicalize_failed:launcher:{error}"))?;
    if monolith_identity == launcher_identity {
        return Err("pair_not_distinct".to_owned());
    }

    let cases: Vec<&Case> = match options.selected_case.as_deref() {
        Some(name) => vec![
            CASES
                .iter()
                .find(|case| case.name == name)
                .ok_or_else(|| format!("unknown_case:{name}"))?,
        ],
        None => CASES.iter().collect(),
    };

    for case in cases {
        let monolith = observe(&options.monolith, &options.abi_library, case)?;
        let launcher = observe(&options.launcher, &options.abi_library, case)?;
        validate_expected(case, &monolith).map_err(|error| format!("monolith:{error}"))?;
        validate_expected(case, &launcher).map_err(|error| format!("launcher:{error}"))?;
        if monolith != launcher {
            return Err(format!(
                "case_mismatch:{}\nmonolith={}\nlauncher={}",
                case.name,
                describe(&monolith),
                describe(&launcher)
            ));
        }
        println!("PASS {}", case.name);
    }
    validate_bad_abi_control(&options)?;
    Ok(())
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut monolith = None;
    let mut launcher = None;
    let mut abi_library = None;
    let mut bad_abi = None;
    let mut selected_case = None;
    let mut args = args;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--monolith") => monolith = Some(required_path(&mut args, "--monolith")?),
            Some("--launcher") => launcher = Some(required_path(&mut args, "--launcher")?),
            Some("--abi-library") => abi_library = Some(required_path(&mut args, "--abi-library")?),
            Some("--bad-abi") => bad_abi = Some(required_path(&mut args, "--bad-abi")?),
            Some("--case") => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing_value:--case".to_owned())?;
                selected_case = Some(
                    value
                        .into_string()
                        .map_err(|_| "case_not_utf8".to_owned())?,
                );
            }
            Some("--list") => {
                for case in CASES {
                    println!("{}", case.name);
                }
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unknown_argument:{other}")),
            None => return Err("argument_not_utf8".to_owned()),
        }
    }
    Ok(Options {
        monolith: monolith.ok_or_else(|| "missing_argument:--monolith".to_owned())?,
        launcher: launcher.ok_or_else(|| "missing_argument:--launcher".to_owned())?,
        abi_library: abi_library.ok_or_else(|| "missing_argument:--abi-library".to_owned())?,
        bad_abi: bad_abi.ok_or_else(|| "missing_argument:--bad-abi".to_owned())?,
        selected_case,
    })
}

fn required_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing_value:{flag}"))
}

fn require_file(label: &str, path: &Path) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("pair_incomplete:{label}"))
    }
}

fn observe(executable: &Path, abi_library: &Path, case: &Case) -> Result<Observation, String> {
    let mut child = Command::new(executable)
        .args(case.argv)
        .env("AGENTERM_NO_ACTIVATE", "1")
        .env("AGENTERM_ABI_LIB", abi_library)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn_failed:{}:{error}", case.name))?;
    let stdout = bounded_reader(
        child
            .stdout
            .take()
            .ok_or_else(|| format!("stdout_missing:{}", case.name))?,
    );
    let stderr = bounded_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| format!("stderr_missing:{}", case.name))?,
    );
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Observation {
                    status,
                    stdout: join_reader(stdout, case, "stdout")?,
                    stderr: join_reader(stderr, case, "stderr")?,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("case_timeout:{}", case.name));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait_failed:{}:{error}", case.name));
            }
        }
    }
}

fn bounded_reader(
    mut reader: impl Read + Send + 'static,
) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(MAX_STREAM_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read_failed:{error}"))?;
        if bytes.len() as u64 > MAX_STREAM_BYTES {
            return Err("stream_too_large".to_owned());
        }
        Ok(bytes)
    })
}

fn join_reader(
    reader: thread::JoinHandle<Result<Vec<u8>, String>>,
    case: &Case,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("reader_panicked:{}:{stream}", case.name))?
        .map_err(|error| format!("reader_failed:{}:{stream}:{error}", case.name))
}

fn validate_expected(case: &Case, observation: &Observation) -> Result<(), String> {
    match case.expectation {
        Expectation::Version => {
            require(observation.status.success(), case, "version_exit")?;
            require(observation.stderr.is_empty(), case, "version_stderr")?;
            require(
                observation.stdout.starts_with(b"agenterm-cu ")
                    && observation.stdout.ends_with(b"\n")
                    && observation
                        .stdout
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count()
                        == 1,
                case,
                "version_stdout",
            )
        }
        Expectation::EntryModeRefusal => {
            validate_json_error(case, observation, 1, "argv_entry_mode_unsupported")
        }
        Expectation::VerbsUsage => validate_json_error(case, observation, 2, "usage"),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Expectation::NativeMessagingInvocationRefusal => {
            require(exit_code(observation) == Some(1), case, "native_exit")?;
            require(observation.stdout.is_empty(), case, "native_stdout")?;
            require(
                observation.stderr == b"browser_bridge_invocation_invalid\n",
                case,
                "native_stderr",
            )
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Expectation::NativeMessagingOriginRefusal => {
            require(exit_code(observation) == Some(1), case, "native_exit")?;
            require(observation.stdout.is_empty(), case, "native_stdout")?;
            require(
                observation.stderr == b"browser_bridge_origin_invalid\n",
                case,
                "native_stderr",
            )
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Expectation::PrivilegeBrokerRefusal => {
            require(exit_code(observation) == Some(3), case, "privilege_exit")?;
            require(observation.stdout.is_empty(), case, "privilege_stdout")?;
            require(
                observation.stderr == b"privilege_broker_transport_failed\n",
                case,
                "privilege_stderr",
            )
        }
        Expectation::Capabilities => {
            require(exit_code(observation) == Some(0), case, "capabilities_exit")?;
            require(observation.stderr.is_empty(), case, "capabilities_stderr")?;
            let value = parse_json(case, observation)?;
            require(
                value.get("ok").and_then(|value| value.as_bool()) == Some(true),
                case,
                "capabilities_ok",
            )?;
            require(
                value
                    .pointer("/data/mechanism")
                    .and_then(|value| value.as_str())
                    == Some("libagenterm"),
                case,
                "capabilities_mechanism",
            )
        }
        Expectation::Refusal => validate_json_error(case, observation, 1, "refused"),
        #[cfg(target_os = "linux")]
        Expectation::HostUnsupported => {
            require(exit_code(observation) == Some(1), case, "host_exit")?;
            require(observation.stdout.is_empty(), case, "host_stdout")?;
            require(
                observation.stderr == b"agenterm-cu host is not implemented on this platform yet\n",
                case,
                "host_stderr",
            )
        }
    }
}

fn validate_json_error(
    case: &Case,
    observation: &Observation,
    expected_exit: i32,
    expected_code: &str,
) -> Result<(), String> {
    require(
        exit_code(observation) == Some(expected_exit),
        case,
        "json_exit",
    )?;
    require(observation.stderr.is_empty(), case, "json_stderr")?;
    let value = parse_json(case, observation)?;
    require(
        value
            .pointer("/error/code")
            .and_then(|value| value.as_str())
            == Some(expected_code),
        case,
        "json_error_code",
    )
}

fn parse_json(case: &Case, observation: &Observation) -> Result<serde_json::Value, String> {
    serde_json::from_slice(&observation.stdout)
        .map_err(|error| format!("expected_json:{}:{error}", case.name))
}

fn validate_bad_abi_control(options: &Options) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "control_clock_invalid".to_owned())?
        .as_nanos();
    let stage = env::temp_dir().join(format!(
        "acu-thin-launcher-parity-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&stage).map_err(|error| format!("control_stage_create_failed:{error}"))?;
    let staged_launcher = stage.join(launcher_filename());
    let staged_provider = stage.join(provider_filename());
    let result = (|| {
        fs::copy(&options.launcher, &staged_launcher)
            .map_err(|error| format!("control_launcher_copy_failed:{error}"))?;
        fs::copy(&options.bad_abi, &staged_provider)
            .map_err(|error| format!("control_provider_copy_failed:{error}"))?;
        let case = CASES
            .iter()
            .find(|case| case.name == "version")
            .ok_or_else(|| "control_case_missing".to_owned())?;
        let monolith = observe(&options.monolith, &options.abi_library, case)?;
        validate_expected(case, &monolith)?;
        let launcher = observe(&staged_launcher, &options.abi_library, case)?;
        require(
            exit_code(&launcher) == Some(70),
            case,
            "bad_abi_boundary_exit",
        )?;
        require(launcher.stdout.is_empty(), case, "bad_abi_boundary_stdout")?;
        require(
            String::from_utf8_lossy(&launcher.stderr).contains("provider_abi_version_mismatch"),
            case,
            "bad_abi_boundary_stderr",
        )?;
        require(monolith != launcher, case, "bad_abi_control_diverges")?;
        println!("PASS bad-abi-control");
        Ok(())
    })();
    let cleanup =
        fs::remove_dir_all(&stage).map_err(|error| format!("control_stage_cleanup_failed:{error}"));
    result.and(cleanup)
}

#[cfg(target_os = "windows")]
fn provider_filename() -> &'static str {
    "agenterm-cu-provider.dll"
}

#[cfg(target_os = "windows")]
fn launcher_filename() -> &'static str {
    "acu-thin-launcher.exe"
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn launcher_filename() -> &'static str {
    "acu-thin-launcher"
}

#[cfg(target_os = "linux")]
fn provider_filename() -> &'static str {
    "agenterm-cu-provider.so"
}

#[cfg(target_os = "macos")]
fn provider_filename() -> &'static str {
    "agenterm-cu-provider.dylib"
}

fn require(condition: bool, case: &Case, fact: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(format!("expectation_failed:{}:{fact}", case.name))
    }
}

fn exit_code(observation: &Observation) -> Option<i32> {
    observation.status.code()
}

fn describe(observation: &Observation) -> String {
    format!(
        "exit={:?} stdout={} stderr={}",
        observation.status.code(),
        escaped(&observation.stdout),
        escaped(&observation.stderr)
    )
}

fn escaped(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_names_are_unique() {
        for (index, case) in CASES.iter().enumerate() {
            assert!(!CASES[..index].iter().any(|other| other.name == case.name));
        }
    }

    #[test]
    fn exact_entry_modes_have_runtime_negative_cases() {
        let expected = [
            "version-extra",
            "network-worker-extra",
            "managed-job-owner-extra",
            "device-lease-owner-extra",
            "privilege-broker-extra",
        ];
        assert!(
            expected
                .iter()
                .all(|name| CASES.iter().any(|case| case.name == *name))
        );
    }
}
