//! Local management surface for persisted, target-bound grants.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;

use crate::{
    CuError, CuReply, Grant,
    auth_store::{
        AuthStore, AuthStoreError, AuthStoreErrorKind, GrantAuthority, GrantRecord, GrantSpec,
    },
    target::TargetRef,
    target_binding::{CurrentIdentityProvider, enroll_current_identity, resolve_target_binding},
};

pub fn dispatch(args: &[String], ambient_authority_present: bool) -> Option<CuReply> {
    if args.first().map(String::as_str) != Some("grant") {
        return None;
    }
    if ambient_authority_present {
        return Some(failure(
            "grant",
            "invalid_authorization",
            "grant management does not accept ambient authorization selectors",
        ));
    }
    Some(match args.get(1).map(String::as_str) {
        Some("create") => create(&args[2..]),
        Some("list") => list(&args[2..]),
        Some("revoke") => revoke(&args[2..]),
        _ => failure(
            "grant",
            "invalid_grant_command",
            "grant requires create, list, or revoke",
        ),
    })
}

fn create(args: &[String]) -> CuReply {
    let parsed = match CreateArgs::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => return failure("grant-create", "invalid_grant", message),
    };
    if parsed.target != "current" {
        return failure(
            "grant-create",
            "target_binding_unavailable",
            "persisted grants currently support only the current target",
        );
    }
    let scopes = match Grant::parse_many_strict(&parsed.scopes) {
        Ok(scopes) => scopes,
        Err(_) => {
            return failure("grant-create", "invalid_grant", "grant scopes are invalid");
        }
    };
    let operations = match parse_operations(&parsed.operations) {
        Ok(operations) => operations,
        Err(message) => return failure("grant-create", "invalid_grant", message),
    };
    let max_uses = if parsed.one_shot {
        1
    } else {
        parsed.max_uses.expect("parser requires one use mode")
    };
    let now = match now_utc_ms() {
        Some(now) => now,
        None => {
            return failure(
                "grant-create",
                "clock_unavailable",
                "system clock is unavailable",
            );
        }
    };
    let Some(expires) = now.checked_add(parsed.ttl_ms) else {
        return failure("grant-create", "invalid_grant", "grant lifetime overflows");
    };
    let path = match store_path(parsed.store) {
        Ok(path) => path,
        Err(error) => return store_failure("grant-create", &error),
    };
    let mut store = match AuthStore::open_private_at(&path) {
        Ok(store) => store,
        Err(error) => return store_failure("grant-create", &error),
    };
    let Some(parent) = path.parent() else {
        return failure(
            "grant-create",
            "grant_store_unavailable",
            "grant store is unavailable",
        );
    };
    let provider = CurrentIdentityProvider::at(parent);
    if enroll_current_identity(&provider).is_err() {
        return failure(
            "grant-create",
            "target_binding_unavailable",
            "verified current target identity is unavailable",
        );
    }
    let binding = match resolve_target_binding(TargetRef::Current, Some(&provider)) {
        Ok(binding) => binding,
        Err(_) => {
            return failure(
                "grant-create",
                "target_binding_unavailable",
                "verified current target identity is unavailable",
            );
        }
    };
    let grant_id = match generated_grant_id() {
        Some(id) => id,
        None => {
            return failure(
                "grant-create",
                "entropy_unavailable",
                "grant id generation failed",
            );
        }
    };
    let spec = GrantSpec::new(
        &grant_id,
        &binding,
        GrantAuthority::new(scopes, operations),
        now,
        now,
        expires,
        max_uses,
    );
    match store.create(spec) {
        Ok(record) => success(
            "grant-create",
            json!({
                "generation": store.generation(),
                "grant": projection(&record),
            }),
        ),
        Err(error) => store_failure("grant-create", &error),
    }
}

fn list(args: &[String]) -> CuReply {
    let store = match only_store_arg(args) {
        Ok(store) => store,
        Err(message) => return failure("grant-list", "invalid_grant", message),
    };
    let path = match store_path(store) {
        Ok(path) => path,
        Err(error) => return store_failure("grant-list", &error),
    };
    match AuthStore::open_private_at(path) {
        Ok(store) => success(
            "grant-list",
            json!({
                "generation": store.generation(),
                "grants": store.list().iter().map(projection).collect::<Vec<_>>(),
            }),
        ),
        Err(error) => store_failure("grant-list", &error),
    }
}

