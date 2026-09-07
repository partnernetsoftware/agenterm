//! Process inventory commands backed by the shared platform facade.

use agenterm_cu::{
    Command, TargetRef,
    command::{ProcessKillMode, ProcessPolicyAction, ProcessRunState, ProcessSignalKind},
};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, take_switch};

struct InspectionBounds {
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
}

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "process" {
        let expected = spec
            .aliases
            .iter()
            .find_map(|alias| alias.strip_prefix("process "))
            .ok_or_else(|| "process requires a subcommand".to_owned())?;
        if args.first().map(String::as_str) != Some(expected) {
            return Err(format!("process requires subcommand {expected}"));
        }
        args.remove(0);
    }
    match spec.name {
        "ps" => ps(target, args),
        "process-state" => process_state(target, args),
        "process-argv" => process_argv(target, args),
        "process-cwd" => process_cwd(target, args),
        "process-environment" => process_environment(target, args),
        "process-fds" => process_fds(target, args),
        "process-maps" => process_maps(target, args),
        "process-threads" => process_threads(target, args),
        "process-sockets" => process_sockets(target, args),
        "process-cgroup" => process_cgroup(target, args),
        "process-usage" => process_usage(target, args),
        "process-wait" => process_wait(target, args),
        "process-kill" => process_kill(target, args),
        "process-set-state" => process_set_state(target, args),
        "process-policy" => process_policy(target, args),
        "process-signal" => process_signal(target, args),
        "process-watch" => process_watch(target, args),
        "shell-exec" => shell_exec(target, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn inspection_pid(args: &mut Vec<String>, verb: &str) -> Result<u32, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| format!("{verb} PID must be a positive integer"))?,
        None => return Err(format!("{verb} requires --pid N (or positional PID)")),
    };
    if pid == 0 {
        return Err(format!("{verb} pid must be greater than zero"));
    }
    Ok(pid)
}

fn inspection_bounds(args: &mut Vec<String>) -> Result<InspectionBounds, String> {
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let limit = flag_parsed::<usize>(args, "--limit")?;
    let max_visited = flag_parsed::<usize>(args, "--max-visited")?;
    if offset.is_some_and(|value| value > 100_000) {
        return Err("process inspection --offset must be in 0..=100000".into());
    }
    if limit.is_some_and(|value| !(1..=5_000).contains(&value)) {
        return Err("process inspection --limit must be in 1..=5000".into());
    }
    if max_visited.is_some_and(|value| !(1..=10_000).contains(&value)) {
        return Err("process inspection --max-visited must be in 1..=10000".into());
    }
    Ok(InspectionBounds {
        offset,
        limit,
        max_visited,
    })
}

fn bounded_filter(value: Option<String>, name: &str, max: usize) -> Result<Option<String>, String> {
    if value.as_ref().is_some_and(|value| {
        value.is_empty()
            || value.len() > max
            || value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    }) {
        return Err(format!(
            "{name} must be 1..={max} UTF-8 bytes without NUL/CR/LF"
        ));
    }
    Ok(value)
}

