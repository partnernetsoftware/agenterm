//! Global flags (target selection, the ssh / vnc / rdp transports and the
//! authorization sources) and the executor assembly they drive.

use std::path::PathBuf;

use agenterm_cu::{
    Authorization, Command, CuError, CuReply, Executor, RdpEndpoint, SshEndpoint, TargetRef,
    VncEndpoint,
};

use super::{command_error, help, usage_err};

#[derive(Default)]
pub struct Globals {
    pub grant: Option<String>,
    pub grant_id: Option<String>,
    pub grant_store: Option<PathBuf>,
    pub target: Option<TargetRef>,
    pub ssh_dest: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_identity: Option<PathBuf>,
    pub ssh_cu: Option<PathBuf>,
    pub ssh_env: Vec<(String, String)>,
    pub vnc_dest: Option<String>,
    pub vnc_port: Option<u16>,
    pub vnc_cu: Option<PathBuf>,
    pub vnc_env: Vec<(String, String)>,
    pub rdp_dest: Option<String>,
}

impl Globals {
    /// Consume the leading global flags; stops at the first non-flag token.
    /// `Err` is the reply to print (a usage error, or the help reply for a
    /// leading `--help`).
    pub fn parse(args: &mut Vec<String>) -> Result<Self, Box<CuReply>> {
        let mut globals = Self::default();
        while let Some(flag) = args.first() {
            match flag.as_str() {
                "--help" | "-h" => return Err(Box::new(help::run_help(&[]))),
                "--target" => {
                    let value = take_value(args, "--target");
                    globals.target = TargetRef::parse(&value);
                    if globals.target.is_none() {
                        return Err(Box::new(usage_err(
                            "unknown --target value; supported: 'current', 'ssh', 'vnc', and 'rdp'",
                        )));
                    }
                }
                "--ssh" => {
                    let value = take_value(args, "--ssh");
                    if value.is_empty() {
                        return Err(Box::new(usage_err("--ssh requires <user@host>")));
                    }
                    globals.ssh_dest = Some(value);
                    if globals.target.is_none() {
                        globals.target = Some(TargetRef::Ssh);
                    }
                }
                "--ssh-port" => {
                    let value = take_value(args, "--ssh-port");
                    match value.parse::<u16>() {
                        Ok(port) => globals.ssh_port = Some(port),
                        Err(_) => {
                            return Err(Box::new(usage_err(
                                "--ssh-port requires a TCP port number",
                            )));
                        }
                    }
                }
                "--ssh-identity" => {
                    let value = take_value(args, "--ssh-identity");
                    if value.is_empty() {
                        return Err(Box::new(usage_err(
                            "--ssh-identity requires a private-key path",
                        )));
                    }
                    globals.ssh_identity = Some(PathBuf::from(value));
                }
                "--ssh-cu" => {
                    let value = take_value(args, "--ssh-cu");
                    if value.is_empty() {
                        return Err(Box::new(usage_err(
                            "--ssh-cu requires a remote agenterm-cu path",
                        )));
                    }
                    globals.ssh_cu = Some(PathBuf::from(value));
                }
                "--ssh-env" => {
                    let value = take_value(args, "--ssh-env");
                    let Some((key, val)) = value.split_once('=') else {
                        return Err(Box::new(usage_err("--ssh-env requires KEY=VAL")));
                    };
                    if key.is_empty() {
                        return Err(Box::new(usage_err("--ssh-env requires a non-empty KEY")));
                    }
                    globals.ssh_env.push((key.to_owned(), val.to_owned()));
                }
                "--vnc" => {
                    let value = take_value(args, "--vnc");
                    if value.is_empty() {
                        return Err(Box::new(usage_err("--vnc requires <host[:port]>")));
                    }
                    globals.vnc_dest = Some(value);
                    if globals.target.is_none() {
                        globals.target = Some(TargetRef::Vnc);
                    }
                }
                "--vnc-port" => {
                    let value = take_value(args, "--vnc-port");
                    match value.parse::<u16>() {
                        Ok(port) => globals.vnc_port = Some(port),
                        Err(_) => {
                            return Err(Box::new(usage_err(
                                "--vnc-port requires a TCP port number",
                            )));
                        }
                    }
                }
                "--vnc-cu" => {
                    let value = take_value(args, "--vnc-cu");
                    if value.is_empty() {
                        return Err(Box::new(usage_err(
                            "--vnc-cu requires an agenterm-cu path for the session worker",
                        )));
                    }
                    globals.vnc_cu = Some(PathBuf::from(value));
                }
                "--vnc-env" => {
                    let value = take_value(args, "--vnc-env");
                    let Some((key, val)) = value.split_once('=') else {
                        return Err(Box::new(usage_err("--vnc-env requires KEY=VAL")));
                    };
                    if key.is_empty() {
                        return Err(Box::new(usage_err("--vnc-env requires a non-empty KEY")));
                    }
                    globals.vnc_env.push((key.to_owned(), val.to_owned()));
                }
                "--rdp" => {
                    let value = take_value(args, "--rdp");
                    if value.is_empty() {
                        return Err(Box::new(usage_err("--rdp requires <host[:port]>")));
                    }
                    globals.rdp_dest = Some(value);
                    if globals.target.is_none() {
                        globals.target = Some(TargetRef::Rdp);
                    }
                }
                "--grant" => {
                    if globals.grant.is_some() {
                        return Err(Box::new(usage_err("duplicate --grant")));
                    }
                    globals.grant = Some(take_value(args, "--grant"));
                }
                "--grant-id" => {
                    if globals.grant_id.is_some() {
                        return Err(Box::new(usage_err("duplicate --grant-id")));
                    }
                    let value = take_value(args, "--grant-id");
                    if !agenterm_cu::grant_management::valid_grant_id(&value) {
                        return Err(Box::new(usage_err("--grant-id is invalid")));
                    }
                    globals.grant_id = Some(value);
                }
                "--grant-store" => {
                    if globals.grant_store.is_some() {
                        return Err(Box::new(usage_err("duplicate --grant-store")));
                    }
                    let value = take_value(args, "--grant-store");
                    if value.is_empty() {
                        return Err(Box::new(usage_err("--grant-store requires a path")));
                    }
                    globals.grant_store = Some(PathBuf::from(value));
                }
                _ if flag.starts_with('-') => {
                    return Err(Box::new(usage_err(format!("unknown global flag '{flag}'"))));
                }
                _ => break,
            }
        }
        Ok(globals)
    }

