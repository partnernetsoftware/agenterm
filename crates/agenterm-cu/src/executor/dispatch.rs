//! `Command` -> verb-family payload builder. One exhaustive match so a
//! new `Command` variant is a compile error here, not a silent `unsupported`.

use super::*;

impl Executor {
    pub(super) fn run_current(&self, command: &Command) -> Result<serde_json::Value, CuError> {
        match command {
            Command::Capabilities { .. } => Ok(capabilities_payload()),
            Command::Permissions { .. } => Ok(permissions_payload()),
            Command::Doctor { .. } => Ok(doctor_payload()),
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
                offset,
                max,
                ..
            } => process_list_payload(*pid, *parent, name.as_deref(), *offset, *max),
            Command::ProcessState { pid, .. } => process_state_payload(*pid),
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
            Command::Tree {
                window,
                depth,
                max_nodes,
                flat,
                ..
            } => tree_payload(*window, *depth, *max_nodes, *flat),
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
                target_id,
                target_url,
                target_title,
                ..
            } => page_js_payload(
                expression.as_deref(),
                *port,
                cdp_selector(target_id, target_url, target_title),
            ),
            Command::PageTargets {
                port,
                browser_profile,
                ..
            } => page_targets_payload(*port, browser_profile.as_deref()),
            Command::PageText {
                window,
                max_bytes,
                within,
                depth,
                max_nodes,
                port,
                target_id,
                target_url,
                target_title,
                ..
            } => page_text_payload(
                *window,
                *max_bytes,
                *within,
                *depth,
                *max_nodes,
                *port,
                cdp_selector(target_id, target_url, target_title),
            ),
            Command::PageFind {
                port,
                target_id,
                target_url,
                target_title,
                selector,
                text,
                role,
                name,
                ..
            } => page_find_payload(
                *port,
                cdp_selector(target_id, target_url, target_title),
                selector.as_deref(),
                text.as_deref(),
                role.as_deref(),
                name.as_deref(),
            ),
            Command::PageClick {
                port,
                target_id,
                target_url,
                target_title,
                selector,
                text,
                node,
                button,
                clicks,
                ..
            } => page_click_payload(
                *port,
                cdp_selector(target_id, target_url, target_title),
                selector.as_deref(),
                text.as_deref(),
                *node,
                button.as_deref(),
                *clicks,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageFill {
                port,
                target_id,
                target_url,
                target_title,
                selector,
                node,
                text,
                clear,
                submit,
                ..
            } => page_fill_payload(
                *port,
                cdp_selector(target_id, target_url, target_title),
                selector.as_deref(),
                *node,
                text,
                *clear,
                *submit,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageNav {
                port,
                target_id,
                target_url,
                target_title,
                url,
                wait_ms,
                ..
            } => page_nav_payload(
                *port,
                cdp_selector(target_id, target_url, target_title),
                url,
                *wait_ms,
                &mut self.open_receipts(command.target())?,
            ),
            Command::PageScreenshot {
                port,
                target_id,
                target_url,
                target_title,
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
                    *port,
                    cdp_selector(target_id, target_url, target_title),
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
                type_name,
                max_bytes,
                out,
                replace,
                ..
            } => {
                if let Some(type_name) = type_name {
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
