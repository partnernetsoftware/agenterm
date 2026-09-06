//! `Command` -> verb-family payload builder. One exhaustive match so a
//! new `Command` variant is a compile error here, not a silent `unsupported`.

use super::*;
use crate::FileTransactionAction;

impl Executor {
    pub(super) fn run_current(
        &self,
        command: &Command,
        job_request: Option<&JobRequestContext<'_>>,
    ) -> Result<serde_json::Value, CuError> {
        match command {
            Command::Capabilities { .. } => Ok(capabilities_payload()),
            Command::Setup {
                action, bin_dir, ..
            } => setup_payload(*action, bin_dir.as_deref()),
            Command::Permissions {
                action, permission, ..
            } => match action {
                PermissionAction::Status => {
                    if permission.is_some() {
                        Err(CuError::new(
                            "invalid_input",
                            "permissions status does not accept a permission selector",
                        ))
                    } else {
                        Ok(permissions_payload())
                    }
                }
                PermissionAction::Open => permissions_open_payload(
                    *permission,
                    &mut self.open_receipts(command.target())?,
                ),
            },
            Command::Doctor { .. } => doctor_payload(),
            Command::RuntimeStatus { .. } => runtime_status_payload(),
            Command::AudioStatus { .. } => serde_json::to_value(crate::audio_control::status()?)
                .map_err(|_| {
                    CuError::new(
                        "audio_serialization_failed",
                        "audio status could not be serialized",
                    )
                }),
            Command::AudioPlanVolume {
                volume,
                ttl_seconds,
                ..
            } => audio_plan_payload(crate::audio_control::plan_volume(*volume, *ttl_seconds)?),
            Command::AudioPlanMuted {
                muted, ttl_seconds, ..
            } => audio_plan_payload(crate::audio_control::plan_muted(*muted, *ttl_seconds)?),
            Command::AudioApply {
                request, approval, ..
            } => {
                let plan = crate::audio_control::decode_request(request)?;
                serde_json::to_value(crate::audio_control::apply(&plan, approval)?).map_err(|_| {
                    CuError::new(
                        "audio_serialization_failed",
                        "audio receipt could not be serialized",
                    )
                })
            }
            Command::ServiceList {
                scope,
                match_text,
                max,
                ..
            } => serde_json::to_value(crate::service_control::list(
                *scope,
                match_text.as_deref(),
                *max,
            )?)
            .map_err(|_| {
                CuError::new(
                    "service_serialization_failed",
                    "service inventory could not be serialized",
                )
            }),
            Command::ServiceStatus { scope, name, .. } => serde_json::to_value(
                crate::service_control::status_named(*scope, name)?,
            )
            .map_err(|_| {
                CuError::new(
                    "service_serialization_failed",
                    "service status could not be serialized",
                )
            }),
            Command::ServicePlan {
                scope,
                name,
                operation,
                definition,
                ttl_seconds,
                ..
            } => {
                let identity = crate::service_control::identity(*scope, name)?;
                service_plan_payload(crate::service_control::plan(
                    &identity,
                    *operation,
                    definition.as_ref().map(std::path::PathBuf::from),
                    *ttl_seconds,
                )?)
            }
            Command::ServiceApply {
                request, approval, ..
            } => {
                let plan = crate::service_control::decode_request(request)?;
                serde_json::to_value(crate::service_control::apply(&plan, approval)?).map_err(
                    |_| {
                        CuError::new(
                            "service_serialization_failed",
                            "service apply receipt could not be serialized",
                        )
                    },
                )
            }
            Command::ServiceTransact {
                scope,
                operation,
                name,
                definition,
                ttl_seconds,
                ..
            } => service_transaction_payload(
                job_request.ok_or_else(|| {
                    CuError::new(
                        "service_request_identity_required",
                        "service lifecycle requires request-id, session and session-lease",
                    )
                })?,
                *scope,
                *operation,
                name.as_deref(),
                definition.as_deref(),
                *ttl_seconds,
            ),
            Command::LoginSessionStatus { .. } => {
                serde_json::to_value(crate::login_session::status()?).map_err(|_| {
                    CuError::new(
                        "login_session_serialization_failed",
                        "login-session status could not be serialized",
                    )
                })
            }
            Command::LoginSessionPlanLock { ttl_seconds, .. } => {
                let plan = crate::login_session::plan_lock_with_ttl(*ttl_seconds)?;
                let request = crate::login_session::encode_lock_request(&plan)?;
                let mut value = serde_json::to_value(&plan).map_err(|_| {
                    CuError::new(
                        "login_session_serialization_failed",
                        "session lock plan could not be serialized",
                    )
                })?;
                value["request"] = serde_json::Value::String(request);
                value["approval"] = serde_json::Value::String(plan.approval_digest.clone());
                Ok(value)
            }
            Command::LoginSessionApplyLock {
                request, approval, ..
            } => {
                let plan = crate::login_session::decode_lock_request(request)?;
                serde_json::to_value(crate::login_session::apply_lock(&plan, approval)?).map_err(
                    |_| {
                        CuError::new(
                            "login_session_serialization_failed",
                            "session lock receipt could not be serialized",
                        )
                    },
                )
            }
            Command::HostOpen {
                value,
                application,
                background,
                ..
            } => host_open_payload(
                value,
                application.as_deref(),
                *background,
                &mut self.open_receipts(command.target())?,
            ),
            Command::HostNotify {
                title,
                body,
                subtitle,
                sound,
                ..
            } => host_notify_payload(
                title,
                body,
                subtitle.as_deref(),
                *sound,
                &mut self.open_receipts(command.target())?,
            ),
            Command::AuditQuery {
                verb_filter,
                outcome,
                since_ms,
                offset,
                max,
                scan_max,
                byte_max,
                ..
            } => self.query_audit(audit::AuditQuery {
                verb: verb_filter.as_deref(),
                outcome: outcome.as_deref(),
                since_ms: *since_ms,
                offset: *offset,
                max: *max,
                scan_max: *scan_max,
                byte_max: *byte_max,
            }),
            Command::AuditCompact {
                max_age_days,
                max_events,
                max_bytes,
                apply,
                ..
            } => audit::compact(audit::AuditRetention {
                max_age_days: *max_age_days,
                max_events: *max_events,
                max_bytes: *max_bytes,
                apply: *apply,
            }),
            Command::SessionStart {
                label, ttl_seconds, ..
            } => session_start_payload(label.as_deref(), *ttl_seconds),
            Command::SessionList { .. } => session_list_payload(),
            Command::SessionStatus { session_id, .. } => session_status_payload(session_id),
            Command::SessionRenew {
                session_id,
                lease,
                ttl_seconds,
                ..
            } => session_renew_payload(session_id, lease, *ttl_seconds),
            Command::SessionEnd {
                session_id,
                lease,
                confirm,
                ..
            } => session_end_payload(
                &self.open_runtime_coordinator()?,
                session_id,
                lease,
                *confirm,
            ),
            Command::LockAcquire {
                session_id,
                lease,
                lock_target,
                ttl_seconds,
                ..
            } => lock_acquire_payload(session_id, lease, lock_target, *ttl_seconds),
            Command::LockList { .. } => lock_list_payload(),
            Command::LockRelease { lock_id, lease, .. } => lock_release_payload(lock_id, lease),
            Command::JobSpawn {
                command,
                environment,
                cwd,
                ttl_seconds,
                ..
            } => {
                let request = job_request.ok_or_else(|| {
                    CuError::new(
                        "managed_job_request_identity_required",
                        "job-spawn requires request-id, session and session-lease",
                    )
                })?;
                job_spawn_payload(
                    command,
                    environment,
                    cwd.as_deref(),
                    *ttl_seconds,
                    request.session_id,
                    request.session_lease,
                    request.runtime,
                )
            }
            Command::JobList {
                state, offset, max, ..
            } => job_list_payload(*state, *offset, *max),
            Command::JobStatus { job_id, .. } => job_status_payload(job_id),
            Command::JobResources {
                job_id,
                generation,
                watch_ms,
                ..
            } => job_resources_payload(job_id, *generation, *watch_ms),
            Command::JobEvents {
                job_id,
                generation,
                stdout_cursor,
                stderr_cursor,
                timeout_ms,
                max_bytes,
                ..
            } => job_events_payload(
                job_id,
                *generation,
                stdout_cursor,
                stderr_cursor,
                *timeout_ms,
                *max_bytes,
            ),
            Command::JobOutput {
                job_id,
                generation,
                stream,
                cursor,
                max_bytes,
                ..
            } => job_output_payload(job_id, *generation, *stream, cursor, *max_bytes),
            Command::JobWrite {
                job_id,
                generation,
                data_base64,
                close_stdin,
                ..
            } => {
                let request = require_job_request(job_request, "job-write")?;
                job_write_payload(
                    job_id,
                    *generation,
                    data_base64,
                    *close_stdin,
                    request.session_id,
                )
            }
            Command::JobWait {
                job_id,
                generation,
                timeout_ms,
                expect_exit,
                ..
            } => job_wait_payload(job_id, *generation, *timeout_ms, *expect_exit),
            Command::JobStop {
                job_id,
                generation,
                grace_ms,
                expect_stopped,
                ..
            } => {
                let request = require_job_request(job_request, "job-stop")?;
                job_stop_payload(
                    job_id,
                    *generation,
                    *grace_ms,
                    *expect_stopped,
                    request.session_id,
                )
            }
            Command::JobRenew {
                job_id,
                generation,
                ttl_seconds,
                ..
            } => {
                let request = require_job_request(job_request, "job-renew")?;
                job_renew_payload(job_id, *generation, *ttl_seconds, request.session_id)
            }
            Command::FileCopy {
                source,
                destination,
                replace,
                apply,
                ..
            } => {
                let store = crate::file_transactions::FileTransactionStore::open()?;
                let plan = store.plan(source, destination, *replace)?;
                if *apply {
                    serde_json::to_value(store.apply(&plan)?).map_err(|error| {
                        CuError::new("file_transaction_reply_failed", error.to_string())
                    })
                } else {
                    serde_json::to_value(plan).map_err(|error| {
                        CuError::new("file_transaction_reply_failed", error.to_string())
                    })
                }
            }
            Command::FileMove {
                source,
                destination,
                replace,
                apply,
                ..
            } => {
                let store = crate::file_move_transactions::FileMoveStore::open()?;
                let plan = store.plan(source, destination, *replace)?;
                if *apply {
                    serde_json::to_value(store.apply(&plan)?).map_err(|error| {
                        CuError::new("file_transaction_reply_failed", error.to_string())
                    })
                } else {
                    serde_json::to_value(plan).map_err(|error| {
                        CuError::new("file_transaction_reply_failed", error.to_string())
                    })
                }
            }
            Command::FileTransaction {
                action,
                transaction_id,
                ..
            } => {
                // Both transaction kinds share one private receipt directory
                // and one id namespace; the durable receipt names its owner.
                let moves = crate::file_move_transactions::FileMoveStore::open()?;
                let receipt = if moves.peek_operation(transaction_id)?
                    == crate::file_move_transactions::OPERATION
                {
                    let receipt = match action {
                        FileTransactionAction::Status => moves.status(transaction_id),
                        FileTransactionAction::Rollback => moves.rollback(transaction_id),
                        FileTransactionAction::Recover => moves.recover(transaction_id),
                        FileTransactionAction::Finalize => moves.finalize(transaction_id),
                    }?;
                    serde_json::to_value(receipt)
                } else {
                    let store = crate::file_transactions::FileTransactionStore::open()?;
                    let receipt = match action {
                        FileTransactionAction::Status => store.status(transaction_id),
                        FileTransactionAction::Rollback => store.rollback(transaction_id),
                        FileTransactionAction::Recover => store.recover(transaction_id),
                        FileTransactionAction::Finalize => store.finalize(transaction_id),
                    }?;
                    serde_json::to_value(receipt)
                };
                receipt.map_err(|error| {
                    CuError::new("file_transaction_reply_failed", error.to_string())
                })
            }
            Command::Windows {
                pid,
                app,
                title,
                focused,
                minimized,
                browser_profile,
                offset,
                max,
                ..
            } => windows_payload(
                observe::WindowFilter {
                    pid: *pid,
                    app: app.clone(),
                    title: title.clone(),
                    focused: *focused,
                    minimized: *minimized,
                },
                browser_profile.clone(),
                *offset,
                *max,
            ),
            Command::WindowsWatch {
                pid,
                app,
                title,
                duration_ms,
                interval_ms,
                max_events,
                ..
            } => windows_watch_payload(
                observe::WindowFilter {
                    pid: *pid,
                    app: app.clone(),
                    title: title.clone(),
                    focused: None,
                    minimized: None,
                },
                *duration_ms,
                *interval_ms,
                *max_events,
            ),
            Command::Apps { all, .. } => apps_payload(*all),
            Command::Ps {
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
                offset,
                max,
                ..
            } => {
                if let (Some(pid), true) = (*pid, depth.is_some() || *files || *ports) {
                    process_tree_payload(
                        pid,
                        depth.unwrap_or(4),
                        max.unwrap_or(500),
                        *files,
                        *ports,
                        *max_visited,
                    )
                } else {
                    process_list_payload(ProcessInventoryOptions {
                        pid: *pid,
                        parent: *parent,
                        name: name.as_deref(),
                        app: app.as_deref(),
                        command: command.as_deref(),
                        cpu_above_percent: *cpu_above_percent,
                        memory_above_mb: *memory_above_mb,
                        sort: sort.as_deref(),
                        sample_ms: *sample_ms,
                        max_visited: *max_visited,
                        offset: *offset,
                        max: *max,
                    })
                }
            }
            Command::ProcessState { pid, .. } => process_state_payload(*pid),
            Command::ProcessArgv {
                pid,
                values,
                offset,
                limit,
                ..
            } => process_argv_payload(*pid, *values, *offset, *limit),
            Command::ProcessCwd { pid, .. } => process_cwd_payload(*pid),
            Command::ProcessEnvironment {
                pid,
                prefix,
                values,
                offset,
                limit,
                ..
            } => process_environment_payload(*pid, prefix.as_deref(), *values, *offset, *limit),
            Command::ProcessFds {
                pid,
                kind,
                target_filter,
                offset,
                limit,
                max_visited,
                ..
            } => process_fds_payload(
                *pid,
                kind.as_deref(),
                target_filter.as_deref(),
                *offset,
                *limit,
                *max_visited,
            ),
            Command::ProcessMaps {
                pid,
                path,
                permissions,
                executable_only,
                offset,
                limit,
                max_visited,
                ..
            } => process_maps_payload(
                *pid,
                path.as_deref(),
                permissions.as_deref(),
                *executable_only,
                *offset,
                *limit,
                *max_visited,
            ),
            Command::ProcessThreads {
                pid,
                name,
                state,
                offset,
                limit,
                max_visited,
                ..
            } => process_threads_payload(
                *pid,
                name.as_deref(),
                state.as_deref(),
                *offset,
                *limit,
                *max_visited,
            ),
            Command::ProcessSockets {
                pid,
                family,
                protocol,
                state,
                endpoint,
                offset,
                limit,
                max_visited,
                ..
            } => process_sockets_payload(
                *pid,
                family.as_deref(),
                protocol.as_deref(),
                state.as_deref(),
                endpoint.as_deref(),
                *offset,
                *limit,
                *max_visited,
            ),
            Command::ProcessCgroup {
                pid,
                start_identity,
                ..
            } => process_cgroup_payload(*pid, start_identity.as_deref()),
            Command::ProcessUsage {
                pid,
                watch_ms,
                interval_ms,
                max_samples,
                ..
            } => match watch_ms {
                Some(duration_ms) => {
                    process_usage_watch_payload(*pid, *duration_ms, *interval_ms, *max_samples)
                }
                None => process_usage_payload(*pid),
            },
            Command::ProcessWait {
                pid,
                start_identity,
                timeout_ms,
                ..
            } => process_wait_payload(*pid, start_identity, *timeout_ms),
            Command::ProcessKill {
                pid,
                start_identity,
                mode,
                timeout_ms,
                expect_exited,
                ..
            } => process_kill_payload(
                *pid,
                start_identity,
                *mode,
                *timeout_ms,
                *expect_exited,
                &mut self.open_receipts(command.target())?,
            ),
            Command::ProcessSetState {
                pid,
                start_identity,
                state,
                timeout_ms,
                ..
            } => process_set_state_payload(
                *pid,
                start_identity,
                *state,
                *timeout_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::ProcessSignal {
                pid,
                start_identity,
                signal,
                timeout_ms,
                force,
                tree,
                max_descendants,
                ..
            } => process_signal_payload(
                *pid,
                start_identity.as_deref(),
                *signal,
                ProcessSignalOptions {
                    timeout_ms: *timeout_ms,
                    force: *force,
                    tree: *tree,
                    max_descendants: *max_descendants,
                },
                &mut self.open_receipts(command.target())?,
            ),
            Command::PrivilegePlanProcessPriority {
                pid,
                nice,
                ttl_seconds,
                ..
            } => crate::privilege_plan::process_priority_plan_now(*pid, *nice, *ttl_seconds),
            Command::ProcessWatch {
                pid,
                parent,
                name,
                all,
                duration_ms,
                interval_ms,
                max_events,
                max_processes,
                ..
            } => process_watch_payload(
                *pid,
                *parent,
                name.as_deref(),
                *all,
                *duration_ms,
                *interval_ms,
                *max_events,
                *max_processes,
            ),
            Command::ShellExec {
                command,
                timeout_ms,
                max_output_bytes,
                ..
            } => shell_exec_payload(command, *timeout_ms, *max_output_bytes),
            Command::NetworkInterfaces { max, .. } => network_interfaces_payload(*max),
            Command::NetworkRoutes { max, .. } => network_routes_payload(*max),
            Command::NetworkDns { max, .. } => network_dns_payload(*max),
            Command::NetworkProbe {
                host,
                port,
                attempts,
                timeout_ms,
                ..
            } => network_probe::payload(host, *port, *attempts, *timeout_ms),
            Command::FileInspect { path, .. } => file_inspect_payload(path),
            Command::PtyStart {
                name,
                cwd,
                command: child_command,
                ..
            } => pty_start_payload(
                name,
                cwd.as_deref(),
                child_command,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PtyList { .. } => pty_list_payload(),
            Command::PtyPrune {
                name, expect_stale, ..
            } => pty_prune_payload(
                name,
                *expect_stale,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PtyStatus { name, .. } => pty_status_payload(name),
            Command::PtyRead {
                name,
                cursor,
                max_bytes,
                ..
            } => pty_read_payload(name, cursor, *max_bytes),
            Command::PtySnapshot { name, .. } => {
                pty_snapshot_payload(name, &self.pty_snapshot_store()?)
            }
            Command::PtyDiff {
                name,
                base,
                advance,
                max,
                ..
            } => pty_diff_payload(name, base, *advance, *max, &self.pty_snapshot_store()?),
            Command::PtyEvents {
                name,
                epoch,
                after,
                limit,
                ..
            } => pty_events_payload(name, epoch, *after, *limit),
            Command::PtyResize {
                name,
                rows,
                columns,
                ..
            } => pty_resize_payload(
                name,
                *rows,
                *columns,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PtySend { name, text, .. } => {
                pty_send_payload(name, text, &mut self.open_receipts(command.target())?)
            }
            Command::PtyWait {
                name,
                contains,
                cursor,
                timeout_ms,
                ..
            } => pty_wait_payload(name, contains, cursor, *timeout_ms),
            Command::PtyWaitExit {
                name,
                timeout_ms,
                expect_status,
                ..
            } => pty_wait_exit_payload(name, *timeout_ms, *expect_status),
            Command::PtySignal {
                name,
                signal,
                expect,
                ..
            } => pty_signal_payload(
                name,
                *signal,
                expect,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PtyStop {
                name,
                expect_stopped,
                ..
            } => pty_stop_payload(
                name,
                *expect_stopped,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TerminalList { .. } => terminal_list_payload(),
            Command::TerminalNew {
                title,
                parent,
                detached,
                command: child_command,
                ..
            } => terminal_new_payload(
                title.as_deref(),
                parent.as_deref(),
                *detached,
                child_command,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TerminalClose {
                tab, expect_closed, ..
            } => terminal_close_payload(
                tab,
                *expect_closed,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TerminalRead { tab, max_bytes, .. } => terminal_read_payload(tab, *max_bytes),
            Command::TerminalSnapshot { tab, .. } => terminal_snapshot_payload(tab),
            Command::TerminalScroll {
                tab, action, rows, ..
            } => terminal_scroll_payload(
                tab,
                *action,
                *rows,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TerminalScreenshot { tab, out, .. } => terminal_screenshot_payload(tab, out),
            Command::TerminalEvents {
                tab,
                epoch,
                after,
                limit,
                ..
            } => terminal_events_payload(tab, epoch, *after, *limit),
            Command::TerminalOutput {
                tab,
                cursor,
                max_bytes,
                ..
            } => terminal_output_payload(tab, cursor, *max_bytes),
            Command::TerminalSend { tab, text, .. } => {
                terminal_send_payload(tab, text, &mut self.open_receipts(command.target())?)
            }
            Command::TerminalWait {
                tab,
                condition,
                timeout_ms,
                ..
            } => terminal_wait_payload(tab, condition, *timeout_ms),
            Command::TermRead {
                window,
                tail,
                raw,
                max_bytes,
                ..
            } => term_read_payload(*window, *tail, *raw, *max_bytes),
            Command::TermSend {
                window,
                text,
                expect,
                enter,
                foreground,
                verify_timeout_ms,
                ..
            } => term_send_payload(
                *window,
                text,
                expect.as_deref(),
                *enter,
                *foreground,
                *verify_timeout_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TermWait {
                window,
                pattern,
                timeout_ms,
                interval_ms,
                max_bytes,
                ..
            } => term_wait_payload(*window, pattern, *timeout_ms, *interval_ms, *max_bytes),
            Command::Tree {
                window,
                depth,
                max_nodes,
                flat,
                ..
            } => tree_payload(*window, *depth, *max_nodes, *flat),
            Command::DesktopState {
                window,
                depth,
                max_nodes,
                ..
            } => desktop_state_payload(*window, *depth, *max_nodes),
            Command::Query {
                window,
                depth,
                max_nodes,
                role,
                text,
                text_exact,
                identifier,
                actionable,
                within,
                offset,
                max,
                selector,
                ..
            } => query_payload(
                *window,
                *depth,
                *max_nodes,
                observe::NodeFilter::from_parts(
                    role,
                    text.as_deref(),
                    text_exact.as_deref(),
                    identifier.as_deref(),
                    *actionable,
                    *within,
                ),
                text.is_some() && text_exact.is_some(),
                *offset,
                *max,
                selector.as_deref(),
            ),
            Command::Invoke {
                window,
                node,
                index,
                name,
                identifier,
                role,
                action,
                value,
                focused,
                selector,
                ..
            } => invoke_payload(
                *window,
                observe::TargetSpec {
                    node: node.clone(),
                    index: *index,
                    name: name.clone(),
                    identifier: identifier.clone(),
                    role: role.clone(),
                    focused: *focused,
                },
                *action,
                value.as_deref(),
                selector.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::MenuInspect {
                window,
                depth,
                max_nodes,
                title,
                exact,
                enabled,
                offset,
                max,
                ..
            } => menu_inspect_payload(
                *window,
                *depth,
                *max_nodes,
                observe::MenuFilter {
                    title: title.clone(),
                    exact: *exact,
                    enabled: *enabled,
                },
                *offset,
                *max,
            ),
            Command::MenuInvoke { window, path, .. } => {
                menu_invoke_payload(*window, path, &mut self.open_receipts(command.target())?)
            }
            Command::Focused {
                window,
                role,
                max_value_bytes,
                ..
            } => focused_payload(*window, role.as_deref(), *max_value_bytes),
            Command::Observe {
                window,
                duration_ms,
                ready_path,
                depth,
                max_nodes,
                max_events,
                notifications,
                interval_ms,
                mode,
                ..
            } => observe_payload(
                *window,
                *duration_ms,
                ready_path.as_deref(),
                *depth,
                *max_nodes,
                *max_events,
                notifications,
                *interval_ms,
                mode.as_deref(),
            ),
            Command::Verify { window, expect, .. } => verify_payload(*window, expect),
            Command::PageJs {
                expression,
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                ..
            } => page_js_payload(
                expression.as_deref(),
                match pid {
                    Some(pid) => Some(resolve_cdp_port(*port, Some(*pid))?),
                    None => *port,
                },
                cdp_selector(target_id, target_url, target_title, target_match),
            ),
            Command::PageTargets {
                port,
                pid,
                browser_profile,
                ..
            } => page_targets_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                browser_profile.as_deref(),
            ),
            Command::PageText {
                window,
                max_bytes,
                within,
                depth,
                max_nodes,
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                ..
            } => page_text_payload(
                *window,
                *max_bytes,
                *within,
                *depth,
                *max_nodes,
                match pid {
                    Some(pid) => Some(resolve_cdp_port(*port, Some(*pid))?),
                    None => *port,
                },
                cdp_selector(target_id, target_url, target_title, target_match),
            ),
            Command::PageFind {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                selector,
                text,
                role,
                name,
                ..
            } => page_find_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                selector.as_deref(),
                text.as_deref(),
                role.as_deref(),
                name.as_deref(),
            ),
            Command::PageClick {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                selector,
                text,
                node,
                x,
                y,
                button,
                clicks,
                ..
            } => page_click_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                selector.as_deref(),
                text.as_deref(),
                *node,
                *x,
                *y,
                button.as_deref(),
                *clicks,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageDownload {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                selector,
                text,
                node,
                download_dir,
                wait_ms,
                ..
            } => page_download_payload(
                resolve_cdp_port(*port, *pid)?,
                cdp_selector(target_id, target_url, target_title, target_match),
                selector.as_deref(),
                text.as_deref(),
                *node,
                download_dir,
                *wait_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageHover {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                x,
                y,
                ..
            } => page_hover_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                *x,
                *y,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageScroll {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                x,
                y,
                dx,
                dy,
                ..
            } => page_scroll_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                *x,
                *y,
                dx.unwrap_or(0.0),
                dy.unwrap_or(120.0),
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageDrag {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                x1,
                y1,
                x2,
                y2,
                ..
            } => page_drag_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                *x1,
                *y1,
                *x2,
                *y2,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageDialog {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                dismiss,
                text,
                wait_ms,
                ..
            } => page_dialog_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                *dismiss,
                text.as_deref(),
                *wait_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageFiles {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                selector,
                node,
                files,
                ..
            } => page_files_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                selector.as_deref(),
                *node,
                files,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageFill {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                selector,
                node,
                text,
                clear,
                submit,
                ..
            } => page_fill_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                selector.as_deref(),
                *node,
                text,
                *clear,
                *submit,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageType {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                text,
                ..
            } => page_type_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                text,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageNav {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                url,
                wait_ms,
                ..
            } => page_nav_payload(
                Some(resolve_cdp_port(*port, *pid)?),
                cdp_selector(target_id, target_url, target_title, target_match),
                url,
                *wait_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageScreenshot {
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                out,
                replace,
                activate,
                ..
            } => {
                let mut receipts = if *activate {
                    Some(self.open_receipts(command.target())?)
                } else {
                    None
                };
                page_screenshot_payload(
                    Some(resolve_cdp_port(*port, *pid)?),
                    cdp_selector(target_id, target_url, target_title, target_match),
                    out,
                    *replace,
                    *activate,
                    receipts.as_mut(),
                )
            }
            Command::TabList { window, .. } => tab_list_payload(*window),
            Command::TabSelect {
                window,
                title,
                index,
                ..
            } => tab_select_payload(
                *window,
                title.as_deref(),
                *index,
                &mut self.open_receipts(command.target())?,
            ),
            Command::TabClose {
                window,
                title,
                index,
                exact,
                expect,
                port,
                ..
            } => tab_close_payload(
                *window,
                title.as_deref(),
                *index,
                *exact,
                expect.as_deref(),
                *port,
                &mut self.open_receipts(command.target())?,
            ),
            Command::BrowserProfiles { app, .. } => browser_profiles_payload(app.as_deref()),
            Command::BrowserOpen {
                profile,
                url,
                app,
                timeout_ms,
                ..
            } => browser_open_payload(
                profile,
                url.as_deref(),
                app.as_deref(),
                *timeout_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::BrowserSessionStart {
                name,
                browser,
                ready_timeout_ms,
                ttl_ms,
                ..
            } => browser_session_start_payload(name, browser, *ready_timeout_ms, *ttl_ms),
            Command::BrowserSessionList { .. } => browser_session_list_payload(),
            Command::BrowserSessionStatus { name, .. } => browser_session_status_payload(name),
            Command::BrowserSessionStop {
                name,
                expect_stopped,
                timeout_ms,
                ..
            } => browser_session_stop_payload(name, *expect_stopped, *timeout_ms),
            Command::BrowserSessionRemove {
                name,
                expect_stopped,
                expect_failed,
                ..
            } => browser_session_remove_payload(name, *expect_stopped, *expect_failed),
            Command::BrowserBridgeSetup { .. } => browser_bridge_setup_payload(),
            Command::BrowserBridgeConnections { .. } => browser_bridge_connections_payload(),
            Command::BrowserBridgeStatus { connection_id, .. } => {
                browser_bridge_request_payload(connection_id, "status", serde_json::Map::new())
            }
            Command::BrowserBridgeTabs { connection_id, .. } => {
                browser_bridge_request_payload(connection_id, "tabs", serde_json::Map::new())
            }
            Command::BrowserBridgeDebugRead {
                connection_id,
                tab_id,
                max_frames,
                max_depth,
                max_scan,
                max_results,
                ..
            } => browser_bridge_debug_read_payload(
                connection_id,
                *tab_id,
                *max_frames,
                *max_depth,
                *max_scan,
                *max_results,
            ),
            Command::App {
                window,
                action,
                snapshot,
                expect,
                pid,
                path,
                ..
            } => app_payload(
                *window,
                *action,
                *snapshot,
                expect.as_deref(),
                *pid,
                path.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Spaces { .. } => spaces_payload(),
            Command::Displays { .. } => displays_payload(),
            Command::Unlock { window, .. } => unlock_payload(*window),
            Command::Align { group, .. } => Err(CuError::new(
                "unsupported",
                crate::mcu_surface::typed_reason_for_verb(group),
            )
            .with_detail(serde_json::json!({
                "verb": group,
                "group": crate::mcu_surface::group_id_for_verb(group),
                "os": crate::mcu_surface::host_os(),
            }))),
            Command::Screenshot { path, window, .. } => screenshot(path, *window),
            Command::DeviceScreenshot {
                path,
                device,
                timeout_ms,
                list,
                ..
            } => device_screenshot_payload(path.as_deref(), device.as_deref(), *timeout_ms, *list),
            Command::ResourceStatus { .. } => resource_status_payload(),
            Command::PowerStatus { .. } => power_status_payload(),
            Command::StorageDevices { max, .. } => storage_devices_payload(*max),
            Command::DeviceList { selector, max, .. } => device_inventory_payload(*selector, *max),
            Command::DeviceWatch {
                selector,
                max,
                duration_ms,
                interval_ms,
                event_max,
                ..
            } => device_watch_payload(*selector, *max, *duration_ms, *interval_ms, *event_max),
            Command::DeviceClaims { offset, max, .. } => device_claims_payload(*offset, *max),
            Command::DeviceClaim {
                device_id,
                ttl_seconds,
                serial,
                ..
            } => device_claim_payload(
                device_id,
                *ttl_seconds,
                serial.as_ref(),
                require_job_request(job_request, "device-claim")?,
            ),
            Command::DeviceStatus {
                lease_id,
                generation,
                ..
            } => device_status_payload(lease_id, *generation),
            Command::DeviceRead {
                lease_id,
                generation,
                lease,
                max_bytes,
                timeout_ms,
                encoding,
                ..
            } => device_read_payload(
                lease_id,
                *generation,
                lease,
                *max_bytes,
                *timeout_ms,
                *encoding,
                require_job_request(job_request, "device-read")?,
            ),
            Command::DeviceWrite {
                lease_id,
                generation,
                lease,
                data,
                encoding,
                timeout_ms,
                ..
            } => device_write_payload(
                lease_id,
                *generation,
                lease,
                data,
                *encoding,
                *timeout_ms,
                require_job_request(job_request, "device-write")?,
            ),
            Command::DeviceRenew {
                lease_id,
                generation,
                lease,
                ttl_seconds,
                ..
            } => device_renew_payload(
                lease_id,
                *generation,
                lease,
                *ttl_seconds,
                require_job_request(job_request, "device-renew")?,
            ),
            Command::DeviceRelease {
                lease_id,
                generation,
                lease,
                ..
            } => device_release_payload(
                lease_id,
                *generation,
                lease,
                require_job_request(job_request, "device-release")?,
            ),
            Command::SimulatorDevices { max, .. } => simulator_devices_payload(*max),
            Command::SimulatorBoot {
                udid,
                timeout_ms,
                expect_booted,
                ..
            } => simulator_boot_payload(udid, *timeout_ms, *expect_booted),
            Command::SimulatorApps { udid, max, .. } => simulator_apps_payload(udid, *max),
            Command::SimulatorLaunch {
                udid,
                bundle_id,
                timeout_ms,
                expect_accepted,
                ..
            } => simulator_app_lifecycle_payload(
                udid,
                bundle_id,
                *timeout_ms,
                *expect_accepted,
                agenterm_platform::simulator::SimulatorAppAction::Launch,
            ),
            Command::SimulatorTerminate {
                udid,
                bundle_id,
                timeout_ms,
                expect_accepted,
                ..
            } => simulator_app_lifecycle_payload(
                udid,
                bundle_id,
                *timeout_ms,
                *expect_accepted,
                agenterm_platform::simulator::SimulatorAppAction::Terminate,
            ),
            Command::PointerMove { x, y, .. } => pointer_move(*x, *y),
            Command::PointerPosition { .. } => pointer_position(),
            Command::Click { .. } => {
                click_command(command, &mut self.open_receipts(command.target())?)
            }
            Command::Focus {
                window,
                node,
                name,
                role,
                ..
            } => focus(
                *window,
                node.as_deref(),
                name.as_deref(),
                role.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::SendText {
                text,
                window,
                name,
                role,
                ..
            } => send_text(
                text,
                *window,
                name.as_deref(),
                role.as_deref(),
                allow_browser_chrome(command),
                &mut self.open_receipts(command.target())?,
            ),
            Command::ClipboardRead {
                metadata_only,
                type_name,
                max_bytes,
                out,
                replace,
                ..
            } => {
                if *metadata_only {
                    clipboard_metadata()
                } else if let Some(type_name) = type_name {
                    clipboard_read_type(type_name, *max_bytes, out.as_deref(), *replace)
                } else {
                    clipboard_read()
                }
            }
            Command::ClipboardWrite {
                type_name, path, ..
            } => clipboard_write(type_name, path),
            Command::ClipboardWriteFile { path, .. } => clipboard_write_file(path),
            Command::ClipboardClear { apply, .. } => clipboard_clear(*apply),
            Command::Copy {
                window, name, role, ..
            } => copy(*window, name.as_deref(), role.as_deref()),
            Command::Paste {
                text,
                window,
                name,
                role,
                ..
            } => paste(
                text.as_deref(),
                *window,
                name.as_deref(),
                role.as_deref(),
                allow_browser_chrome(command),
                &mut self.open_receipts(command.target())?,
            ),
            Command::SendKeys {
                keys,
                window,
                name,
                role,
                ..
            } => send_keys(
                keys,
                *window,
                name.as_deref(),
                role.as_deref(),
                allow_browser_chrome(command),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Scroll {
                window, name, role, ..
            } => scroll(*window, name.as_deref(), role.as_deref()),
            Command::GetExtents {
                window, name, role, ..
            } => get_extents(*window, name.as_deref(), role.as_deref()),
            Command::Select {
                start,
                end,
                window,
                name,
                role,
                ..
            } => select(*window, name.as_deref(), role.as_deref(), *start, *end),
            Command::GetSelection {
                window, name, role, ..
            } => get_selection(*window, name.as_deref(), role.as_deref()),
            Command::SetCaret {
                offset,
                window,
                name,
                role,
                ..
            } => set_caret(*window, name.as_deref(), role.as_deref(), *offset),
            Command::GetCaret {
                window, name, role, ..
            } => get_caret(*window, name.as_deref(), role.as_deref()),
            Command::GetText {
                window, name, role, ..
            } => get_text(*window, name.as_deref(), role.as_deref()),
            Command::Wait {
                timeout_ms,
                condition,
                ..
            } => wait(*timeout_ms, condition),
            Command::WindowPlace {
                action,
                window,
                frame,
                ..
            } => window_place(action, *window, *frame),
            Command::OrderWin {
                window,
                relation,
                relative,
                ..
            } => orderwin_payload(*window, *relation, *relative),
            Command::Close {
                window,
                pid,
                title,
                snapshot,
                expect,
                ..
            } => close_payload(
                *window,
                *pid,
                title.as_deref(),
                *snapshot,
                expect.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Activate { window, .. } => {
                activate_payload(*window, &mut self.open_receipts(command.target())?)
            }
            Command::Raise { window, .. } => {
                raise_payload(*window, &mut self.open_receipts(command.target())?)
            }
            Command::Minimize { window, expect, .. } => window_state_payload(
                WindowState::Minimized,
                *window,
                expect.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Restore { window, expect, .. } => window_state_payload(
                WindowState::Restored,
                *window,
                expect.as_deref(),
                &mut self.open_receipts(command.target())?,
            ),
            Command::Drag {
                window,
                from,
                to,
                button,
                steps,
                degraded,
                ..
            } => drag_payload(
                *window,
                *from,
                *to,
                *button,
                *steps,
                *degraded,
                &mut self.open_receipts(command.target())?,
            ),
            Command::Hit {
                window,
                x,
                y,
                depth,
                max_nodes,
                ..
            } => hit_payload(*window, *x, *y, *depth, *max_nodes),
            Command::Zoom {
                window,
                region,
                out,
                replace,
                pad,
                ..
            } => zoom_payload(*window, *region, out, *replace, *pad),
            Command::Snapshot {
                window,
                depth,
                max_nodes,
                out,
                ..
            } => snapshot_payload(
                &self.snapshot_store()?,
                command.target(),
                *window,
                *depth,
                *max_nodes,
                out.as_deref(),
            ),
            Command::Diff {
                window,
                base,
                advance,
                max,
                ..
            } => diff_payload(
                &self.snapshot_store()?,
                command.target(),
                *window,
                base.as_deref(),
                *advance,
                *max,
            ),
            Command::Receipts { window, max, .. } => {
                receipts_payload(&self.receipt_dir()?, command.target(), *window, *max)
            }
        }
    }
}

fn audio_plan_payload(plan: crate::audio_control::AudioPlan) -> Result<serde_json::Value, CuError> {
    let request = crate::audio_control::encode_request(&plan)?;
    let reverse = crate::audio_control::reverse_plan(&plan)?;
    let rollback_request = crate::audio_control::encode_request(&reverse)?;
    let mut value = serde_json::to_value(&plan).map_err(|_| {
        CuError::new(
            "audio_serialization_failed",
            "audio plan could not be serialized",
        )
    })?;
    value["request"] = serde_json::Value::String(request);
    value["approval"] = serde_json::Value::String(plan.approval_digest.clone());
    value["rollback_request"] = serde_json::json!({
        "request": rollback_request,
        "approval": reverse.approval_digest,
        "plan": reverse,
    });
    Ok(value)
}

fn service_plan_payload(
    plan: crate::service_control::ServicePlan,
) -> Result<serde_json::Value, CuError> {
    let request = crate::service_control::encode_request(&plan)?;
    let mut value = serde_json::to_value(&plan).map_err(|_| {
        CuError::new(
            "service_serialization_failed",
            "service plan could not be serialized",
        )
    })?;
    value["request"] = serde_json::Value::String(request);
    value["approval"] = serde_json::Value::String(plan.approval_digest.clone());
    Ok(value)
}

fn service_transaction_payload(
    request: &JobRequestContext<'_>,
    scope: crate::service_control::ServiceScope,
    operation: crate::service_control::ServiceOperation,
    name: Option<&str>,
    definition: Option<&str>,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    use crate::service_control::{self, ServiceOperation};

    let (name, definition) = if operation == ServiceOperation::Bootstrap {
        let path = std::path::PathBuf::from(definition.ok_or_else(|| {
            CuError::new(
                "service_definition_required",
                "service bootstrap requires a definition",
            )
        })?);
        let binding = service_control::definition_binding(&path)?;
        (binding.declared_name, Some(path))
    } else {
        (
            name.ok_or_else(|| {
                CuError::new(
                    "service_name_required",
                    "service lifecycle requires a service name",
                )
            })?
            .to_owned(),
            None,
        )
    };
    let identity = service_control::identity(scope, &name)?;
    let lock_target = format!(
        "service:{}:{}/{}",
        identity.provider, identity.provider_scope, identity.name
    );
    let now_s = now_utc_ms()
        .ok_or_else(|| CuError::new("service_clock_invalid", "system clock is unavailable"))?
        / 1_000;
    request.runtime.lock_acquire(
        request.session_id,
        request.session_lease,
        &lock_target,
        ttl_seconds,
        now_s,
    )?;
    let plan = service_control::plan(&identity, operation, definition, ttl_seconds)?;
    let receipt = service_control::apply(&plan, &plan.approval_digest)?;
    Ok(serde_json::json!({
        "operation": operation,
        "identity": identity,
        "before": plan.before,
        "receipt": receipt,
        "target_lock": lock_target,
    }))
}

fn require_job_request<'a>(
    request: Option<&'a JobRequestContext<'_>>,
    verb: &str,
) -> Result<&'a JobRequestContext<'a>, CuError> {
    request.ok_or_else(|| {
        CuError::new(
            "managed_job_request_identity_required",
            format!("{verb} requires request-id, session and session-lease"),
        )
    })
}

/// `--allow-browser-chrome` for the focused text writers (`send-text`,
/// `paste`, `send-keys` with `--window` and no `--name`): `true` writes
/// browser chrome (the omnibox, a toolbar field) deliberately instead of
/// refusing `focused_node_is_browser_chrome`.
///
/// Reads the `Command::{SendText, Paste, SendKeys}` field `allow_browser_chrome`
/// (`#[serde(default)]`; `--allow-browser-chrome` on the CLI). Every other
/// command runs with the guard armed.
fn allow_browser_chrome(command: &Command) -> bool {
    match command {
        Command::SendText {
            allow_browser_chrome,
            ..
        }
        | Command::Paste {
            allow_browser_chrome,
            ..
        }
        | Command::SendKeys {
            allow_browser_chrome,
            ..
        } => *allow_browser_chrome,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard is armed unless the command carries `allow_browser_chrome`;
    /// commands without the field never lift it.
    #[test]
    fn browser_chrome_guard_follows_the_command_field() {
        for allow in [false, true] {
            let commands = [
                Command::SendText {
                    target: TargetRef::Current,
                    text: "CODE".into(),
                    window: Some(7),
                    name: None,
                    role: None,
                    allow_browser_chrome: allow,
                },
                Command::Paste {
                    target: TargetRef::Current,
                    text: None,
                    window: Some(7),
                    name: None,
                    role: None,
                    allow_browser_chrome: allow,
                },
                Command::SendKeys {
                    target: TargetRef::Current,
                    keys: "CODE".into(),
                    window: Some(7),
                    name: None,
                    role: None,
                    allow_browser_chrome: allow,
                },
            ];
            for command in &commands {
                assert_eq!(allow_browser_chrome(command), allow, "{}", command.verb());
            }
        }
        let other = Command::Copy {
            target: TargetRef::Current,
            window: Some(7),
            name: None,
            role: None,
        };
        assert!(!allow_browser_chrome(&other));
    }
}
