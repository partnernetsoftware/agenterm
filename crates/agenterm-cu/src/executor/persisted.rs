//! Persisted-grant authorization (`--grant-id`): reserve one use of a
//! stored grant against the verified current-target binding, audit the
//! decision, revalidate the binding, then dispatch on the current target.

use super::*;

impl Executor {
    pub(super) fn execute_persisted(
        &self,
        command: &Command,
        persisted: &PersistedAuthorization,
    ) -> CuReply {
        if command.target() != TargetRef::Current {
            return CuReply::err(
                command,
                CuError::new(
                    "persisted_grant_remote_unsupported",
                    "persisted grants currently authorize only the current target",
                ),
            );
        }
        if let Err(message) = command.validate() {
            return CuReply::err(command, CuError::new("invalid_input", message));
        }
        let Some(operation) = command.authorization_operation() else {
            return CuReply::err(
                command,
                CuError::new(
                    "authorization_operation_unavailable",
                    "command has no canonical persisted-authorization operation",
                ),
            );
        };
        let decision_id = match generated_decision_id() {
            Some(id) => id,
            None => {
                return CuReply::err(
                    command,
                    CuError::new("authorization_unavailable", "decision id generation failed"),
                );
            }
        };
        let mut audit = match self.open_audit() {
            Ok(audit) => audit,
            Err(error) => return CuReply::err(command, error),
        };
        let Some(state_dir) = persisted.store_path.parent() else {
            return CuReply::err(
                command,
                CuError::new("grant_store_unavailable", "grant store is unavailable"),
            );
        };
        let provider = CurrentIdentityProvider::at(state_dir);
        let binding = match self.resolve_current_binding(&provider) {
            Ok(binding) => binding,
            Err(_) => {
                return CuReply::err(
                    command,
                    CuError::new(
                        "target_binding_unavailable",
                        "verified current target identity is unavailable",
                    ),
                );
            }
        };
        let required = command.required_grant();
        let mut store = match AuthStore::open_private_at(&persisted.store_path) {
            Ok(store) => store,
            Err(error) => return CuReply::err(command, map_store_authorization_error(&error)),
        };
        let now = match now_utc_ms() {
            Some(now) => now,
            None => {
                return CuReply::err(
                    command,
                    CuError::new("authorization_clock_invalid", "system clock is unavailable"),
                );
            }
        };
        let attempt = GrantAttempt::new(&persisted.grant_id, &binding, required, &operation);
        match store.reserve_attempt(&attempt, now) {
            Ok(GrantDecision::Denied(denial)) => {
                let outcome = match denial.kind {
                    GrantDenialKind::NotFound => "not_found",
                    GrantDenialKind::NotYetValid => "not_yet_valid",
                    GrantDenialKind::Expired => "expired",
                    GrantDenialKind::Revoked => "revoked",
                    GrantDenialKind::Exhausted => "exhausted",
                    GrantDenialKind::TargetMismatch => "target_mismatch",
                    GrantDenialKind::OperationUnbound => "operation_unbound",
                    GrantDenialKind::OperationMismatch => "operation_mismatch",
                    GrantDenialKind::ScopeMissing => "scope_missing",
                };
                if let Err(error) = audit.record_persisted(
                    command.target(),
                    command,
                    required,
                    &decision_id,
                    binding.target_id(),
                    &persisted.grant_id,
                    "denied",
                    outcome,
                    None,
                ) {
                    return CuReply::err(command, error);
                }
                return CuReply::err(
                    command,
                    CuError::new("refused", format!("persisted grant is {outcome}")),
                );
            }
            Ok(GrantDecision::Authorized(_)) => {}
            Err(error) => return CuReply::err(command, map_store_authorization_error(&error)),
        }
        if let Err(error) = audit.record_persisted(
            command.target(),
            command,
            required,
            &decision_id,
            binding.target_id(),
            &persisted.grant_id,
            "authorized",
            "attempt",
            None,
        ) {
            return CuReply::err(command, error);
        }
        let revalidated = match self.resolve_current_binding(&provider) {
            Ok(binding) => binding,
            Err(_) => {
                return self.persisted_pre_dispatch_failure(
                    command,
                    &mut audit,
                    required,
                    &decision_id,
                    &binding,
                    &persisted.grant_id,
                    "target_binding_unavailable",
                );
            }
        };
        if revalidated != binding {
            return self.persisted_pre_dispatch_failure(
                command,
                &mut audit,
                required,
                &decision_id,
                &binding,
                &persisted.grant_id,
                "target_binding_changed",
            );
        }
        let reply = self.execute_current(command);
        let outcome = if reply.ok { "ok" } else { "failed" };
        let detail = reply.data.clone().or_else(|| {
            reply
                .error
                .as_ref()
                .and_then(|error| serde_json::to_value(error).ok())
        });
        if let Err(mut error) = audit.record_persisted(
            command.target(),
            command,
            required,
            &decision_id,
            binding.target_id(),
            &persisted.grant_id,
            "authorized",
            outcome,
            detail,
        ) {
            let effect = if reply.ok {
                reply.data.as_ref().and_then(|data| data.get("effect"))
            } else {
                reply
                    .error
                    .as_ref()
                    .and_then(|error| error.detail.as_ref())
                    .and_then(|detail| detail.get("effect"))
            }
            .cloned()
            .unwrap_or(serde_json::Value::String("unknown".into()));
            error.detail = Some(serde_json::json!({
                "stage": "audit_outcome",
                "effect": effect,
                "decision_id": decision_id,
                "original_reply": reply,
            }));
            return CuReply::err(command, error);
        }
        reply
    }