fn revoke(args: &[String]) -> CuReply {
    let (grant_id, store_override) = match revoke_args(args) {
        Ok(parsed) => parsed,
        Err(message) => return failure("grant-revoke", "invalid_grant", message),
    };
    let path = match store_path(store_override) {
        Ok(path) => path,
        Err(error) => return store_failure("grant-revoke", &error),
    };
    let mut store = match AuthStore::open_private_at(path) {
        Ok(store) => store,
        Err(error) => return store_failure("grant-revoke", &error),
    };
    let now = match now_utc_ms() {
        Some(now) => now,
        None => {
            return failure(
                "grant-revoke",
                "clock_unavailable",
                "system clock is unavailable",
            );
        }
    };
    match store.revoke(&grant_id, now) {
        Ok(decision) => {
            let record = match decision {
                crate::auth_store::RevokeDecision::Revoked(record)
                | crate::auth_store::RevokeDecision::AlreadyRevoked(record) => record,
            };
            success(
                "grant-revoke",
                json!({
                    "generation": store.generation(),
                    "grant": projection(&record),
                }),
            )
        }
        Err(error) => store_failure("grant-revoke", &error),
    }
}

struct CreateArgs {
    target: String,
    scopes: String,
    operations: String,
    ttl_ms: i64,
    max_uses: Option<u64>,
    one_shot: bool,
    store: Option<PathBuf>,
}

impl CreateArgs {
    fn parse(args: &[String]) -> Result<Self, &'static str> {
        let mut target = None;
        let mut scopes = None;
        let mut operations = None;
        let mut ttl_ms = None;
        let mut seen_ttl = false;
        let mut max_uses = None;
        let mut seen_max_uses = false;
        let mut one_shot = false;
        let mut store = None;
        let mut index = 0;
        while index < args.len() {
            let flag = args[index].as_str();
            if flag == "--one-shot" {
                if one_shot {
                    return Err("duplicate --one-shot");
                }
                one_shot = true;
                index += 1;
                continue;
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.is_empty())
                .ok_or("grant flag requires a value")?;
            match flag {
                "--target" if target.is_none() => target = Some(value.clone()),
                "--scopes" if scopes.is_none() => scopes = Some(value.clone()),
                "--operations" if operations.is_none() => operations = Some(value.clone()),
                "--ttl-ms" if !seen_ttl => {
                    seen_ttl = true;
                    ttl_ms = Some(
                        value
                            .parse::<i64>()
                            .map_err(|_| "--ttl-ms requires an integer")?,
                    );
                }
                "--max-uses" if !seen_max_uses => {
                    seen_max_uses = true;
                    max_uses = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| "--max-uses requires an integer")?,
                    );
                }
                "--grant-store" if store.is_none() => store = Some(PathBuf::from(value)),
                "--target" | "--scopes" | "--operations" | "--ttl-ms" | "--max-uses"
                | "--grant-store" => {
                    return Err("duplicate grant flag");
                }
                _ => return Err("unknown grant flag"),
            }
            index += 2;
        }
        let ttl_ms = ttl_ms
            .filter(|value| *value > 0)
            .ok_or("--ttl-ms requires a positive integer")?;
        if one_shot == max_uses.is_some() {
            return Err("choose exactly one of --one-shot or --max-uses");
        }
        if max_uses == Some(0) {
            return Err("--max-uses requires a positive integer");
        }
        Ok(Self {
            target: target.ok_or("--target current is required")?,
            scopes: scopes.ok_or("--scopes is required")?,
            operations: operations.ok_or("--operations is required")?,
            ttl_ms,
            max_uses,
            one_shot,
            store,
        })
    }
}

fn parse_operations(value: &str) -> Result<std::collections::BTreeSet<String>, &'static str> {
    let operations = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    if operations.is_empty() || operations.contains("") {
        return Err("--operations requires one or more comma-separated canonical ids");
    }
    if operations
        .iter()
        .any(|operation| !known_authorization_operation(operation))
    {
        return Err("--operations contains an unknown canonical operation id");
    }
    Ok(operations)
}

