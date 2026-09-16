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
    /// Runs the bounded stateful lifetime sequence instead of the closed
    /// prompt-exit table. See `run_lifetime_parity`.
    lifetime_parity: bool,
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

    if options.lifetime_parity {
        return run_lifetime_parity(&options);
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
    let mut lifetime_parity = false;
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
            Some("--lifetime-parity") => lifetime_parity = true,
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
        lifetime_parity,
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

// ---------------------------------------------------------------------------
// Bounded stateful lifetime parity: arms L1-L7 (lifetime) and C1-C5 (child
// output), each executed once per side.
//
// The closed `CASES` table above owns prompt-exit, non-mutating cases, and its
// README states that a stateful or resident case needs its own isolation and
// containment design. This is that design: every child in this mode runs with
// HOME, XDG and the four AGENTERM_CU_* state paths redirected into one
// repository-local run directory, every step has a per-step deadline inside one
// whole-sequence wall, and a failure tears both jobs down and reports residual
// pids. Only normalized fields are compared; ids, generations, pids and
// timestamps never are.
// ---------------------------------------------------------------------------

const LIFETIME_STEP_TIMEOUT: Duration = Duration::from_secs(30);
const LIFETIME_WHOLE_TIMEOUT: Duration = Duration::from_secs(165);

/// Copied verbatim from the qjs managed-job smoke's deterministic child.
const LIFETIME_CHILD_SH: &str =
    "IFS= read -r line; printf 'OUT:%s\\n' \"$line\"; printf 'ERR:%s\\n' \"$line\" >&2; exit 7";
/// `base64("ping\n")`, the smoke's fixed input.
const LIFETIME_CHILD_INPUT: &str = "cGluZwo=";
/// `base64("OUT:ping\n")` and `base64("ERR:ping\n")`, derived from that input.
const LIFETIME_EXPECT_STDOUT: &str = "T1VUOnBpbmcK";
const LIFETIME_EXPECT_STDERR: &str = "RVJSOnBpbmcK";

/// Fields kept for one arm's projections. `state`/`status` shape is compared;
/// identities such as `job_id`, `generation` and `owner_pid` are not.
/// The session step projects only the stable shape; its random identity values
/// are never compared across sides.
/// The one session label both sides use. Fixed on purpose: a label is semantic,
/// not run identity, so its value can be compared across sides. Both sides also
/// share one `--request-id` per verb for the same reason -- each side keeps its
/// own isolated state directory, so an equal id can never replay the other
/// side's effect (that is what makes "same argv" hold).
const LIFETIME_SESSION_LABEL: &str = "lifetime-parity";
/// The session step projects the stable label value plus the presence of the
/// identity; `session_id` / `lease` themselves are shape-only and never compared.
const LIFETIME_KEYS_SESSION: &[&str] = &["label"];
const LIFETIME_KEYS_STATUS: &[&str] = &["state", "io_available", "live"];
/// The smoke's own io-closed bound (`scripts/qjs/cu-managed-job-smoke.qjs:216-225`):
/// `wait_owner_absent` polls the job this many times, this far apart, and L6
/// reuses that rule instead of a single read. `#172` showed why: one read right
/// after a successful `job-stop` catches a legal intermediate state -- the owner
/// is still answering, so `io_available` is `true` while `live` already says
/// `exited` (measured gap `created_at` → `terminal_at` = 159 ms).
const LIFETIME_IO_CLOSED_POLLS: usize = 100;
const LIFETIME_IO_CLOSED_INTERVAL: Duration = Duration::from_millis(100);
/// Teardown must be able to settle a job even when the **sequence** wall is
/// already spent -- otherwise a late failure would report a residual it never
/// tried to resolve. Each job therefore gets its own fixed, bounded convergence
/// budget (the same 100 × 100 ms shape as the L6 poll, i.e. ten seconds), and
/// every poll inside it still takes the sooner of `LIFETIME_STEP_TIMEOUT` and
/// that budget.
const LIFETIME_TEARDOWN_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const LIFETIME_KEYS_IDENTITY: &[&str] = &["state", "start_identity"];
const LIFETIME_KEYS_STOP: &[&str] = &["state"];
const LIFETIME_KEYS_WRITE: &[&str] = &["accepted_bytes", "delivery", "stdin_closed"];
const LIFETIME_KEYS_EVENTS: &[&str] = &[
    "stdout.bytes",
    "stdout.next_cursor",
    "stdout.data_base64",
    "stderr.bytes",
    "stderr.next_cursor",
    "stderr.data_base64",
];
const LIFETIME_KEYS_OUTPUT: &[&str] = &[
    "stream",
    "output.bytes",
    "output.next_cursor",
    "output.data_base64",
];
const LIFETIME_KEYS_WAIT: &[&str] = &["completed", "status.state.kind", "status.state.exit_code"];

#[derive(Clone)]
struct JobRef {
    label: &'static str,
    id: String,
    generation: String,
    owner_pid: Option<String>,
}

/// The identity a side's effect verbs must carry. It is obtained from the
/// session's own creation reply and is never minted by this harness.
struct SessionIdentity {
    id: String,
    lease: String,
}

struct LifetimeSide {
    name: &'static str,
    executable: PathBuf,
    abi_library: PathBuf,
    run_dir: PathBuf,
    /// `None` until `start_session` has read the identity back from the reply.
    session: Option<SessionIdentity>,
    jobs: Vec<JobRef>,
    /// Whole-sequence wall for this side: every step waits on the sooner of its
    /// own 30 s budget and this instant.
    whole_deadline: Instant,
    projections: Vec<(String, serde_json::Value)>,
}

impl LifetimeSide {
    fn new(
        name: &'static str,
        executable: PathBuf,
        abi_library: PathBuf,
        run_dir: PathBuf,
    ) -> Self {
        Self {
            name,
            executable,
            abi_library,
            run_dir,
            // Neither the label nor any identity is minted here: the label is a
            // fixed semantic value, and the identity exists only after the
            // session's own creation reply has been parsed (§2.2).
            session: None,
            jobs: Vec::new(),
            whole_deadline: Instant::now() + LIFETIME_WHOLE_TIMEOUT,
            projections: Vec::new(),
        }
    }

    fn env_vec(&self) -> Vec<(String, String)> {
        let join = |name: &str| self.run_dir.join(name).to_string_lossy().into_owned();
        vec![
            ("HOME".to_owned(), join("home")),
            ("XDG_DATA_HOME".to_owned(), join("data")),
            ("XDG_CONFIG_HOME".to_owned(), join("config")),
            ("AGENTERM_CU_AUDIT_PATH".to_owned(), join("cu-audit.jsonl")),
            (
                "AGENTERM_CU_RUNTIME_PATH".to_owned(),
                join("cu-runtime.json"),
            ),
            (
                "AGENTERM_CU_IDEMPOTENCY_PATH".to_owned(),
                join("cu-requests.json"),
            ),
            (
                "AGENTERM_CU_MANAGED_JOB_PATH".to_owned(),
                join("cu-managed-jobs.json"),
            ),
        ]
    }

    /// The effect-verb prefix. It requires the identity `start_session` read back
    /// from the creation reply; asking before that is a harness error, never a
    /// silently empty `--session`.
    fn mutation_prefix(&self, request: &str) -> Result<Vec<String>, String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "lifetime_session_not_established".to_owned())?;
        Ok(vec![
            "--target".to_owned(),
            "current".to_owned(),
            "--grant".to_owned(),
            "actuate".to_owned(),
            "--request-id".to_owned(),
            format!("{LIFETIME_SESSION_LABEL}-{request}"),
            "--session".to_owned(),
            session.id.clone(),
            "--session-lease".to_owned(),
            session.lease.clone(),
        ])
    }

    /// The identity a hand-built argv needs (the controls that assemble their own
    /// prefix), for the same reason: it must come from the creation reply.
    fn session_identity(&self) -> Result<(String, String), String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "lifetime_session_not_established".to_owned())?;
        Ok((session.id.clone(), session.lease.clone()))
    }

    /// Creates this side's session and reads the identity back from the reply.
    /// `session-start` must not carry the prefix: it mints what the prefix needs.
    fn start_session(&mut self) -> Result<(), String> {
        let argv = vec![
            "--target".to_owned(),
            "current".to_owned(),
            "--grant".to_owned(),
            "actuate".to_owned(),
            "session-start".to_owned(),
            "--label".to_owned(),
            LIFETIME_SESSION_LABEL.to_owned(),
            "--ttl-seconds".to_owned(),
            "120".to_owned(),
        ];
        let observation = self.observe_argv(&argv, "L1-session-start")?;
        let projection =
            lifetime_projection(&observation, "L1-session-start", LIFETIME_KEYS_SESSION)?;
        require_lifetime(
            projection.pointer("/exit").and_then(|value| value.as_i64()) == Some(0)
                && projection.pointer("/ok").and_then(|value| value.as_bool()) == Some(true),
            &format!("L1-session-start:positive_reply:{projection}"),
        )?;
        let value: serde_json::Value =
            serde_json::from_slice(&observation.stdout).map_err(|error| {
                format!(
                    "lifetime_expected_json:L1-session-start:{error}:stdout_summary={}",
                    bounded_sample(&observation.stdout)
                )
            })?;
        let id = value
            .pointer("/data/session_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "lifetime_session_id_missing:L1-session-start".to_owned())?
            .to_owned();
        let lease = value
            .pointer("/data/lease")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "lifetime_session_lease_missing:L1-session-start".to_owned())?
            .to_owned();
        // The label is semantic, not run identity: assert it exactly and let its
        // value cross sides. The ids and the lease stay shape-only (§3.2).
        require_lifetime(
            value
                .pointer("/data/label")
                .and_then(|value| value.as_str())
                == Some(LIFETIME_SESSION_LABEL),
            &format!("L1-session-start:label:{value}"),
        )?;
        self.projections.push((
            "L1-session-start".to_owned(),
            serde_json::json!({
                "identity_present": true,
                "label": LIFETIME_SESSION_LABEL,
            }),
        ));
        self.session = Some(SessionIdentity { id, lease });
        Ok(())
    }

    fn observe_argv(&self, argv: &[String], label: &str) -> Result<Observation, String> {
        observe_lifetime_step(
            &self.executable,
            &self.abi_library,
            &self.env_vec(),
            argv,
            label,
            LIFETIME_STEP_TIMEOUT,
            self.whole_deadline,
        )
    }

    /// Records one arm's normalized projection.
    /// The owner pid comes from the public census, exactly as the qjs smoke does
    /// (`job-list`, `cu-managed-job-smoke.qjs:311-320`), never from the spawn
    /// reply. Census rows carry run identity, so only the presence of an owner
    /// crosses sides.
    fn census_owner_pid(&mut self, job_id: &str) -> Result<String, String> {
        let mut census = observe_prefix();
        census.extend(["job-list".to_owned(), "--max".to_owned(), "64".to_owned()]);
        let observation = self.observe_argv(&census, "L3b-job-list")?;
        let value: serde_json::Value =
            serde_json::from_slice(&observation.stdout).map_err(|error| {
                format!(
                    "lifetime_expected_json:L3b-job-list:{error}:stdout_summary={}",
                    bounded_sample(&observation.stdout)
                )
            })?;
        require_lifetime(
            value.get("ok").and_then(|value| value.as_bool()) == Some(true),
            "L3b-job-list:positive_reply",
        )?;
        let rows = value
            .pointer("/data/jobs")
            .and_then(|value| value.as_array())
            .ok_or_else(|| "lifetime_census_missing:L3b-job-list".to_owned())?;
        // The census must name an owner: a harness that cannot find it may not
        // quietly skip L4/L7 and still compare equal.
        let owner = rows
            .iter()
            .find_map(|row| {
                if row.get("job_id").and_then(|value| value.as_str()) == Some(job_id) {
                    row.get("owner_pid")
                        .and_then(|value| value.as_i64())
                        .map(|value| value.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| format!("lifetime_census_owner_missing:L3b-job-list:{job_id}"))?;
        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == job_id) {
            job.owner_pid = Some(owner.clone());
        }
        self.projections.push((
            "L3b-job-list".to_owned(),
            serde_json::json!({ "owner_pid_present": true }),
        ));
        Ok(owner)
    }

    fn step(
        &mut self,
        name: &str,
        argv: &[String],
        keys: &[&str],
    ) -> Result<serde_json::Value, String> {
        let observation = self.observe_argv(argv, name)?;
        let projection = lifetime_projection(&observation, name, keys)?;
        require_lifetime(
            projection.pointer("/exit").and_then(|value| value.as_i64()) == Some(0)
                && projection.pointer("/ok").and_then(|value| value.as_bool()) == Some(true),
            &format!("{name}:positive_reply:{projection}"),
        )?;
        self.projections.push((name.to_owned(), projection.clone()));
        Ok(projection)
    }

    /// Settles one job's IO authority using the smoke's own bound.
    ///
    /// This is the rule `wait_owner_absent` applies directly after an explicit
    /// `job-stop` (`cu-managed-job-smoke.qjs:216-225`): up to
    /// `LIFETIME_IO_CLOSED_POLLS` fresh `job-status` polls, `LIFETIME_IO_CLOSED_INTERVAL`
    /// apart. Every poll must be a positive reply (`exit 0`, `ok: true`), and only
    /// `io_available === false` **together with** a `live` key that is **present
    /// and `null`** is the closed state.
    ///
    /// Both halves are read from the **raw** reply, because the projection folds a
    /// missing key into `null` and so cannot witness presence (`lifetime_project`).
    /// A poll that omits either field is also **not** an early failure: it is
    /// recorded as `missing` and the loop keeps polling, so a transient omission
    /// can never masquerade as convergence while the final diagnostic still says
    /// which half was missing. An intermediate observation is a diagnostic only --
    /// it never enters the cross-side projection.
    ///
    /// `deadline` is **explicit** so the two callers can differ: L6 hands in the
    /// sequence wall, teardown hands in its own fixed budget (`#174`), so an
    /// already-spent sequence wall can no longer stop teardown from cleaning up.
    /// Every poll still takes the sooner of `LIFETIME_STEP_TIMEOUT` and `deadline`.
    ///
    /// Borrows `&self` so teardown can settle a job without recording anything:
    /// the **caller** decides whether the returned (closed) projection becomes a
    /// compared step.
    fn wait_io_closed(
        &self,
        job_id: &str,
        label: &str,
        deadline: Instant,
    ) -> Result<serde_json::Value, String> {
        let mut last = String::from("no_poll_completed");
        for attempt in 0..LIFETIME_IO_CLOSED_POLLS {
            // The caller's deadline wins whenever it is earlier, so the loop can
            // never outlive the budget it was handed (§3.5).
            if Instant::now() >= deadline {
                return Err(format!(
                    "lifetime_io_closed_deadline:{label}:attempt={attempt}:last={last}"
                ));
            }
            let mut argv = observe_prefix();
            argv.extend(["job-status".to_owned(), job_id.to_owned()]);
            // Straight to the step observer: handing it `deadline` as the whole
            // bound is what lets teardown outlive a spent sequence wall.
            let observation = observe_lifetime_step(
                &self.executable,
                &self.abi_library,
                &self.env_vec(),
                &argv,
                label,
                LIFETIME_STEP_TIMEOUT,
                deadline,
            )?;
            let raw = lifetime_reply(&observation, label)?;
            let projection =
                lifetime_project(&raw, observation.status.code(), LIFETIME_KEYS_STATUS);
            require_lifetime(
                projection.pointer("/exit").and_then(|value| value.as_i64()) == Some(0)
                    && projection.pointer("/ok").and_then(|value| value.as_bool()) == Some(true),
                &format!("{label}:positive_reply:{projection}"),
            )?;
            // Presence and nullness of `live` come from the raw reply: the
            // projection would report a missing key as `null`.
            let live = raw.pointer("/data/live");
            let io_available = raw
                .pointer("/data/io_available")
                .and_then(|value| value.as_bool());
            if io_available == Some(false) && live.is_some_and(serde_json::Value::is_null) {
                return Ok(projection);
            }
            // Not closed yet: say exactly which half is not, and keep polling. A
            // missing field is a stall to report, never a silent success.
            let io_text = match io_available {
                Some(value) => value.to_string(),
                None => "missing_or_not_bool".to_owned(),
            };
            let live_text = match live {
                None => "missing",
                Some(value) if value.is_null() => "null",
                Some(_) => "not_null",
            };
            last = format!("io_available={io_text}:live={live_text}");
            thread::sleep(LIFETIME_IO_CLOSED_INTERVAL);
        }
        Err(format!(
            "lifetime_io_closed_timeout:{label}:polls={LIFETIME_IO_CLOSED_POLLS}:last={last}"
        ))
    }

    fn run_sequence(&mut self) -> Result<(), String> {
        fs::create_dir_all(self.run_dir.join("home"))
            .map_err(|error| format!("lifetime_run_dir_failed:{error}"))?;

        // L1: one session for both jobs. The creating verb carries no prefix and
        // its reply is the only source of this side's identity (§2.2).
        self.start_session()?;

        // L2: the lifetime job, whose deterministic child stays blocked on stdin.
        // `spawn_argv` observes once and records its own projection, so this arm
        // is a single product call.
        self.spawn_argv("L2")?;
        let job_l = self
            .jobs
            .last()
            .ok_or_else(|| "lifetime_job_missing:L".to_owned())?
            .clone();

        // L3/L4: ready, then the owner identity this side must later release.
        let mut status = observe_prefix();
        status.extend(["job-status".to_owned(), job_l.id.clone()]);
        let ready = self.step("L3-job-status", &status, LIFETIME_KEYS_STATUS)?;
        require_lifetime(
            ready
                .pointer("/data/state")
                .and_then(|value| value.as_str())
                == Some("running"),
            &format!("L3-job-status:running:{ready}"),
        )?;
        // L3b: the owner pid comes from the public census, never from the spawn
        // reply -- the qjs smoke reads it the same way. The census must name an
        // owner: a harness that cannot find it may not quietly skip L4/L7 and
        // still compare equal.
        let owner_pid = self.census_owner_pid(&job_l.id)?;
        // L4: the owner identity this side must later release. A live owner with
        // no identity is a harness failure, not a skip.
        let mut probe = observe_prefix();
        probe.extend([
            "process-state".to_owned(),
            "--pid".to_owned(),
            owner_pid.clone(),
        ]);
        let identity_projection = self.step("L4-process-state", &probe, LIFETIME_KEYS_IDENTITY)?;
        require_lifetime(
            identity_projection
                .pointer("/data/state")
                .and_then(|value| value.as_str())
                == Some("live"),
            &format!("L4-process-state:live:{identity_projection}"),
        )?;
        let identity = identity_projection
            .pointer("/data/start_identity")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "lifetime_start_identity_missing:L4-process-state".to_owned())?
            .to_owned();

        // L5: controlled termination.
        let mut stop = self.mutation_prefix("stop-l")?;
        stop.extend([
            "job-stop".to_owned(),
            job_l.id.clone(),
            job_l.generation.clone(),
        ]);
        self.step("L5-job-stop", &stop, LIFETIME_KEYS_STOP)?;

        // L6: the job's IO authority must be gone -- proved by the smoke's bounded
        // poll, never by a single read. Immediately after a successful `job-stop`
        // the owner is still answering, so one read catches a legal intermediate
        // state (`#172`: `io_available === true` with a `Some(exited)` `live`).
        // Only the final, closed observation becomes the compared step.
        // L6 runs inside the sequence budget, so the sequence wall is its bound.
        let l6_deadline = self.whole_deadline;
        let closed_projection = self.wait_io_closed(&job_l.id, "L6-job-status", l6_deadline)?;
        self.projections
            .push(("L6-job-status".to_owned(), closed_projection));

        // L7: the original identity must no longer be live. `absent` and
        // `reused` are normalized into one value, `released`; the two labels are
        // never compared with each other.
        let mut released_probe = observe_prefix();
        released_probe.extend([
            "process-state".to_owned(),
            "--pid".to_owned(),
            owner_pid.clone(),
        ]);
        let released = self.step("L7-process-state", &released_probe, LIFETIME_KEYS_IDENTITY)?;
        let live_with_same_identity = released
            .pointer("/data/state")
            .and_then(|value| value.as_str())
            == Some("live")
            && released
                .pointer("/data/start_identity")
                .and_then(|value| value.as_str())
                == Some(identity.as_str());
        require_lifetime(
            !live_with_same_identity,
            &format!("L7-process-state:original_identity_still_live:{released}"),
        )?;
        // L7 enters the cross-side comparison as one normalized value only:
        // `absent` and `reused` both mean the original identity is no longer
        // running (`released`). The raw classification never crosses sides,
        // because a reused pid is a different process and demanding the same
        // label would be a false red.
        if let Some(last) = self.projections.last_mut() {
            last.1 = serde_json::json!({ "released": true });
        }

        // C1: the child-output job (again one product call).
        self.spawn_argv("C1")?;
        let job_c = self
            .jobs
            .last()
            .ok_or_else(|| "lifetime_job_missing:C".to_owned())?
            .clone();

        // C2: the fixed five bytes, then close stdin so the child can finish.
        let mut write = self.mutation_prefix("write-c")?;
        write.extend([
            "job-write".to_owned(),
            job_c.id.clone(),
            job_c.generation.clone(),
            "--data-base64".to_owned(),
            LIFETIME_CHILD_INPUT.to_owned(),
            "--close-stdin".to_owned(),
        ]);
        let wrote = self.step("C2-job-write", &write, LIFETIME_KEYS_WRITE)?;
        require_lifetime(
            wrote
                .pointer("/data/accepted_bytes")
                .and_then(|value| value.as_u64())
                == Some(5)
                && wrote
                    .pointer("/data/stdin_closed")
                    .and_then(|value| value.as_bool())
                    == Some(true),
            "C2-job-write",
        )?;

        // C3/C4: the child's stdout and stderr, through the two output verbs.
        let mut events = observe_prefix();
        events.extend([
            "job-events".to_owned(),
            job_c.id.clone(),
            job_c.generation.clone(),
            "--stdout-cursor".to_owned(),
            "0".to_owned(),
            "--stderr-cursor".to_owned(),
            "0".to_owned(),
            "--timeout-ms".to_owned(),
            "10000".to_owned(),
            "--max-bytes".to_owned(),
            "4096".to_owned(),
        ]);
        let observed = self.step("C3-job-events", &events, LIFETIME_KEYS_EVENTS)?;
        require_lifetime(
            observed
                .pointer("/data/stdout/data_base64")
                .and_then(|value| value.as_str())
                == Some(LIFETIME_EXPECT_STDOUT)
                && observed
                    .pointer("/data/stderr/data_base64")
                    .and_then(|value| value.as_str())
                    == Some(LIFETIME_EXPECT_STDERR),
            "C3-job-events",
        )?;
        let mut output = observe_prefix();
        output.extend([
            "job-output".to_owned(),
            job_c.id.clone(),
            job_c.generation.clone(),
            "--stream".to_owned(),
            "stderr".to_owned(),
            "--cursor".to_owned(),
            "0".to_owned(),
            "--max-bytes".to_owned(),
            "4096".to_owned(),
        ]);
        let single = self.step("C4-job-output", &output, LIFETIME_KEYS_OUTPUT)?;
        require_lifetime(
            single
                .pointer("/data/stream")
                .and_then(|value| value.as_str())
                == Some("stderr")
                && single
                    .pointer("/data/output/data_base64")
                    .and_then(|value| value.as_str())
                    == Some(LIFETIME_EXPECT_STDERR),
            "C4-job-output",
        )?;

        // C5: the child's own exit status.
        let mut wait = observe_prefix();
        wait.extend([
            "job-wait".to_owned(),
            job_c.id.clone(),
            job_c.generation.clone(),
            "--timeout-ms".to_owned(),
            "10000".to_owned(),
            "--expect-exit".to_owned(),
            "7".to_owned(),
        ]);
        let waited = self.step("C5-job-wait", &wait, LIFETIME_KEYS_WAIT)?;
        require_lifetime(
            waited
                .pointer("/data/completed")
                .and_then(|value| value.as_bool())
                == Some(true)
                && waited
                    .pointer("/data/status/state/exit_code")
                    .and_then(|value| value.as_i64())
                    == Some(7),
            "C5-job-wait",
        )?;
        Ok(())
    }

    fn spawn_argv(&mut self, arm: &str) -> Result<(), String> {
        let argv = lifetime_spawn_argv(self, &format!("spawn-{arm}"))?;
        let arm_label = format!("{arm}-job-spawn");
        let observation = self.observe_argv(&argv, &arm_label)?;
        // The spawn reply carries identity only -- this arm's contract is
        // `exit0 + ok:true + identity shape`, so nothing else is projected (no
        // status/io/live that this reply does not own).
        let projection = lifetime_projection(&observation, &arm_label, &[])?;
        require_lifetime(
            projection.pointer("/exit").and_then(|value| value.as_i64()) == Some(0)
                && projection.pointer("/ok").and_then(|value| value.as_bool()) == Some(true),
            &format!("{arm_label}:positive_reply:{projection}"),
        )?;
        let value: serde_json::Value =
            serde_json::from_slice(&observation.stdout).map_err(|error| {
                format!(
                    "lifetime_expected_json:{arm_label}:{error}:stdout_summary={}",
                    bounded_sample(&observation.stdout)
                )
            })?;
        let id = value
            .pointer("/data/job_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("lifetime_job_id_missing:{arm}"))?
            .to_owned();
        let generation = value
            .pointer("/data/generation")
            .and_then(|value| value.as_i64())
            .map(|value| value.to_string())
            .ok_or_else(|| format!("lifetime_generation_missing:{arm}"))?;
        // Record the job **before** any further assertion, so a later refusal can
        // still tear this job down. The owner pid is deliberately not read from
        // this reply: it comes from the public census (`census_owner_pid`).
        self.jobs.push(JobRef {
            label: if arm == "L2" { "L" } else { "C" },
            id: id.clone(),
            generation: generation.clone(),
            owner_pid: None,
        });
        self.projections
            .push((arm_label.clone(), projection.clone()));
        require_lifetime(
            !id.is_empty()
                && generation
                    .parse::<i64>()
                    .map(|value| value > 0)
                    .unwrap_or(false),
            &format!("{arm_label}:identity_shape:{projection}"),
        )?;
        Ok(())
    }

    /// Best-effort teardown of both jobs; reports every residual it could not
    /// resolve instead of swallowing it.
    fn teardown(&mut self) -> String {
        let mut residuals = Vec::new();
        for job in &self.jobs {
            // `teardown` reports instead of returning, so a missing session is a
            // residual here rather than an early return.
            let mut stop = match self.mutation_prefix("teardown-stop") {
                Ok(argv) => argv,
                Err(error) => {
                    residuals.push(format!("{}:prefix_failed:{error}", job.label));
                    continue;
                }
            };
            stop.extend([
                "job-stop".to_owned(),
                job.id.clone(),
                job.generation.clone(),
            ]);
            // Each job gets **its own** budget for the whole cleanup -- the stop
            // call and the settlement poll together: the sequence wall may already
            // be spent when a late failure brings us here, and an exhausted wall
            // must never be the reason a job is left uncleaned (`#174`). Both calls
            // therefore go straight to the step observer with that deadline as
            // their whole bound, each still capped by `LIFETIME_STEP_TIMEOUT`.
            let settle_deadline = Instant::now() + LIFETIME_TEARDOWN_SETTLE_TIMEOUT;
            if let Err(error) = observe_lifetime_step(
                &self.executable,
                &self.abi_library,
                &self.env_vec(),
                &stop,
                "teardown-stop",
                LIFETIME_STEP_TIMEOUT,
                settle_deadline,
            ) {
                residuals.push(format!("{}:stop_failed:{error}", job.label));
            }
            // Settle with the same bounded convergence L6 uses: a brief legitimate
            // `live` right after a successful stop must not be reported as a
            // residual. Teardown records **no** cross-side projection -- the closed
            // projection is deliberately discarded here.
            if let Err(error) = self.wait_io_closed(&job.id, "teardown-status", settle_deadline) {
                residuals.push(format!("{}:{error}", job.label));
            }
        }
        let _ = self.name;
        if residuals.is_empty() {
            "none".to_owned()
        } else {
            residuals.join(";")
        }
    }
}

fn observe_prefix() -> Vec<String> {
    vec![
        "--target".to_owned(),
        "current".to_owned(),
        "--grant".to_owned(),
        "observe".to_owned(),
    ]
}

/// The one spawn argv for every arm: the same fixed blocking child and the same
/// resource flags, built in one place so a second call site cannot drift.
fn lifetime_spawn_argv(side: &LifetimeSide, request: &str) -> Result<Vec<String>, String> {
    let mut argv = side.mutation_prefix(request)?;
    argv.extend([
        "job-spawn".to_owned(),
        "--cwd".to_owned(),
        side.run_dir.to_string_lossy().into_owned(),
        "--env".to_owned(),
        "AGENTERM_JOB_SMOKE_PRIVATE=not-for-logs".to_owned(),
        "--ttl-seconds".to_owned(),
        "10".to_owned(),
        "--cpu-seconds".to_owned(),
        "60".to_owned(),
        "--max-processes".to_owned(),
        "32".to_owned(),
    ]);
    if cfg!(target_os = "windows") || !cfg!(target_os = "macos") {
        argv.extend(["--memory-bytes".to_owned(), "536870912".to_owned()]);
    }
    if !cfg!(target_os = "windows") {
        argv.extend([
            "--file-size-bytes".to_owned(),
            "67108864".to_owned(),
            "--max-open-files".to_owned(),
            "256".to_owned(),
        ]);
    }
    argv.push("--".to_owned());
    argv.extend([
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        LIFETIME_CHILD_SH.to_owned(),
    ]);
    Ok(argv)
}

fn observe_lifetime_step(
    executable: &Path,
    abi_library: &Path,
    env: &[(String, String)],
    argv: &[String],
    label: &str,
    per_step: Duration,
    whole_deadline: Instant,
) -> Result<Observation, String> {
    let mut command = Command::new(executable);
    command
        .args(argv)
        .env("AGENTERM_NO_ACTIVATE", "1")
        .env("AGENTERM_ABI_LIB", abi_library)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("lifetime_spawn_failed:{label}:{error}"))?;
    let stdout = bounded_reader(
        child
            .stdout
            .take()
            .ok_or_else(|| format!("lifetime_stdout_missing:{label}"))?,
    );
    let stderr = bounded_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| format!("lifetime_stderr_missing:{label}"))?,
    );
    let deadline = std::cmp::min(Instant::now() + per_step, whole_deadline);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Observation {
                    status,
                    stdout: join_reader_label(stdout, label, "stdout")?,
                    stderr: join_reader_label(stderr, label, "stderr")?,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("lifetime_step_timeout:{label}"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("lifetime_wait_failed:{label}:{error}"));
            }
        }
    }
}