    pub(super) fn resolve_current_binding(
        &self,
        provider: &CurrentIdentityProvider,
    ) -> Result<crate::target_binding::TargetBinding, crate::target_binding::TargetBindingError>
    {
        #[cfg(test)]
        if let Some(binding) = self.persisted_binding.as_ref() {
            return Ok(binding.clone());
        }
        resolve_target_binding(TargetRef::Current, Some(provider))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn persisted_pre_dispatch_failure(
        &self,
        command: &Command,
        audit: &mut AuditLog,
        required: Grant,
        decision_id: &str,
        binding: &crate::target_binding::TargetBinding,
        grant_id: &str,
        code: &'static str,
    ) -> CuReply {
        let error = CuError::new(
            code,
            "verified current target identity changed before dispatch",
        );
        if let Err(audit_error) = audit.record_persisted(
            command.target(),
            command,
            required,
            decision_id,
            binding.target_id(),
            grant_id,
            "authorized",
            "failed",
            serde_json::to_value(&error).ok(),
        ) {
            return CuReply::err(command, audit_error);
        }
        CuReply::err(command, error)
    }
}

pub(super) fn map_store_authorization_error(error: &crate::auth_store::AuthStoreError) -> CuError {
    if error.published {
        return CuError::new(
            "authorization_in_doubt",
            "grant consumption may have been published without confirmed durability",
        )
        .with_detail(serde_json::json!({
            "effect": "not_applied",
            "authorization": "possibly_consumed",
        }));
    }
    let (code, message) = match error.kind {
        AuthStoreErrorKind::Parse
        | AuthStoreErrorKind::Validate
        | AuthStoreErrorKind::LegacyUnverified => {
            ("grant_store_corrupt", "grant store is corrupt or untrusted")
        }
        AuthStoreErrorKind::LockContended => ("grant_store_contended", "grant store is busy"),
        _ => ("grant_store_unavailable", "grant store is unavailable"),
    };
    CuError::new(code, message)
}

pub(super) fn generated_decision_id() -> Option<String> {
    let bytes = agenterm_platform::entropy::secure_random_array::<16>().ok()?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(3 + bytes.len() * 2);
    output.push_str("d1_");
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Some(output)
}

pub(super) fn now_utc_ms() -> Option<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    #[test]
    fn persisted_one_shot_is_audited_revalidated_and_exhausted() {
        let audit_path = audit_scratch("persisted-one-shot");
        let root = audit_path.parent().unwrap();
        let store_path = root.join("cu-grants.json");
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "1".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "2".repeat(64)),
        };
        let grant_id = format!("cu1_{}", "3".repeat(64));
        let now = now_utc_ms().unwrap();
        let mut store = AuthStore::open_private_at(&store_path).unwrap();
        store
            .create(crate::auth_store::GrantSpec::new(
                &grant_id,
                &binding,
                crate::auth_store::GrantAuthority::new(
                    BTreeSet::from([Grant::Observe]),
                    BTreeSet::from(["capabilities".to_owned()]),
                ),
                now,
                now,
                now + 60_000,
                1,
            ))
            .unwrap();
        drop(store);

        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let executor = Executor::new(Authorization::new(BTreeSet::new()))
            .with_persisted_grant(&grant_id, &store_path)
            .with_persisted_binding(binding)
            .with_audit_path(audit_path.clone());
        assert!(executor.execute(&command).ok);
        let refused = executor.execute(&command);
        assert!(!refused.ok);
        assert_eq!(refused.error.as_ref().unwrap().code, "refused");
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("exhausted")
        );

        let raw = std::fs::read_to_string(&audit_path).unwrap();
        assert!(!raw.contains("agt-cu-ses"));
        let records = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["decision"], "authorized");
        assert_eq!(records[0]["outcome"], "attempt");
        assert_eq!(records[1]["outcome"], "ok");
        assert_eq!(records[2]["decision"], "denied");
        assert_eq!(records[2]["outcome"], "exhausted");
        assert_eq!(records[0]["decision_id"], records[1]["decision_id"]);
        assert_ne!(records[1]["decision_id"], records[2]["decision_id"]);
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            1
        );
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn same_scope_wrong_operation_is_refused_without_consuming() {
        let audit_path = audit_scratch("persisted-operation");
        let root = audit_path.parent().unwrap();
        let store_path = root.join("cu-grants.json");
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "7".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "8".repeat(64)),
        };
        let grant_id = format!("cu1_{}", "9".repeat(64));
        let now = now_utc_ms().unwrap();
        let mut store = AuthStore::open_private_at(&store_path).unwrap();
        store
            .create(crate::auth_store::GrantSpec::new(
                &grant_id,
                &binding,
                crate::auth_store::GrantAuthority::new(
                    BTreeSet::from([Grant::Observe]),
                    BTreeSet::from(["capabilities".to_owned()]),
                ),
                now,
                now,
                now + 60_000,
                1,
            ))
            .unwrap();
        drop(store);

        let executor = Executor::new(Authorization::new(BTreeSet::new()))
            .with_persisted_grant(&grant_id, &store_path)
            .with_persisted_binding(binding)
            .with_audit_path(audit_path.clone());
        let refused = executor.execute(&Command::Doctor {
            target: TargetRef::Current,
        });
        assert!(!refused.ok);
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|error| error.message.contains("operation_mismatch"))
        );
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            0
        );
        assert!(
            executor
                .execute(&Command::Capabilities {
                    target: TargetRef::Current,
                })
                .ok
        );
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            1
        );
        remove_audit_scratch(&audit_path);
    }

    #[test]
    fn persisted_audit_open_failure_does_not_reserve_the_grant() {
        let audit_path = audit_scratch("persisted-audit-open");
        let root = audit_path.parent().unwrap();
        let store_path = root.join("cu-grants.json");
        let binding = crate::target_binding::TargetBinding {
            tier: TargetRef::Current,
            target_id: format!("agt-cu-tgt-v1-{}", "4".repeat(64)),
            session_binding: format!("agt-cu-ses-v1-{}", "5".repeat(64)),
        };
        let grant_id = format!("cu1_{}", "6".repeat(64));
        let now = now_utc_ms().unwrap();
        let mut store = AuthStore::open_private_at(&store_path).unwrap();
        store
            .create(crate::auth_store::GrantSpec::new(
                &grant_id,
                &binding,
                crate::auth_store::GrantAuthority::new(
                    BTreeSet::from([Grant::Observe]),
                    BTreeSet::from(["capabilities".to_owned()]),
                ),
                now,
                now,
                now + 60_000,
                1,
            ))
            .unwrap();
        drop(store);
        std::fs::create_dir_all(&audit_path).unwrap();

        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let reply = Executor::new(Authorization::new(BTreeSet::new()))
            .with_persisted_grant(&grant_id, &store_path)
            .with_persisted_binding(binding)
            .with_audit_path(audit_path.clone())
            .execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "audit_unavailable");
        assert_eq!(
            AuthStore::open_private_at(&store_path).unwrap().list()[0].consumed_uses,
            0
        );
        remove_audit_scratch(&audit_path);
    }
}