fn known_authorization_operation(operation: &str) -> bool {
    const SPLIT_BASES: &[&str] = &[
        "app",
        "audio",
        "audit-compact",
        "clipboard-clear",
        "device-screenshot",
        "diff",
        "file-copy",
        "file-move",
        "file-transaction",
        "invoke",
        "login-session",
        "page-dialog",
        "page-screenshot",
        "permissions",
        "process-kill",
        "process-set-state",
        "process-signal",
        "pty-diff",
        "service",
        "setup",
        "window-place",
    ];
    if matches!(
        operation,
        "setup.check"
            | "setup.apply"
            | "permissions.status"
            | "permissions.open"
            | "audio.status"
            | "audio.plan-volume"
            | "audio.plan-muted"
            | "audio.apply"
            | "login-session.status"
            | "login-session.plan-lock"
            | "login-session.apply-lock"
            | "audit-compact.plan"
            | "audit-compact.apply"
            | "file-copy.plan"
            | "file-copy.apply"
            | "file-move.plan"
            | "file-move.apply"
            | "file-transaction.status"
            | "file-transaction.rollback"
            | "file-transaction.recover"
            | "file-transaction.finalize"
            | "page-screenshot.capture"
            | "page-screenshot.capture-and-activate"
            | "app.hide"
            | "app.show"
            | "app.quit"
            | "app.launch"
            | "device-screenshot.list"
            | "device-screenshot.capture"
            | "page-dialog.accept"
            | "page-dialog.dismiss"
            | "diff.read"
            | "diff.advance"
            | "process-kill.graceful"
            | "process-kill.forceful"
            | "process-set-state.running"
            | "process-set-state.stopped"
            | "pty-diff.read"
            | "pty-diff.advance"
            | "clipboard-clear.plan"
            | "clipboard-clear.apply"
    ) {
        return true;
    }
    let parts = operation.split('.').collect::<Vec<_>>();
    if matches!(
        parts.as_slice(),
        ["service", "list" | "status", "user" | "system"]
    ) || matches!(
        parts.as_slice(),
        [
            "service",
            "plan" | "transact",
            "user" | "system",
            "start" | "stop" | "restart" | "bootstrap" | "bootout"
        ]
    ) || matches!(parts.as_slice(), ["service", "apply"])
        || matches!(
            parts.as_slice(),
            [
                "invoke",
                "press"
                    | "set-value"
                    | "select-option"
                    | "set-checked"
                    | "set-expanded"
                    | "increment"
                    | "decrement"
                    | "set-selected"
                    | "set-selection"
                    | "scroll-to"
                    | "cancel"
                    | "show-default-ui"
            ]
        )
        || matches!(
            parts.as_slice(),
            [
                "window-place",
                "center"
                    | "fullscreen"
                    | "left-half"
                    | "right-half"
                    | "top-half"
                    | "bottom-half"
                    | "upper-left"
                    | "lower-left"
                    | "upper-right"
                    | "lower-right"
                    | "next-third"
                    | "previous-third"
                    | "next-display"
                    | "previous-display"
                    | "larger"
                    | "smaller"
                    | "undo"
                    | "redo"
                    | "frame"
                    | "move"
                    | "resize"
            ]
        )
        || matches!(
            parts.as_slice(),
            [
                "process-signal",
                "single" | "tree",
                "normal" | "force",
                "sighup"
                    | "sigint"
                    | "sigterm"
                    | "sigkill"
                    | "sigstop"
                    | "sigcont"
                    | "sigusr1"
                    | "sigusr2"
            ]
        )
    {
        return true;
    }

    if SPLIT_BASES.contains(&operation)
        || matches!(operation, "exec" | "grant" | "help" | "host" | "verbs")
        || matches!(
            operation,
            "pty"
                | "job"
                | "process"
                | "resource"
                | "power"
                | "storage"
                | "file"
                | "network"
                | "device"
                | "privilege"
                | "daemon"
                | "desktop-helper"
                | "simulator"
                | "page"
                | "ghost"
                | "open"
                | "notify"
                | "service"
        )
    {
        return false;
    }
    let Ok(catalog) =
        serde_json::from_str::<serde_json::Value>(include_str!("bin/cli/verbs-catalog.json"))
    else {
        return false;
    };
    catalog["verbs"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["name"] == operation))
}

fn only_store_arg(args: &[String]) -> Result<Option<PathBuf>, &'static str> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--grant-store" && !value.is_empty() => Ok(Some(value.into())),
        _ => Err("grant list accepts only one optional --grant-store path"),
    }
}

fn revoke_args(args: &[String]) -> Result<(String, Option<PathBuf>), &'static str> {
    let mut grant_id = None;
    let mut store = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .filter(|value| !value.is_empty())
            .ok_or("grant revoke flag requires a value")?;
        match flag {
            "--grant-id" if grant_id.is_none() => grant_id = Some(value.clone()),
            "--grant-store" if store.is_none() => store = Some(PathBuf::from(value)),
            "--grant-id" | "--grant-store" => return Err("duplicate grant revoke flag"),
            _ => return Err("unknown grant revoke flag"),
        }
        index += 2;
    }
    let grant_id = grant_id.ok_or("--grant-id is required")?;
    if !valid_grant_id(&grant_id) {
        return Err("--grant-id is invalid");
    }
    Ok((grant_id, store))
}

fn store_path(override_path: Option<PathBuf>) -> Result<PathBuf, AuthStoreError> {
    override_path.map_or_else(AuthStore::default_path, Ok)
}

fn generated_grant_id() -> Option<String> {
    let bytes = agenterm_platform::entropy::secure_random_array::<32>().ok()?;
    Some(format!("cu1_{}", encode_hex(&bytes)))
}