fn join_reader_label(
    reader: thread::JoinHandle<Result<Vec<u8>, String>>,
    label: &str,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("reader_panicked:{label}:{stream}"))?
        .map_err(|error| format!("reader_failed:{label}:{stream}:{error}"))
}

/// Keeps only the fields a parity claim may compare: the exit code, the reply's
/// `ok`/`command`/typed error code, and the whitelisted `data` paths. Identity
/// values are never included.
/// A bounded, printable sample of one stream: at most 200 escaped bytes plus the
/// true length, so a diagnostic can show what happened without copying a whole
/// 64 KiB stream into the error text.
fn bounded_sample(bytes: &[u8]) -> String {
    const SAMPLE: usize = 200;
    let head = &bytes[..bytes.len().min(SAMPLE)];
    format!("len={} sample={}", bytes.len(), escaped(head))
}

/// Parses one lifetime reply **once** and hands back the raw value, so a caller
/// can tell a key that is present-and-`null` from a key that is missing: the
/// projection below folds the missing case into `null`, which is fine for
/// comparison but wrong for a criterion that literally says "`null`".
fn lifetime_reply(observation: &Observation, label: &str) -> Result<serde_json::Value, String> {
    if !observation.stderr.is_empty() {
        return Err(format!(
            "lifetime_stderr_not_empty:{label}:{}",
            bounded_sample(&observation.stderr)
        ));
    }
    serde_json::from_slice(&observation.stdout).map_err(|error| {
        format!(
            "lifetime_expected_json:{label}:{error}:stdout_summary={}",
            bounded_sample(&observation.stdout)
        )
    })
}