    /// Re-spell the authorization flags for `exec`, which parses them itself
    /// so remote workers can be invoked as `--grant observe exec --json -`
    /// as well as `exec --grant=observe`.
    pub fn exec_args(&self, rest: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut exec_args: Vec<String> = Vec::new();
        if let Some(raw) = self.grant.as_ref() {
            exec_args.push(format!("--grant={raw}"));
        }
        if let Some(id) = self.grant_id.as_ref() {
            exec_args.push(format!("--grant-id={id}"));
        }
        if let Some(path) = self.grant_store.as_ref() {
            exec_args.push(format!("--grant-store={}", path.display()));
        }
        exec_args.extend(rest);
        exec_args
    }

    /// Apply the AGENTERM_CU_SSH / AGENTERM_CU_VNC fallbacks and refuse the
    /// transport combinations that cannot mean anything.
    pub fn resolve_target(&mut self) -> Result<TargetRef, Box<CuReply>> {
        if self.target.is_none()
            && let Ok(dest) = std::env::var("AGENTERM_CU_SSH")
            && !dest.is_empty()
        {
            self.ssh_dest = Some(dest);
            self.target = Some(TargetRef::Ssh);
        }
        if self.target.is_none()
            && let Ok(dest) = std::env::var("AGENTERM_CU_VNC")
            && !dest.is_empty()
        {
            self.vnc_dest = Some(dest);
            self.target = Some(TargetRef::Vnc);
        }
        let Some(target) = self.target else {
            return Err(Box::new(usage_err(
                "--target current, --ssh <user@host>, --vnc <host[:port]>, or --rdp <host[:port]> is required on every command",
            )));
        };
        let refused = match target {
            TargetRef::Current => [
                (
                    self.ssh_dest.is_some(),
                    "--ssh cannot be combined with --target current",
                ),
                (
                    self.vnc_dest.is_some(),
                    "--vnc cannot be combined with --target current",
                ),
                (
                    self.rdp_dest.is_some(),
                    "--rdp cannot be combined with --target current",
                ),
            ],
            TargetRef::Ssh => [
                (
                    self.vnc_dest.is_some(),
                    "--vnc cannot be combined with --target ssh / --ssh",
                ),
                (
                    self.rdp_dest.is_some(),
                    "--rdp cannot be combined with --target ssh / --ssh",
                ),
                (false, ""),
            ],
            TargetRef::Vnc => [
                (
                    self.ssh_dest.is_some(),
                    "--ssh cannot be combined with --target vnc / --vnc",
                ),
                (
                    self.rdp_dest.is_some(),
                    "--rdp cannot be combined with --target vnc / --vnc",
                ),
                (false, ""),
            ],
            TargetRef::Rdp => [
                (
                    self.ssh_dest.is_some(),
                    "--ssh cannot be combined with --target rdp / --rdp",
                ),
                (
                    self.vnc_dest.is_some(),
                    "--vnc cannot be combined with --target rdp / --rdp",
                ),
                (false, ""),
            ],
        };
        if let Some((_, message)) = refused.iter().find(|(hit, _)| *hit) {
            return Err(Box::new(usage_err(*message)));
        }
        if target == TargetRef::Ssh
            && self.ssh_dest.is_none()
            && let Ok(dest) = std::env::var("AGENTERM_CU_SSH")
            && !dest.is_empty()
        {
            self.ssh_dest = Some(dest);
        }
        if target == TargetRef::Ssh && self.ssh_dest.is_none() {
            return Err(Box::new(usage_err(
                "ssh target requires --ssh <user@host> (or AGENTERM_CU_SSH)",
            )));
        }
        if target == TargetRef::Vnc
            && self.vnc_dest.is_none()
            && let Ok(dest) = std::env::var("AGENTERM_CU_VNC")
            && !dest.is_empty()
        {
            self.vnc_dest = Some(dest);
        }
        if target == TargetRef::Vnc && self.vnc_dest.is_none() {
            return Err(Box::new(usage_err(
                "vnc target requires --vnc <host[:port]> (or AGENTERM_CU_VNC)",
            )));
        }
        // `--target rdp` without `--rdp` is not a usage error: the executor
        // returns typed `rdp_unavailable` with command/target preserved
        // (cut 3.46).
        Ok(target)
    }