pub fn valid_grant_id(value: &str) -> bool {
    value.strip_prefix("cu1_").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn now_utc_ms() -> Option<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

fn projection(record: &GrantRecord) -> serde_json::Value {
    json!({
        "grant_id": record.grant_id,
        "target_id": record.target_id,
        "tier": record.tier,
        "scopes": record.scopes,
        "operations": record.operations,
        "legacy_operation_unbound": record.legacy_operation_unbound,
        "issued_at_utc_ms": record.issued_at_utc_ms,
        "expires_at_utc_ms": record.expires_at_utc_ms,
        "max_uses": record.max_uses,
        "consumed_uses": record.consumed_uses,
        "remaining_uses": record.max_uses - record.consumed_uses,
        "revoked": record.revoked_at_utc_ms.is_some(),
        "one_shot": record.one_shot,
        "session_bound": record.session_bound,
    })
}

fn store_failure(command: &str, error: &AuthStoreError) -> CuReply {
    let (code, message) = match error.kind {
        AuthStoreErrorKind::Parse
        | AuthStoreErrorKind::Validate
        | AuthStoreErrorKind::LegacyUnverified => {
            ("grant_store_corrupt", "grant store is corrupt or untrusted")
        }
        AuthStoreErrorKind::LockContended => ("grant_store_contended", "grant store is busy"),
        AuthStoreErrorKind::GrantNotFound => ("grant_not_found", "grant was not found"),
        AuthStoreErrorKind::DuplicateGrant => ("invalid_grant", "grant could not be created"),
        _ => ("grant_store_unavailable", "grant store is unavailable"),
    };
    failure(command, code, message)
}

fn success(command: &str, data: serde_json::Value) -> CuReply {
    CuReply {
        ok: true,
        target: "current".into(),
        command: command.into(),
        data: Some(data),
        error: None,
    }
}

fn failure(command: &str, code: &str, message: impl Into<String>) -> CuReply {
    CuReply {
        ok: false,
        target: "current".into(),
        command: command.into(),
        data: None,
        error: Some(CuError::new(code, message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parser_requires_exact_use_mode_and_rejects_duplicates() {
        assert!(
            CreateArgs::parse(&strings(&[
                "--target",
                "current",
                "--scopes",
                "observe",
                "--operations",
                "capabilities",
                "--ttl-ms",
                "1000",
                "--one-shot"
            ]))
            .is_ok()
        );
        assert!(
            CreateArgs::parse(&strings(&[
                "--target",
                "current",
                "--target",
                "current",
                "--scopes",
                "observe",
                "--operations",
                "capabilities",
                "--ttl-ms",
                "1000",
                "--one-shot"
            ]))
            .is_err()
        );
        assert!(
            CreateArgs::parse(&strings(&[
                "--target",
                "current",
                "--scopes",
                "observe",
                "--ttl-ms",
                "1000",
                "--one-shot"
            ]))
            .is_err()
        );
        assert_eq!(
            parse_operations("capabilities,doctor,capabilities").unwrap(),
            std::collections::BTreeSet::from(["capabilities".to_owned(), "doctor".to_owned()])
        );
        assert!(parse_operations("future-operation").is_err());
        assert!(parse_operations("setup.apply,process-kill.forceful").is_ok());
        assert!(parse_operations("window-place.SpectacleWindowActionCenter").is_err());
        assert!(
            CreateArgs::parse(&strings(&[
                "--target",
                "current",
                "--scopes",
                "observe",
                "--operations",
                "capabilities",
                "--ttl-ms",
                "1000"
            ]))
            .is_err()
        );
    }

    #[test]
    fn ambient_authority_is_rejected_without_echoing_values() {
        let reply = dispatch(&strings(&["grant", "list"]), true).unwrap();
        let rendered = serde_json::to_string(&reply).unwrap();
        assert_eq!(reply.error.unwrap().code, "invalid_authorization");
        assert!(!rendered.contains("secret-marker"));
    }

    #[test]
    fn projection_never_contains_session_binding() {
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "1".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "2".repeat(64)),
        };
        let record = GrantRecord::from(GrantSpec::new(
            "cu1_test",
            &binding,
            GrantAuthority::new(
                std::collections::BTreeSet::from([Grant::Observe]),
                std::collections::BTreeSet::from(["capabilities".to_owned()]),
            ),
            1,
            1,
            2,
            1,
        ));
        let rendered = projection(&record).to_string();
        assert!(!rendered.contains("session_binding"));
        assert!(!rendered.contains(&record.session_binding.unwrap()));
    }

    #[test]
    fn generated_ids_are_fixed_lowercase_opaque_values() {
        let id = generated_grant_id().unwrap();
        assert_eq!(id.len(), 68);
        assert!(id.starts_with("cu1_"));
        assert!(valid_grant_id(&id));
        assert!(
            id[4..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
}