/// Keeps only the fields a parity claim may compare: the exit code, the reply's
/// `ok`/`command`/typed error code, and the whitelisted `data` paths. Identity
/// values are never included.
///
/// A whitelisted key the reply omits becomes `null`, so this projection carries
/// **no** presence information -- a caller whose criterion needs "present and
/// `null`" must read the raw reply from `lifetime_reply` instead.
fn lifetime_project(
    raw: &serde_json::Value,
    exit: Option<i32>,
    keys: &[&str],
) -> serde_json::Value {
    let mut kept = serde_json::Map::new();
    for key in keys {
        let pointer = format!("/data/{}", key.replace('.', "/"));
        kept.insert(
            (*key).to_owned(),
            raw.pointer(&pointer)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::json!({
        "exit": exit,
        "ok": raw.get("ok").and_then(|value| value.as_bool()),
        "command": raw.get("command").and_then(|value| value.as_str()),
        "error_code": raw.pointer("/error/code").and_then(|value| value.as_str()),
        "data": serde_json::Value::Object(kept),
    })
}

fn lifetime_projection(
    observation: &Observation,
    label: &str,
    keys: &[&str],
) -> Result<serde_json::Value, String> {
    let raw = lifetime_reply(observation, label)?;
    Ok(lifetime_project(&raw, observation.status.code(), keys))
}

fn require_lifetime(condition: bool, fact: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(format!("lifetime_expectation_failed:{fact}"))
    }
}

fn run_lifetime_parity(options: &Options) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "lifetime_clock_invalid".to_owned())?
        .as_nanos();
    let root = env::current_dir()
        .map_err(|error| format!("lifetime_cwd_failed:{error}"))?
        .join("target/acu-thin-launcher/lifetime-parity")
        .join(format!("{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).map_err(|error| format!("lifetime_run_dir_failed:{error}"))?;

    let mut sides = Vec::new();
    for (name, executable) in [
        ("monolith", options.monolith.clone()),
        ("launcher", options.launcher.clone()),
    ] {
        let mut side = LifetimeSide::new(
            name,
            executable,
            options.abi_library.clone(),
            root.join(name),
        );
        match side.run_sequence() {
            Ok(()) => sides.push(side),
            Err(error) => {
                let residuals = side.teardown();
                return Err(format!(
                    "lifetime_side_failed:{name}:{error}:residuals[{residuals}]:run_root_kept={}",
                    root.display()
                ));
            }
        }
    }

    let mut failures = Vec::new();
    let left_labels: Vec<&str> = sides[0]
        .projections
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let right_labels: Vec<&str> = sides[1]
        .projections
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    if left_labels != right_labels {
        failures.push(format!(
            "lifetime_projection_sequence_mismatch\nmonolith={left_labels:?}\nlauncher={right_labels:?}"
        ));
    } else {
        // Equal label sequences mean equal lengths, so the pairs below cannot be
        // silently truncated by `zip`.
        for ((name, left), (_, right)) in
            sides[0].projections.iter().zip(sides[1].projections.iter())
        {
            if left != right {
                failures.push(format!(
                    "lifetime_parity_mismatch:{name}\nmonolith={left}\nlauncher={right}"
                ));
            } else {
                println!("PASS lifetime {name}");
            }
        }
    }

    // Three negative controls, each compared across the pair.
    failures.extend(lifetime_negative_controls(options, &root)?);

    if !failures.is_empty() {
        let mut residuals = Vec::new();
        for side in &mut sides {
            let name = side.name;
            residuals.push(format!("{name}:{}", side.teardown()));
        }
        return Err(format!(
            "{}\nresiduals[{}]\nrun_root_kept={}",
            failures.join("\n"),
            residuals.join(";"),
            root.display()
        ));
    }

    let mut residuals = Vec::new();
    for side in &mut sides {
        let name = side.name;
        residuals.push(format!("{name}:{}", side.teardown()));
    }
    println!("PASS lifetime-parity residuals[{}]", residuals.join(";"));
    validate_bad_abi_control(options)?;
    // Success removes the whole run root; a failure keeps it in place and names
    // it in the error instead.
    fs::remove_dir_all(&root).map_err(|error| format!("lifetime_run_cleanup_failed:{error}"))
}

/// The census row count on one side's own store, used only to prove that a
/// refused control added no job. It is deliberately not a cross-side projection.
fn census_row_count(
    side: &LifetimeSide,
    env: &[(String, String)],
    deadline: Instant,
) -> Result<usize, String> {
    let mut census = observe_prefix();
    census.extend(["job-list".to_owned(), "--max".to_owned(), "64".to_owned()]);
    let observation = observe_lifetime_step(
        &side.executable,
        &side.abi_library,
        env,
        &census,
        "census-row-count",
        LIFETIME_STEP_TIMEOUT,
        deadline,
    )?;
    let value: serde_json::Value =
        serde_json::from_slice(&observation.stdout).map_err(|error| {
            format!(
                "lifetime_expected_json:census-row-count:{error}:stdout_summary={}",
                bounded_sample(&observation.stdout)
            )
        })?;
    require_lifetime(
        observation.status.code() == Some(0)
            && value.get("ok").and_then(|value| value.as_bool()) == Some(true),
        "census-row-count:positive_reply",
    )?;
    // A malformed census is a failure, never an empty table.
    let rows = value
        .pointer("/data/jobs")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "lifetime_census_missing:census-row-count".to_owned())?;
    Ok(rows.len())
}