    /// The executor for `command`: authorization first, then the transport
    /// endpoint the resolved target names.
    pub fn executor(
        &mut self,
        target: TargetRef,
        command: &Command,
        ambient_authority_present: bool,
        unsupported_authority_environment: bool,
    ) -> Result<Executor, Box<CuReply>> {
        let mut executor = authorize(
            self.grant.as_deref(),
            self.grant_id.take(),
            self.grant_store.take(),
            command,
            ambient_authority_present,
            unsupported_authority_environment,
        )?;
        if target == TargetRef::Ssh {
            let dest = self.ssh_dest.take().expect("ssh destination checked above");
            match SshEndpoint::from_parts(
                dest,
                self.ssh_port,
                self.ssh_identity.take(),
                self.ssh_cu.take(),
                std::mem::take(&mut self.ssh_env),
            ) {
                Ok(endpoint) => executor = executor.with_ssh(endpoint),
                Err(error) => return Err(Box::new(endpoint_error("ssh", error))),
            }
        }
        if target == TargetRef::Vnc {
            let dest = self.vnc_dest.take().expect("vnc destination checked above");
            match VncEndpoint::from_parts(
                dest,
                self.vnc_port,
                self.vnc_cu.take(),
                std::mem::take(&mut self.vnc_env),
            ) {
                Ok(endpoint) => executor = executor.with_vnc(endpoint),
                Err(error) => return Err(Box::new(endpoint_error("vnc", error))),
            }
        }
        if target == TargetRef::Rdp
            && let Some(dest) = self.rdp_dest.take()
        {
            match RdpEndpoint::from_parts(dest) {
                Ok(endpoint) => executor = executor.with_rdp(endpoint),
                Err(error) => return Err(Box::new(endpoint_error("rdp", error))),
            }
        }
        // No endpoint: Executor::execute_rdp returns rdp_unavailable.
        Ok(executor)
    }
}

