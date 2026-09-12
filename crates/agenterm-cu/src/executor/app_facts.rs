//! Product projection for native application facts.

use super::*;

pub(super) fn app_facts_payload(
    selector: &str,
    signing: bool,
    verify: bool,
    entitlements: bool,
) -> Result<serde_json::Value, CuError> {
    let options = agenterm_platform::app_facts::AppFactsOptions {
        signing,
        verify,
        entitlements,
    };
    let facts = query_app_facts(selector, options).map_err(app_facts_error)?;
    serde_json::to_value(facts).map_err(|_| {
        CuError::new(
            "app_facts_serialization_failed",
            "application facts could not be serialized",
        )
    })
}

pub(super) fn query_app_facts(
    selector: &str,
    options: agenterm_platform::app_facts::AppFactsOptions,
) -> Result<agenterm_platform::app_facts::AppFacts, agenterm_platform::app_facts::AppFactsError> {
    let candidates = app_facts_selector_candidates(selector);
    let mut last_not_found = None;
    for candidate in candidates {
        match agenterm_platform::app_facts::query(&candidate, options) {
            Ok(facts) => return Ok(facts),
            Err(error)
                if error.kind() == agenterm_platform::app_facts::AppFactsErrorKind::NotFound =>
            {
                last_not_found = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_not_found.unwrap_or_else(|| {
        agenterm_platform::app_facts::AppFactsError::new(
            agenterm_platform::app_facts::AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            app_facts_not_found_message(selector),
        )
    }))
}

fn app_facts_selector_candidates(selector: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, selector);
    #[cfg(target_os = "linux")]
    linux_app_facts_selector_candidates(selector, &mut candidates);
    candidates
}

#[cfg(target_os = "linux")]
fn linux_app_facts_selector_candidates(selector: &str, candidates: &mut Vec<String>) {
    if !selector.contains('/') && !selector.contains('\\') && !selector.ends_with(".desktop") {
        push_unique_candidate(candidates, format!("{selector}.desktop"));
    }
    if let Ok((installed, _)) = mechanism::list_installed_apps() {
        let wanted = selector.to_lowercase();
        for app in installed {
            let path = std::path::Path::new(&app.path);
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let stem = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let aliases = [
                file_name.to_ascii_lowercase(),
                stem.to_ascii_lowercase(),
                app.name.to_ascii_lowercase(),
            ];
            let exec_alias =
                read_desktop_exec_basename(&app.path).map(|name| name.to_ascii_lowercase());
            let matches =
                aliases.contains(&wanted) || exec_alias.as_deref() == Some(wanted.as_str());
            if matches {
                push_unique_candidate(candidates, file_name);
                push_unique_candidate(candidates, &app.path);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn read_desktop_exec_basename(path: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let exec = content
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Exec=").map(str::trim))?;
    let token = first_exec_token(exec)?;
    std::path::Path::new(&token)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(target_os = "linux")]
fn first_exec_token(exec: &str) -> Option<String> {
    let exec = exec.trim_start();
    if exec.is_empty() {
        return None;
    }
    if let Some(rest) = exec.strip_prefix('"') {
        let mut token = String::new();
        let mut escaped = false;
        for character in rest.chars() {
            if escaped {
                token.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Some(token);
            } else {
                token.push(character);
            }
        }
        return None;
    }
    Some(exec.split_whitespace().next()?.replace("\\ ", " "))
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: impl Into<String>) {
    let candidate = candidate.into();
    if candidate.is_empty() {
        return;
    }
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn app_facts_not_found_message(selector: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        format!(
            "application selector '{selector}' matched no desktop entry id (for example \
             xfce4-terminal.desktop), normalized path, exact Name, process name, or installed \
             application alias; app-inspect --app accepts the same process-name substring"
        )
    }
    #[cfg(target_os = "macos")]
    {
        format!(
            "application selector '{selector}' matched no installed application bundle id, \
             normalized path, or exact display name"
        )
    }
    #[cfg(target_os = "windows")]
    {
        format!(
            "application selector '{selector}' matched no installed Uninstall key id, \
             normalized executable path, or exact display name"
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        format!("application selector '{selector}' matched no native application")
    }
}

pub(super) fn app_facts_error(error: agenterm_platform::app_facts::AppFactsError) -> CuError {
    CuError::new(error.code(), error.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_error_codes_survive_the_product_boundary() {
        let error = agenterm_platform::app_facts::AppFactsError::new(
            agenterm_platform::app_facts::AppFactsErrorKind::Ambiguous,
            "app_facts_ambiguous",
            "more than one app matched",
        );
        let projected = app_facts_error(error);
        assert_eq!(projected.code, "app_facts_ambiguous");
        assert_eq!(projected.message, "more than one app matched");
    }

    #[test]
    fn selector_candidates_keep_original_first() {
        let candidates = app_facts_selector_candidates("example");
        assert_eq!(candidates.first().map(String::as_str), Some("example"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_selector_candidates_add_desktop_suffix() {
        let candidates = app_facts_selector_candidates("xfce4-terminal");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate == "xfce4-terminal.desktop")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_exec_token_parser_handles_quoted_paths() {
        assert_eq!(
            first_exec_token("\"/usr/bin/xfce4-terminal\" --default-working-directory"),
            Some("/usr/bin/xfce4-terminal".into())
        );
        assert_eq!(
            first_exec_token("/usr/bin/firefox-esr --class Firefox"),
            Some("/usr/bin/firefox-esr".into())
        );
    }
}