/// The three precommitted negative controls. Each runs on both sides and must
/// agree; the first two also require the pair to refuse.
fn lifetime_negative_controls(options: &Options, root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();

    // N1: the entry-mode refusal that already exists in the closed table.
    if let Some(case) = CASES
        .iter()
        .find(|case| case.name == "managed-job-owner-extra")
    {
        let monolith = observe(&options.monolith, &options.abi_library, case)?;
        let launcher = observe(&options.launcher, &options.abi_library, case)?;
        if monolith != launcher {
            failures.push(format!(
                "lifetime_negative_mismatch:managed-job-owner-extra\nmonolith={}\nlauncher={}",
                describe(&monolith),
                describe(&launcher)
            ));
        } else {
            println!("PASS lifetime negative managed-job-owner-extra");
        }
    } else {
        failures.push("lifetime_negative_missing:managed-job-owner-extra".to_owned());
    }

    // N2/N3: a refused spawn without the actuate grant, and a stop naming a job
    // that does not exist. Each is compared across the pair as a projection, so
    // no error code has to be guessed, and each must refuse on both sides.
    let mut n2 = Vec::new();
    let mut n3 = Vec::new();
    for (name, executable) in [
        ("monolith", options.monolith.clone()),
        ("launcher", options.launcher.clone()),
    ] {
        let run_dir = root.join(format!("negative-{name}"));
        fs::create_dir_all(run_dir.join("home"))
            .map_err(|error| format!("lifetime_run_dir_failed:{error}"))?;
        let mut side = LifetimeSide::new(name, executable, options.abi_library.clone(), run_dir);
        let env = side.env_vec();
        // Each call keeps its own 30 s budget inside a whole-sequence wall for
        // the controls too -- not one step's budget shared by five calls.
        let deadline = Instant::now() + LIFETIME_WHOLE_TIMEOUT;

        // N2: the same session and lease a real spawn would use, with the grant
        // downgraded to `observe`, so the only variable is the missing actuate
        // authority. A job that appears anyway is recorded and cleaned up. The
        // session step is the shared one, so the identity comes from its own
        // reply instead of being minted here (§2.2).
        side.start_session()?;
        let census_before = census_row_count(&side, &env, deadline)?;

        let (session_id, lease) = side.session_identity()?;
        let ungranted = vec![
            "--target".to_owned(),
            "current".to_owned(),
            "--grant".to_owned(),
            "observe".to_owned(),
            "--request-id".to_owned(),
            format!("{LIFETIME_SESSION_LABEL}-n2-spawn"),
            "--session".to_owned(),
            session_id,
            "--session-lease".to_owned(),
            lease,
            "job-spawn".to_owned(),
            "--cwd".to_owned(),
            side.run_dir.to_string_lossy().into_owned(),
            "--ttl-seconds".to_owned(),
            "10".to_owned(),
            "--".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "exit 0".to_owned(),
        ];
        let refused = observe_lifetime_step(
            &side.executable,
            &side.abi_library,
            &env,
            &ungranted,
            "N2-ungranted-spawn",
            LIFETIME_STEP_TIMEOUT,
            deadline,
        )?;
        let refused_value: serde_json::Value = serde_json::from_slice(&refused.stdout)
            .map_err(|error| format!("lifetime_expected_json:N2-ungranted-spawn:{error}"))?;
        // Judge the refusal without letting `?` return before any cleanup: the
        // projection stays a Result and is recorded at the end.
        let refusal_projection = lifetime_projection(&refused, "N2-ungranted-spawn", &[]);
        if refused_value.get("ok").and_then(|value| value.as_bool()) == Some(true) {
            // The authority was ignored: record the fact, then clean the job up.
            failures.push(format!("lifetime_negative_grant_ignored:N2:{name}"));
            match (
                refused_value
                    .pointer("/data/job_id")
                    .and_then(|value| value.as_str()),
                refused_value
                    .pointer("/data/generation")
                    .and_then(|value| value.as_i64()),
            ) {
                (Some(job_id), Some(generation)) => {
                    failures.push(format!(
                        "lifetime_negative_unexpected_job:N2:{name}:{job_id}:{generation}"
                    ));
                    let mut cleanup = side.mutation_prefix("n2-stop-unexpected")?;
                    cleanup.extend([
                        "job-stop".to_owned(),
                        job_id.to_owned(),
                        generation.to_string(),
                    ]);
                    // A cleanup failure is recorded; it never hides the finding
                    // above.
                    let cleaned = observe_lifetime_step(
                        &side.executable,
                        &side.abi_library,
                        &env,
                        &cleanup,
                        "N2-cleanup-stop",
                        LIFETIME_STEP_TIMEOUT,
                        deadline,
                    )
                    .ok()
                    .and_then(|cleaned| {
                        serde_json::from_slice::<serde_json::Value>(&cleaned.stdout).ok()
                    })
                    .and_then(|value| value.get("ok").and_then(|value| value.as_bool()));
                    if cleaned != Some(true) {
                        failures.push(format!("lifetime_negative_cleanup_failed:N2:{name}"));
                    }
                }
                _ => failures.push(format!(
                    "lifetime_negative_unexpected_job_unreadable:N2:{name}"
                )),
            }
        } else {
            let census_after = census_row_count(&side, &env, deadline)?;
            if census_after != census_before {
                failures.push(format!(
                    "lifetime_negative_census_drift:N2:{name}:{census_before}->{census_after}"
                ));
            }
        }
        match refusal_projection {
            Ok(projection) => n2.push(projection),
            Err(diagnostic) => {
                failures.push(format!("lifetime_negative_unjudged:N2:{name}:{diagnostic}"))
            }
        }

        // N3: a stop that names a **live** job with the wrong generation. The
        // control creates its own session and job through the shared session step
        // (so its identity also comes from the reply, never from this file), and
        // cleans the job up best-effort afterwards.
        side.start_session()?;

        // The control's job must be genuinely live: the same fixed blocking child
        // as the lifetime arm, never a command that exits immediately.
        let spawn = lifetime_spawn_argv(&side, "n3-spawn")?;
        let spawned = observe_lifetime_step(
            &side.executable,
            &side.abi_library,
            &env,
            &spawn,
            "N3-job-spawn",
            LIFETIME_STEP_TIMEOUT,
            deadline,
        )?;
        let spawned: serde_json::Value = serde_json::from_slice(&spawned.stdout)
            .map_err(|error| format!("lifetime_expected_json:N3-job-spawn:{error}"))?;
        require_lifetime(
            spawned.get("ok").and_then(|value| value.as_bool()) == Some(true),
            "N3-job-spawn:positive_reply",
        )?;
        let live_job = spawned
            .pointer("/data/job_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "lifetime_job_id_missing:N3-job-spawn".to_owned())?
            .to_owned();
        let live_generation = spawned
            .pointer("/data/generation")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| "lifetime_generation_missing:N3-job-spawn".to_owned())?;

        // Everything after the spawn runs inside one closure, so that no `?` can
        // return before the correct-generation cleanup below. The closure's
        // Result is recorded afterwards, together with the cleanup's outcome.
        let judged = (|| -> Result<serde_json::Value, String> {
            // The blocked child makes this job genuinely live; re-read it through
            // the public verb so that precondition is observable before the stop,
            // and confirm the reply names the same id.
            let mut live_status = observe_prefix();
            live_status.extend(["job-status".to_owned(), live_job.clone()]);
            let live = observe_lifetime_step(
                &side.executable,
                &side.abi_library,
                &env,
                &live_status,
                "N3-job-status",
                LIFETIME_STEP_TIMEOUT,
                deadline,
            )?;
            let live: serde_json::Value = serde_json::from_slice(&live.stdout)
                .map_err(|error| format!("lifetime_expected_json:N3-job-status:{error}"))?;
            require_lifetime(
                live.get("ok").and_then(|value| value.as_bool()) == Some(true)
                    && live.pointer("/data/state").and_then(|value| value.as_str())
                        == Some("running")
                    && live
                        .pointer("/data/job_id")
                        .and_then(|value| value.as_str())
                        == Some(live_job.as_str()),
                &format!("N3-job-status:live_precondition:{live}"),
            )?;

            let mut wrong = side.mutation_prefix("n3-stop-wrong")?;
            wrong.extend([
                "job-stop".to_owned(),
                live_job.clone(),
                (live_generation + 1).to_string(),
            ]);
            let refused = observe_lifetime_step(
                &side.executable,
                &side.abi_library,
                &env,
                &wrong,
                "N3-wrong-generation",
                LIFETIME_STEP_TIMEOUT,
                deadline,
            )?;
            lifetime_projection(&refused, "N3-wrong-generation", &[])
        })();

        // Unconditional cleanup with the correct generation, whatever happened
        // above. A cleanup failure is recorded; it never hides the original.
        let mut cleanup = side.mutation_prefix("n3-stop-cleanup")?;
        cleanup.extend([
            "job-stop".to_owned(),
            live_job.clone(),
            live_generation.to_string(),
        ]);
        let cleaned = observe_lifetime_step(
            &side.executable,
            &side.abi_library,
            &env,
            &cleanup,
            "N3-cleanup-stop",
            LIFETIME_STEP_TIMEOUT,
            deadline,
        )
        .ok()
        .and_then(|cleaned| serde_json::from_slice::<serde_json::Value>(&cleaned.stdout).ok())
        .and_then(|value| value.get("ok").and_then(|value| value.as_bool()));
        let cleanup_failed = cleaned != Some(true);
        if cleanup_failed {
            failures.push(format!("lifetime_negative_cleanup_failed:N3:{name}"));
        }

        match judged {
            Ok(projection) => n3.push(projection),
            Err(diagnostic) => failures.push(format!(
                "lifetime_negative_failed:N3:{name}:{diagnostic}:cleanup_ok={}",
                !cleanup_failed
            )),
        }
    }
    for (label, pair) in [("N2-ungranted-spawn", &n2), ("N3-wrong-generation", &n3)] {
        if pair.len() != 2 {
            failures.push(format!(
                "lifetime_negative_aborted:{label}:collected={}",
                pair.len()
            ));
            continue;
        }
        if pair[0] != pair[1] {
            failures.push(format!(
                "lifetime_negative_mismatch:{label}\nmonolith={}\nlauncher={}",
                pair[0], pair[1]
            ));
        } else if pair[0].pointer("/ok").and_then(|value| value.as_bool()) != Some(false) {
            failures.push(format!("lifetime_negative_not_refused:{label}"));
        } else {
            println!("PASS lifetime negative {label}");
        }
    }
    Ok(failures)
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