fn endpoint_error(target: &str, error: CuError) -> CuReply {
    CuReply {
        ok: false,
        target: target.into(),
        command: "usage".into(),
        data: None,
        error: Some(error),
    }
}

/// Resolve the authorization sources into an executor: `--grant-id` is
/// exclusive with every other source, `--grant-store` needs `--grant-id`,
/// and an unsupported environment selector fails closed.
pub fn authorize(
    grant: Option<&str>,
    grant_id: Option<String>,
    grant_store: Option<PathBuf>,
    command: &Command,
    ambient_authority_present: bool,
    unsupported_authority_environment: bool,
) -> Result<Executor, Box<CuReply>> {
    if grant_id.is_some() && (grant.is_some() || ambient_authority_present) {
        return Err(Box::new(command_error(
            command,
            "invalid_authorization",
            "--grant-id cannot be combined with another authorization source",
        )));
    }
    if grant_id.is_none() && grant_store.is_some() {
        return Err(Box::new(usage_err(
            "--grant-store requires --grant-id for command execution",
        )));
    }
    if grant_id.is_none() && unsupported_authority_environment {
        return Err(Box::new(command_error(
            command,
            "invalid_authorization",
            "unsupported authorization environment selector is present",
        )));
    }
    let auth = if grant_id.is_some() {
        Authorization::new(Default::default())
    } else {
        resolve_authorization(grant).map_err(|error| {
            Box::new(CuReply {
                ok: false,
                target: command.target().as_str().into(),
                command: command.verb(),
                data: None,
                error: Some(error),
            })
        })?
    };
    let mut executor = Executor::new(auth);
    if let Some(grant_id) = grant_id {
        let store_path =
            match grant_store.map_or_else(agenterm_cu::auth_store::AuthStore::default_path, Ok) {
                Ok(path) => path,
                Err(_) => {
                    return Err(Box::new(command_error(
                        command,
                        "grant_store_unavailable",
                        "grant store is unavailable",
                    )));
                }
            };
        executor = executor.with_persisted_grant(grant_id, store_path);
    }
    Ok(executor)
}

/// `(any AGENTERM_CU_GRANT* / AGENTERM_CU_AUTH* selector present, one of
/// them is not the supported AGENTERM_CU_GRANT)`.
pub fn authority_environment_flags() -> (bool, bool) {
    let mut any = false;
    let mut unsupported = false;
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        let reserved = ["AGENTERM_CU_GRANT", "AGENTERM_CU_AUTH"]
            .iter()
            .any(|prefix| {
                key.get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            });
        if reserved {
            any = true;
            if !key.eq_ignore_ascii_case("AGENTERM_CU_GRANT") {
                unsupported = true;
            }
        }
    }
    (any, unsupported)
}

fn resolve_authorization(cli_grant: Option<&str>) -> Result<Authorization, CuError> {
    let environment_grant = if cli_grant.is_none() {
        std::env::var("AGENTERM_CU_GRANT").ok()
    } else {
        None
    };
    Authorization::try_from_sources(cli_grant, environment_grant.as_deref()).map_err(|error| {
        let problem = match error.kind {
            agenterm_cu::auth::GrantParseErrorKind::EmptyToken => "is empty",
            agenterm_cu::auth::GrantParseErrorKind::UnknownToken => "is unknown",
        };
        CuError::new(
            "invalid_authorization",
            format!("grant scope token {} {problem}", error.token_index),
        )
    })
}

fn take_value(args: &mut Vec<String>, flag: &str) -> String {
    args.remove(0);
    if args.is_empty() {
        eprintln!("missing value for {flag}");
        return String::new();
    }
    args.remove(0)
}
