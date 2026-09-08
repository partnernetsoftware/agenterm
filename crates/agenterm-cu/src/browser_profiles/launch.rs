//! Platform launch argv for `browser open` and inventory matching helpers.

use super::{BrowserApp, discovery};

/// Infer a profile name for a Chromium window when the title omits the suffix.
pub fn inferred_browser_profile(
    window_app_name: &str,
    title: &str,
    app: &BrowserApp,
    entries: &[super::ProfileEntry],
) -> Option<String> {
    if let Some(profile) = crate::observe::browser_profile_from_identity(window_app_name, title) {
        return Some(profile);
    }
    if !cfg!(target_os = "linux") || !window_matches_catalog_app(window_app_name, app) {
        return None;
    }
    if entries.len() == 1 {
        return Some(entries[0].name.clone());
    }
    entries
        .iter()
        .find(|entry| entry.last_used)
        .map(|entry| entry.name.clone())
}

/// Whether a window inventory `app_name` belongs to a catalog browser.
pub fn window_matches_catalog_app(window_app_name: &str, app: &BrowserApp) -> bool {
    if window_app_name == app.name {
        return true;
    }
    let window = window_app_name.to_ascii_lowercase();
    match app.name {
        "Google Chrome" => {
            window.contains("chrome")
                || window == "google-chrome"
                || window == "google-chrome-stable"
        }
        "Brave Origin" => window.contains("brave") && window.contains("origin"),
        "Brave Browser" => window.contains("brave") && !window.contains("origin"),
        _ => false,
    }
}

/// Catalog applications represented in a window inventory name list.
pub fn running_catalog_apps(running: &[String]) -> Vec<&'static BrowserApp> {
    use super::APPS;

    let mut found: Vec<&'static BrowserApp> = Vec::new();
    for name in running {
        if let Some(exact) = APPS.iter().find(|app| app.name == *name) {
            if !found.iter().any(|app| app.name == exact.name) {
                found.push(exact);
            }
            continue;
        }
        if let Some(app) = APPS
            .iter()
            .find(|app| window_matches_catalog_app(name, app))
            && !found.iter().any(|existing| existing.name == app.name)
        {
            found.push(app);
        }
    }
    found
}

/// How `browser open` launches on this host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchPlan {
    pub mechanism: &'static str,
    pub argv: Vec<String>,
}

/// Build the argv `browser open` runs on this host.
pub fn open_launch_plan(
    app: &BrowserApp,
    directory: &str,
    url: Option<&str>,
) -> Result<LaunchPlan, String> {
    if cfg!(target_os = "macos") {
        return Ok(LaunchPlan {
            mechanism: "open -na",
            argv: super::open_argv(app, directory, url),
        });
    }
    if cfg!(target_os = "linux") {
        let binary = resolve_linux_binary(app)?;
        let mut argv = vec![binary, format!("--profile-directory={directory}")];
        if let Some(url) = url {
            argv.push(url.to_owned());
        }
        return Ok(LaunchPlan {
            mechanism: "chromium --profile-directory",
            argv,
        });
    }
    Err("browser open is not mapped on this OS".into())
}

fn resolve_linux_binary(app: &BrowserApp) -> Result<String, String> {
    for name in discovery::linux_launch_binary_names(app) {
        if let Some(path) = discovery::resolve_linux_executable(name) {
            return Ok(path.display().to_string());
        }
    }
    Err(format!(
        "no Linux launcher found for {}; install google-chrome, chromium, or brave-browser",
        app.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_profiles::APPS;

    #[test]
    fn running_catalog_apps_prefers_exact_inventory_names() {
        let running = vec!["Brave Origin".to_owned(), "Terminal".to_owned()];
        let apps = running_catalog_apps(&running);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Brave Origin");
    }

    #[test]
    fn window_matching_accepts_linux_process_names() {
        let chrome = APPS[2];
        assert!(window_matches_catalog_app("chrome", &chrome));
        assert!(window_matches_catalog_app("google-chrome-stable", &chrome));
        assert!(window_matches_catalog_app("Google Chrome", &chrome));
        assert!(!window_matches_catalog_app("xfce4-terminal", &chrome));
    }

    #[test]
    fn inferred_browser_profile_uses_last_used_on_linux() {
        use super::super::ProfileEntry;

        let chrome = APPS[2];
        let entries = vec![
            ProfileEntry {
                name: "work".into(),
                directory: "Profile 1".into(),
                last_used: false,
            },
            ProfileEntry {
                name: "Default".into(),
                directory: "Default".into(),
                last_used: true,
            },
        ];
        if cfg!(target_os = "linux") {
            assert_eq!(
                inferred_browser_profile("chrome", "Example Domain", &chrome, &entries),
                Some("Default".into())
            );
        } else {
            assert_eq!(
                inferred_browser_profile("chrome", "Example Domain", &chrome, &entries),
                None
            );
        }
    }

    #[test]
    fn inferred_browser_profile_single_entry_on_linux() {
        use super::super::ProfileEntry;

        let chrome = APPS[2];
        let entries = vec![ProfileEntry {
            name: "Default".into(),
            directory: "Default".into(),
            last_used: false,
        }];
        if cfg!(target_os = "linux") {
            assert_eq!(
                inferred_browser_profile("google-chrome-stable", "New Tab", &chrome, &entries),
                Some("Default".into())
            );
        }
    }

    #[test]
    fn linux_launch_plan_includes_profile_directory_and_url() {
        if !cfg!(target_os = "linux") {
            return;
        }
        if discovery::resolve_linux_executable("google-chrome").is_none()
            && discovery::resolve_linux_executable("google-chrome-stable").is_none()
        {
            return;
        }
        let plan = open_launch_plan(&APPS[2], "Default", Some("https://example.com/"))
            .expect("launch plan");
        assert_eq!(plan.mechanism, "chromium --profile-directory");
        assert!(plan.argv[0].contains("chrome"));
        assert_eq!(plan.argv[1], "--profile-directory=Default");
        assert_eq!(plan.argv[2], "https://example.com/");
    }
}