fn process_fds(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-fds")?;
    let kind = bounded_filter(flag_text(args, "--type")?, "process-fds --type", 64)?;
    let target_filter = bounded_filter(
        flag_text(args, "--target-text")?,
        "process-fds --target-text",
        1024,
    )?;
    let InspectionBounds {
        offset,
        limit,
        max_visited,
    } = inspection_bounds(args)?;
    if !args.is_empty() {
        return Err(format!("process-fds received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessFds {
        target,
        pid,
        kind,
        target_filter,
        offset,
        limit,
        max_visited,
    })
}

fn process_maps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-maps")?;
    let path = bounded_filter(flag_text(args, "--path")?, "process-maps --path", 1024)?;
    let permissions = bounded_filter(
        flag_text(args, "--permissions")?,
        "process-maps --permissions",
        3,
    )?;
    if permissions.as_ref().is_some_and(|value| {
        value
            .bytes()
            .any(|byte| !matches!(byte, b'r' | b'w' | b'x'))
    }) {
        return Err("process-maps --permissions accepts only r/w/x".into());
    }
    let executable_only = take_switch(args, "--executable-only");
    let InspectionBounds {
        offset,
        limit,
        max_visited,
    } = inspection_bounds(args)?;
    if !args.is_empty() {
        return Err(format!("process-maps received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessMaps {
        target,
        pid,
        path,
        permissions,
        executable_only,
        offset,
        limit,
        max_visited,
    })
}

fn process_threads(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-threads")?;
    let name = bounded_filter(flag_text(args, "--name")?, "process-threads --name", 256)?;
    let state = bounded_filter(flag_text(args, "--state")?, "process-threads --state", 32)?;
    let InspectionBounds {
        offset,
        limit,
        max_visited,
    } = inspection_bounds(args)?;
    if !args.is_empty() {
        return Err(format!("process-threads received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessThreads {
        target,
        pid,
        name,
        state,
        offset,
        limit,
        max_visited,
    })
}

fn process_sockets(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-sockets")?;
    let family = bounded_filter(flag_text(args, "--family")?, "process-sockets --family", 32)?;
    let protocol = bounded_filter(
        flag_text(args, "--protocol")?,
        "process-sockets --protocol",
        32,
    )?;
    let state = bounded_filter(flag_text(args, "--state")?, "process-sockets --state", 32)?;
    let endpoint = bounded_filter(
        flag_text(args, "--endpoint")?,
        "process-sockets --endpoint",
        1024,
    )?;
    let InspectionBounds {
        offset,
        limit,
        max_visited,
    } = inspection_bounds(args)?;
    if !args.is_empty() {
        return Err(format!("process-sockets received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessSockets {
        target,
        pid,
        family,
        protocol,
        state,
        endpoint,
        offset,
        limit,
        max_visited,
    })
}

fn process_cgroup(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-cgroup")?;
    let start_identity = bounded_filter(
        flag_text(args, "--start-identity")?,
        "process-cgroup --start-identity",
        1024,
    )?;
    if !args.is_empty() {
        return Err(format!("process-cgroup received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessCgroup {
        target,
        pid,
        start_identity,
    })
}

fn process_signal(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-signal PID must be a positive integer".to_owned())?,
        None => return Err("process-signal requires --pid N (or positional PID)".into()),
    };
    if pid == 0 {
        return Err("process-signal pid must be greater than zero".into());
    }
    let signal = args
        .first()
        .and_then(|value| ProcessSignalKind::parse(value))
        .ok_or_else(|| {
            "process-signal requires HUP|INT|TERM|KILL|STOP|CONT|USR1|USR2".to_owned()
        })?;
    args.remove(0);
    let start_identity = flag_text(args, "--start-identity")?.filter(|value| !value.is_empty());
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(5_000);
    let force = take_switch(args, "--force");
    let tree = take_switch(args, "--tree");
    let max_was_explicit = args.iter().any(|value| value == "--max");
    let max_descendants = flag_parsed::<usize>(args, "--max")?.unwrap_or(500);
    if !(1..=60_000).contains(&timeout_ms) {
        return Err("process-signal --timeout-ms must be in 1..=60000".into());
    }
    if signal == ProcessSignalKind::Kill && !force {
        return Err("process-signal SIGKILL requires --force".into());
    }
    if force && signal != ProcessSignalKind::Kill {
        return Err("process-signal --force is valid only with SIGKILL".into());
    }
    if !(1..=10_000).contains(&max_descendants) {
        return Err("process-signal --max must be in 1..=10000".into());
    }
    if !tree && max_was_explicit {
        return Err("process-signal --max requires --tree".into());
    }
    if !args.is_empty() {
        return Err(format!("process-signal received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessSignal {
        target,
        pid,
        start_identity,
        signal,
        timeout_ms,
        force,
        tree,
        max_descendants,
    })
}

fn process_set_state(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-set-state PID must be a positive integer".to_owned())?,
        None => return Err("process-set-state requires --pid N (or positional PID)".into()),
    };
    if pid == 0 {
        return Err("process-set-state pid must be greater than zero".into());
    }
    let start_identity = flag_text(args, "--start-identity")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "process-set-state requires --start-identity ID from process-state".to_owned()
        })?;
    let state = args
        .first()
        .and_then(|value| ProcessRunState::parse(value))
        .ok_or_else(|| "process-set-state requires state running|stopped".to_owned())?;
    args.remove(0);
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(5_000);
    if !(1..=60_000).contains(&timeout_ms) {
        return Err("process-set-state --timeout-ms must be in 1..=60000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "process-set-state received unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ProcessSetState {
        target,
        pid,
        start_identity,
        state,
        timeout_ms,
    })
}

fn process_policy(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = inspection_pid(args, "process-policy")?;
    let start_identity = flag_text(args, "--start-identity")?.filter(|value| !value.is_empty());
    let action = args
        .first()
        .and_then(|value| ProcessPolicyAction::parse(value))
        .ok_or_else(|| "process-policy requires status|background|normal".to_owned())?;
    args.remove(0);
    if action.requested_background().is_some() && start_identity.is_none() {
        return Err(
            "process-policy background|normal requires --start-identity ID from process-state"
                .into(),
        );
    }
    if !args.is_empty() {
        return Err(format!("process-policy received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessPolicy {
        target,
        pid,
        action,
        start_identity,
    })
}

fn shell_exec(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let command = flag_text(args, "--command")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "shell-exec requires one non-empty --command TEXT".to_owned())?;
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(10_000);
    let max_output_bytes = flag_parsed::<usize>(args, "--max-output-bytes")?.unwrap_or(1_048_576);
    if command.len() > 131_072 || command.as_bytes().contains(&0) {
        return Err("shell-exec --command must be 1..=131072 UTF-8 bytes without NUL".into());
    }
    if !(100..=120_000).contains(&timeout_ms) {
        return Err("shell-exec --timeout-ms must be in 100..=120000".into());
    }
    if !(1..=16_777_216).contains(&max_output_bytes) {
        return Err("shell-exec --max-output-bytes must be in 1..=16777216".into());
    }
    if !args.is_empty() {
        return Err(format!("shell-exec received unexpected {:?}", args[0]));
    }
    Ok(Command::ShellExec {
        target,
        command,
        timeout_ms,
        max_output_bytes,
    })
}

fn process_cwd(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-cwd PID must be a positive integer".to_owned())?,
        None => return Err("process-cwd requires --pid N (or positional PID)".into()),
    };
    if pid == 0 {
        return Err("process-cwd pid must be greater than zero".into());
    }
    if !args.is_empty() {
        return Err(format!("process-cwd received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessCwd { target, pid })
}

fn process_environment(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-environment PID must be a positive integer".to_owned())?,
        None => return Err("process-environment requires --pid N (or positional PID)".into()),
    };
    let prefix = flag_text(args, "--prefix")?;
    let values = take_switch(args, "--values");
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let limit = flag_parsed::<usize>(args, "--limit")?;
    if pid == 0 {
        return Err("process-environment pid must be greater than zero".into());
    }
    if prefix.as_ref().is_some_and(|value| {
        value.len() > 256 || value.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n'))
    }) {
        return Err(
            "process-environment --prefix must be at most 256 UTF-8 bytes without NUL/CR/LF".into(),
        );
    }
    if offset.is_some_and(|value| value > 100_000) {
        return Err("process-environment --offset must be in 0..=100000".into());
    }
    if limit.is_some_and(|value| !(1..=5_000).contains(&value)) {
        return Err("process-environment --limit must be in 1..=5000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "process-environment received unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ProcessEnvironment {
        target,
        pid,
        prefix,
        values,
        offset,
        limit,
    })
}

fn process_argv(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-argv PID must be a positive integer".to_owned())?,
        None => return Err("process-argv requires --pid N (or positional PID)".into()),
    };
    let values = take_switch(args, "--values");
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let limit = flag_parsed::<usize>(args, "--limit")?;
    if pid == 0 {
        return Err("process-argv pid must be greater than zero".into());
    }
    if limit.is_some_and(|value| !(1..=4_096).contains(&value)) {
        return Err("process-argv --limit must be in 1..=4096".into());
    }
    if !args.is_empty() {
        return Err(format!("process-argv received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessArgv {
        target,
        pid,
        values,
        offset,
        limit,
    })
}

fn process_kill(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = match flag_parsed::<u32>(args, "--pid")? {
        Some(pid) => pid,
        None if !args.is_empty() && !args[0].starts_with('-') => args
            .remove(0)
            .parse::<u32>()
            .map_err(|_| "process-kill PID must be a positive integer".to_owned())?,
        None => return Err("process-kill requires --pid N (or positional PID)".into()),
    };
    if pid == 0 {
        return Err("process-kill --pid must be greater than zero".into());
    }
    let start_identity = flag_text(args, "--start-identity")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "process-kill requires --start-identity ID from process-state".to_owned())?;
    let mode = match flag_text(args, "--mode")? {
        Some(raw) => ProcessKillMode::parse(&raw).ok_or_else(|| {
            "process-kill --mode must be graceful|forceful (aliases: term|kill|SIGTERM|SIGKILL)"
                .to_owned()
        })?,
        None => ProcessKillMode::Forceful,
    };
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(30_000);
    let expect_exited = flag_text(args, "--expect")?.as_deref() == Some("exited");
    if !(1..=86_400_000).contains(&timeout_ms) {
        return Err("process-kill --timeout-ms must be in 1..=86400000".into());
    }
    if !expect_exited {
        return Err("process-kill requires --expect exited".into());
    }
    if !args.is_empty() {
        return Err(format!("process-kill received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessKill {
        target,
        pid,
        start_identity,
        mode,
        timeout_ms,
        expect_exited,
    })
}

fn process_watch(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let parent = flag_parsed::<u32>(args, "--parent")?;
    let name = flag_text(args, "--name")?;
    let all = take_switch(args, "--all");
    let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(30_000);
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    let max_processes = flag_parsed::<usize>(args, "--max-processes")?;
    if pid.is_none() && parent.is_none() && name.is_none() && !all {
        return Err("process-watch requires --pid N, --parent N, --name SUB or --all".into());
    }
    if pid == Some(0)
        || parent == Some(0)
        || name.as_deref().is_some_and(|value| value.trim().is_empty())
    {
        return Err("process-watch selectors must be positive or non-empty".into());
    }
    if !(1..=86_400_000).contains(&duration_ms)
        || interval_ms.is_some_and(|value| !(1..=60_000).contains(&value))
        || max_events.is_some_and(|value| !(1..=4_096).contains(&value))
        || max_processes.is_some_and(|value| !(1..=5_000).contains(&value))
    {
        return Err("process-watch requires duration-ms in 1..=86400000, interval-ms in 1..=60000, max-events in 1..=4096 and max-processes in 1..=5000".into());
    }
    if !args.is_empty() {
        return Err(format!("process-watch received unexpected {:?}", args[0]));
    }
    Ok(Command::ProcessWatch {
        target,
        pid,
        parent,
        name,
        all,
        duration_ms,
        interval_ms,
        max_events,
        max_processes,
    })
}

fn process_wait(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid_flag(args, "process-wait")?;
    let start_identity = flag_text(args, "--start-identity")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "process-wait requires --start-identity ID from process-state".to_owned())?;
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(30_000);
    if !(1..=86_400_000).contains(&timeout_ms) {
        return Err("process-wait --timeout-ms must be in 1..=86400000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "process-wait accepts only --pid N --start-identity ID --timeout-ms N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ProcessWait {
        target,
        pid,
        start_identity,
        timeout_ms,
    })
}

fn required_positive_pid_flag(args: &mut Vec<String>, command: &str) -> Result<u32, String> {
    let pid =
        flag_parsed::<u32>(args, "--pid")?.ok_or_else(|| format!("{command} requires --pid N"))?;
    if pid == 0 {
        return Err(format!("{command} --pid must be greater than zero"));
    }
    Ok(pid)
}

fn process_usage(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid_flag(args, "process-usage")?;
    let watch_ms = flag_parsed::<u64>(args, "--watch-ms")?;
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_samples = flag_parsed::<usize>(args, "--max-samples")?;
    if watch_ms.is_none() && (interval_ms.is_some() || max_samples.is_some()) {
        return Err("process-usage --interval-ms/--max-samples require --watch-ms N".into());
    }
    if watch_ms.is_some_and(|value| !(1..=86_400_000).contains(&value)) {
        return Err("process-usage --watch-ms must be in 1..=86400000".into());
    }
    if interval_ms.is_some_and(|value| !(1..=60_000).contains(&value)) {
        return Err("process-usage --interval-ms must be in 1..=60000".into());
    }
    if max_samples.is_some_and(|value| !(1..=4_096).contains(&value)) {
        return Err("process-usage --max-samples must be in 1..=4096".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "process-usage accepts only --pid N [--watch-ms N --interval-ms N --max-samples N]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ProcessUsage {
        target,
        pid,
        watch_ms,
        interval_ms,
        max_samples,
    })
}

fn required_positive_pid(args: &mut Vec<String>, command: &str) -> Result<u32, String> {
    let pid = required_positive_pid_flag(args, command)?;
    if !args.is_empty() {
        return Err(format!(
            "{command} accepts only --pid N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(pid)
}

fn process_state(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid(args, "process-state")?;
    Ok(Command::ProcessState { target, pid })
}

fn ps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let parent = flag_parsed::<u32>(args, "--parent")?;
    let name = flag_text(args, "--name")?;
    let app = flag_text(args, "--app")?;
    let command = flag_text(args, "--command")?;
    for (flag, value) in [("--name", &name), ("--app", &app), ("--command", &command)] {
        if let Some(value) = value.as_deref()
            && (value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(format!(
                "ps {flag} must be 1..=256 bytes without control characters"
            ));
        }
    }
    let cpu_above_percent = flag_parsed::<f64>(args, "--cpu-above")?;
    if cpu_above_percent
        .is_some_and(|value| !value.is_finite() || !(0.0..=100_000.0).contains(&value))
    {
        return Err("ps --cpu-above must be in 0..=100000".into());
    }
    let memory_above_mb = flag_parsed::<f64>(args, "--memory-above-mb")?;
    if memory_above_mb.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err("ps --memory-above-mb must be a finite non-negative number".into());
    }
    let sort = flag_text(args, "--sort")?;
    if sort
        .as_deref()
        .is_some_and(|value| !matches!(value, "pid" | "cpu" | "mem" | "memory"))
    {
        return Err("ps --sort must be pid|cpu|mem|memory".into());
    }
    let sample_ms = flag_parsed::<u64>(args, "--sample-ms")?;
    if sample_ms.is_some_and(|value| !(10..=10_000).contains(&value)) {
        return Err("ps --sample-ms must be in 10..=10000".into());
    }
    if sample_ms.is_some() && cpu_above_percent.is_none() && sort.as_deref() != Some("cpu") {
        return Err("ps --sample-ms requires --cpu-above or --sort cpu".into());
    }
    let max_visited = flag_parsed::<usize>(args, "--max-visited")?;
    if max_visited.is_some_and(|value| !(1..=10_000).contains(&value)) {
        return Err("ps --max-visited must be in 1..=10000".into());
    }
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    let depth = flag_parsed::<usize>(args, "--depth")?;
    if depth.is_some_and(|value| value > 64) {
        return Err("ps --depth must be in 0..=64".into());
    }
    let files = take_switch(args, "--files");
    let ports = take_switch(args, "--ports");
    let meta = take_switch(args, "--meta");
    if (depth.is_some() || files || ports) && pid.is_none() {
        return Err("ps --depth/--files/--ports require --pid N".into());
    }
    if (depth.is_some() || files || ports)
        && (parent.is_some()
            || name.is_some()
            || app.is_some()
            || command.is_some()
            || cpu_above_percent.is_some()
            || memory_above_mb.is_some()
            || sort.is_some()
            || sample_ms.is_some()
            || offset.is_some())
    {
        return Err("ps --pid detail mode cannot be combined with list filters or --offset".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "ps accepts only bounded process inventory flags; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Ps {
        target,
        pid,
        parent,
        name,
        app,
        command,
        cpu_above_percent,
        memory_above_mb,
        sort,
        sample_ms,
        max_visited,
        depth,
        files,
        ports,
        meta,
        offset,
        max,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::verbs;

    #[test]
    fn shell_exec_has_a_closed_bounded_shape() {
        let spec = verbs::lookup("shell-exec").expect("shell-exec verb");
        let mut args = vec![
            "--command".into(),
            "printf marker".into(),
            "--timeout-ms".into(),
            "2500".into(),
            "--max-output-bytes".into(),
            "4096".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut args).expect("parse"),
            Command::ShellExec {
                ref command,
                timeout_ms: 2500,
                max_output_bytes: 4096,
                ..
            } if command == "printf marker"
        ));
        for invalid in [
            vec!["--command", ""],
            vec!["--command", "x", "--timeout-ms", "99"],
            vec!["--command", "x", "--max-output-bytes", "16777217"],
        ] {
            let mut invalid = invalid.into_iter().map(str::to_owned).collect();
            assert!(parse(spec, spec.name, TargetRef::Current, &mut invalid).is_err());
        }
    }

    #[test]
    fn ps_parses_the_closed_bounded_shape() {
        let spec = verbs::lookup("ps").expect("ps verb");
        let mut args = vec![
            "--pid".into(),
            "42".into(),
            "--parent".into(),
            "7".into(),
            "--name".into(),
            "worker".into(),
            "--command".into(),
            "agenterm".into(),
            "--memory-above-mb".into(),
            "1".into(),
            "--sort".into(),
            "memory".into(),
            "--max-visited".into(),
            "100".into(),
            "--offset".into(),
            "3".into(),
            "--max".into(),
            "9".into(),
        ];
        let command = parse(spec, spec.name, TargetRef::Current, &mut args).expect("parse");
        assert!(matches!(
            command,
            Command::Ps {
                pid: Some(42),
                parent: Some(7),
                ref name,
                ref command,
                memory_above_mb: Some(1.0),
                ref sort,
                max_visited: Some(100),
                offset: Some(3),
                max: Some(9),
                ..
            } if name.as_deref() == Some("worker")
                && command.as_deref() == Some("agenterm")
                && sort.as_deref() == Some("memory")
        ));
    }

    #[test]
    fn ps_accepts_rich_filters_and_rejects_mixed_detail_mode() {
        let spec = verbs::lookup("ps").expect("ps verb");
        let mut args = vec![
            "--cpu-above".into(),
            "5".into(),
            "--sort".into(),
            "cpu".into(),
            "--sample-ms".into(),
            "50".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut args).expect("rich ps"),
            Command::Ps {
                cpu_above_percent: Some(5.0),
                sample_ms: Some(50),
                ..
            }
        ));
        let mut mixed = vec![
            "--pid".into(),
            "42".into(),
            "--depth".into(),
            "1".into(),
            "--name".into(),
            "worker".into(),
        ];
        let error = parse(spec, spec.name, TargetRef::Current, &mut mixed).expect_err("mixed");
        assert!(error.contains("cannot be combined"), "{error}");
    }

    #[test]
    fn process_state_requires_one_positive_pid() {
        let spec = verbs::lookup("process-state").expect("process-state verb");
        let mut args = vec!["--pid".into(), "42".into()];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessState { pid: 42, .. }
        ));

        let mut missing = Vec::new();
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut missing)
                .expect_err("missing")
                .contains("requires --pid")
        );
    }

    #[test]
    fn process_argv_accepts_native_and_mcu_shapes_and_bounds_the_page() {
        let spec = verbs::lookup("process-argv").expect("process-argv verb");
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "--offset".into(),
            "3".into(),
            "--limit".into(),
            "9".into(),
            "--values".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessArgv {
                pid: 42,
                values: true,
                offset: Some(3),
                limit: Some(9),
                ..
            }
        ));

        let mut mcu = vec!["argv".into(), "42".into()];
        assert!(matches!(
            parse(spec, "process", TargetRef::Current, &mut mcu).expect("MCU alias"),
            Command::ProcessArgv {
                pid: 42,
                values: false,
                offset: None,
                limit: None,
                ..
            }
        ));

        let mut unbounded = vec!["--pid".into(), "42".into(), "--limit".into(), "4097".into()];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut unbounded)
                .expect_err("bounded")
                .contains("1..=4096")
        );
    }

    #[test]
    fn process_cwd_accepts_native_and_mcu_shapes() {
        let spec = verbs::lookup("process-cwd").expect("process-cwd verb");
        let mut native = vec!["--pid".into(), "42".into()];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessCwd { pid: 42, .. }
        ));

        let mut mcu = vec!["cwd".into(), "42".into()];
        assert!(matches!(
            parse(spec, "process", TargetRef::Current, &mut mcu).expect("MCU alias"),
            Command::ProcessCwd { pid: 42, .. }
        ));
    }

    #[test]
    fn process_environment_accepts_disclosure_filter_and_bounded_page() {
        let spec = verbs::lookup("process-environment").expect("process-environment verb");
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "--prefix".into(),
            "APP_".into(),
            "--offset".into(),
            "3".into(),
            "--limit".into(),
            "9".into(),
            "--values".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessEnvironment {
                pid: 42,
                prefix: Some(prefix),
                values: true,
                offset: Some(3),
                limit: Some(9),
                ..
            } if prefix == "APP_"
        ));

        let mut mcu = vec!["env".into(), "42".into()];
        assert!(matches!(
            parse(spec, "process", TargetRef::Current, &mut mcu).expect("MCU alias"),
            Command::ProcessEnvironment {
                pid: 42,
                prefix: None,
                values: false,
                offset: None,
                limit: None,
                ..
            }
        ));

        let mut unbounded = vec!["--pid".into(), "42".into(), "--limit".into(), "5001".into()];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut unbounded)
                .expect_err("bounded")
                .contains("1..=5000")
        );
    }

    #[test]
    fn process_usage_parses_the_same_closed_pid_shape() {
        let spec = verbs::lookup("process-usage").expect("process-usage verb");
        let mut args = vec!["--pid".into(), "42".into()];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Ssh, &mut args).expect("parse"),
            Command::ProcessUsage {
                target: TargetRef::Ssh,
                pid: 42,
                watch_ms: None,
                interval_ms: None,
                max_samples: None,
            }
        ));
    }

    #[test]
    fn process_usage_watch_requires_closed_bounded_parameters() {
        let spec = verbs::lookup("process-usage").expect("process-usage verb");
        let mut args = vec![
            "--pid".into(),
            "42".into(),
            "--watch-ms".into(),
            "1000".into(),
            "--interval-ms".into(),
            "100".into(),
            "--max-samples".into(),
            "4".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessUsage {
                pid: 42,
                watch_ms: Some(1000),
                interval_ms: Some(100),
                max_samples: Some(4),
                ..
            }
        ));

        let mut orphan_interval = vec![
            "--pid".into(),
            "42".into(),
            "--interval-ms".into(),
            "100".into(),
        ];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut orphan_interval)
                .expect_err("watch required")
                .contains("require --watch-ms")
        );
    }

    #[test]
    fn process_wait_requires_identity_and_a_bounded_timeout() {
        let spec = verbs::lookup("process-wait").expect("process-wait verb");
        let mut args = vec![
            "--pid".into(),
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
            "--timeout-ms".into(),
            "250".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessWait {
                pid: 42,
                timeout_ms: 250,
                ref start_identity,
                ..
            } if start_identity == "boot:123"
        ));

        let mut missing_identity = vec!["--pid".into(), "42".into()];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut missing_identity)
                .expect_err("identity")
                .contains("--start-identity")
        );
    }

    #[test]
    fn process_sockets_accepts_native_and_mcu_shapes_with_independent_bounds() {
        let spec = verbs::lookup("process-sockets").expect("process-sockets verb");
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "--family".into(),
            "IPv6".into(),
            "--protocol".into(),
            "tcp".into(),
            "--state".into(),
            "listen".into(),
            "--endpoint".into(),
            ":443".into(),
            "--offset".into(),
            "3".into(),
            "--limit".into(),
            "5".into(),
            "--max-visited".into(),
            "200".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessSockets {
                pid: 42,
                offset: Some(3),
                limit: Some(5),
                max_visited: Some(200),
                ref family,
                ref protocol,
                ref state,
                ref endpoint,
                ..
            } if family.as_deref() == Some("IPv6")
                && protocol.as_deref() == Some("tcp")
                && state.as_deref() == Some("listen")
                && endpoint.as_deref() == Some(":443")
        ));

        let mut mcu = vec!["sockets".into(), "42".into(), "--limit".into(), "1".into()];
        assert!(matches!(
            parse(spec, "process", TargetRef::Ssh, &mut mcu).expect("MCU alias"),
            Command::ProcessSockets {
                target: TargetRef::Ssh,
                pid: 42,
                limit: Some(1),
                ..
            }
        ));
    }

    #[test]
    fn process_cgroup_accepts_native_and_grouped_identity_bound_shapes() {
        let spec = verbs::lookup("process-cgroup").expect("process-cgroup verb");
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessCgroup {
                pid: 42,
                ref start_identity,
                ..
            } if start_identity.as_deref() == Some("boot:123")
        ));

        let mut grouped = vec!["cgroup".into(), "42".into()];
        assert!(matches!(
            parse(spec, "process", TargetRef::Ssh, &mut grouped).expect("grouped"),
            Command::ProcessCgroup {
                target: TargetRef::Ssh,
                pid: 42,
                start_identity: None,
            }
        ));
    }

    #[test]
    fn process_kill_requires_identity_and_explicit_exit_postcondition() {
        let spec = verbs::lookup("kill").expect("kill alias");
        let mut args = vec![
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
            "--mode".into(),
            "SIGKILL".into(),
            "--timeout-ms".into(),
            "250".into(),
            "--expect".into(),
            "exited".into(),
        ];
        assert!(matches!(
            parse(spec, "kill", TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessKill {
                pid: 42,
                mode: ProcessKillMode::Forceful,
                timeout_ms: 250,
                expect_exited: true,
                ref start_identity,
                ..
            } if start_identity == "boot:123"
        ));

        let mut missing_expect = vec![
            "--pid".into(),
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
        ];
        assert!(
            parse(spec, "kill", TargetRef::Current, &mut missing_expect)
                .expect_err("explicit postcondition")
                .contains("--expect exited")
        );

        let mut bad_mode = vec![
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
            "--mode".into(),
            "maybe".into(),
            "--expect".into(),
            "exited".into(),
        ];
        assert!(
            parse(spec, "kill", TargetRef::Current, &mut bad_mode)
                .expect_err("closed mode")
                .contains("graceful|forceful")
        );
    }

    #[test]
    fn process_set_state_accepts_native_and_mcu_shapes_and_requires_identity() {
        let spec = verbs::lookup("process-set-state").expect("process-set-state verb");
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
            "stopped".into(),
            "--timeout-ms".into(),
            "250".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut native).expect("native"),
            Command::ProcessSetState {
                pid: 42,
                state: ProcessRunState::Stopped,
                timeout_ms: 250,
                ref start_identity,
                ..
            } if start_identity == "boot:123"
        ));

        let mut mcu = vec![
            "set-state".into(),
            "42".into(),
            "running".into(),
            "--start-identity".into(),
            "boot:123".into(),
        ];
        assert!(matches!(
            parse(spec, "process", TargetRef::Ssh, &mut mcu).expect("MCU alias"),
            Command::ProcessSetState {
                target: TargetRef::Ssh,
                pid: 42,
                state: ProcessRunState::Running,
                ..
            }
        ));

        let mut missing_identity = vec!["--pid".into(), "42".into(), "stopped".into()];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut missing_identity)
                .expect_err("identity")
                .contains("--start-identity")
        );
    }

    #[test]
    fn process_policy_parses_observation_and_identity_bound_intent() {
        let spec = verbs::lookup("process-policy").expect("process-policy verb");
        let mut status = vec!["--pid".into(), "42".into(), "status".into()];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Current, &mut status).expect("status"),
            Command::ProcessPolicy {
                pid: 42,
                action: ProcessPolicyAction::Status,
                start_identity: None,
                ..
            }
        ));

        let mut alias = vec![
            "policy".into(),
            "42".into(),
            "background".into(),
            "--start-identity".into(),
            "boot:123".into(),
        ];
        assert!(matches!(
            parse(spec, "process", TargetRef::Ssh, &mut alias).expect("alias"),
            Command::ProcessPolicy {
                target: TargetRef::Ssh,
                pid: 42,
                action: ProcessPolicyAction::Background,
                start_identity: Some(ref identity),
            } if identity == "boot:123"
        ));

        let mut missing_identity = vec!["42".into(), "normal".into()];
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut missing_identity)
                .expect_err("identity")
                .contains("--start-identity")
        );
    }

    #[test]
    fn process_signal_accepts_native_and_mcu_shapes_but_gates_sigkill() {
        let spec = crate::cli::verbs::lookup("process-signal").unwrap();
        let mut native = vec![
            "--pid".into(),
            "42".into(),
            "USR1".into(),
            "--start-identity".into(),
            "boot:123".into(),
        ];
        assert!(matches!(
            parse(spec, "process-signal", TargetRef::Current, &mut native).unwrap(),
            Command::ProcessSignal {
                signal: ProcessSignalKind::User1,
                force: false,
                tree: false,
                ..
            }
        ));

        let mut mcu = vec!["42".into(), "SIGKILL".into(), "--force".into()];
        assert!(matches!(
            parse(spec, "signal", TargetRef::Current, &mut mcu).unwrap(),
            Command::ProcessSignal {
                signal: ProcessSignalKind::Kill,
                force: true,
                tree: false,
                ..
            }
        ));
        let mut refused = vec!["42".into(), "SIGKILL".into()];
        assert!(parse(spec, "signal", TargetRef::Current, &mut refused).is_err());

        let mut tree = vec![
            "42".into(),
            "TERM".into(),
            "--tree".into(),
            "--max".into(),
            "64".into(),
        ];
        assert!(matches!(
            parse(spec, "signal", TargetRef::Current, &mut tree).unwrap(),
            Command::ProcessSignal {
                signal: ProcessSignalKind::Terminate,
                tree: true,
                max_descendants: 64,
                ..
            }
        ));
        let mut max_without_tree = vec!["42".into(), "TERM".into(), "--max".into(), "500".into()];
        assert!(parse(spec, "signal", TargetRef::Current, &mut max_without_tree).is_err());
    }

    #[test]
    fn process_watch_requires_a_selector_and_closed_budgets() {
        let spec = verbs::lookup("process-watch").expect("process-watch verb");
        let mut args = vec![
            "--name".into(),
            "worker".into(),
            "--duration-ms".into(),
            "1000".into(),
            "--interval-ms".into(),
            "50".into(),
            "--max-events".into(),
            "8".into(),
            "--max-processes".into(),
            "20".into(),
        ];
        assert!(matches!(
            parse(spec, spec.name, TargetRef::Ssh, &mut args).expect("parse"),
            Command::ProcessWatch {
                target: TargetRef::Ssh,
                ref name,
                duration_ms: 1000,
                interval_ms: Some(50),
                max_events: Some(8),
                max_processes: Some(20),
                ..
            } if name.as_deref() == Some("worker")
        ));

        let mut missing = Vec::new();
        assert!(
            parse(spec, spec.name, TargetRef::Current, &mut missing)
                .expect_err("missing")
                .contains("requires --pid")
        );
    }
}
