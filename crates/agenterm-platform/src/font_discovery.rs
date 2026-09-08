//! Linux monospace discovery honesty for CEO read-back smokes.

use std::path::Path;

use crate::font::{candidates, probe};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonospaceDiscovery {
    pub source: &'static str,
    pub primary_family: Option<&'static str>,
    pub primary_path: Option<String>,
    pub fontconfig_family: Option<String>,
    pub fontconfig_file: Option<String>,
    pub alternatives: Vec<&'static str>,
}

fn fontconfig_match_raw() -> Option<(String, String)> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family}\n%{file}\n", "monospace"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let family = lines.next()?.trim();
    let file = lines.next()?.trim();
    if family.is_empty() || file.is_empty() {
        return None;
    }
    Some((family.to_owned(), file.to_owned()))
}

pub fn discover() -> MonospaceDiscovery {
    let fontconfig_match = fontconfig_match_raw();
    let facts = probe();
    let primary_family = facts.primary_family;
    let primary_path = primary_family.and_then(|family| {
        candidates()
            .iter()
            .find(|candidate| candidate.name == family && candidate.exists())
            .map(|candidate| candidate.absolute_path().to_string_lossy().into_owned())
    });
    let alternatives = facts
        .available_families
        .iter()
        .filter(|family| primary_family != Some(*family))
        .copied()
        .collect();
    let source = match (&fontconfig_match, primary_family, &primary_path) {
        (Some((family, file)), Some(primary), Some(path))
            if Path::new(file).is_file() && family == primary && file == path =>
        {
            "fontconfig"
        }
        (_, Some(_), Some(_)) => "hardcoded-fallback",
        _ => "unavailable",
    };
    MonospaceDiscovery {
        source,
        primary_family,
        primary_path,
        fontconfig_family: fontconfig_match.as_ref().map(|(family, _)| family.clone()),
        fontconfig_file: fontconfig_match.as_ref().map(|(_, file)| file.clone()),
        alternatives,
    }
}
