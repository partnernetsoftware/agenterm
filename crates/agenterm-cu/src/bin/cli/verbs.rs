//! Hot command routing plus the compressed, immutable cold help catalog.
//!
//! `verbs-catalog.json` is the single declaration source. `build.rs` validates
//! it, generates this module's small hot table, and compresses the cold help
//! projections. Ordinary dispatch never inflates or parses the cold catalog.

use std::sync::OnceLock;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    System,
    Windows,
    Process,
    Network,
    File,
    Terminal,
    A11yObserve,
    A11yActuate,
    Browser,
    Clipboard,
    Placement,
    Transports,
    Host,
}

pub struct VerbSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub family: Family,
}

impl VerbSpec {
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }
}

include!(concat!(env!("OUT_DIR"), "/agenterm_cu_verbs_hot.rs"));

static COLD: OnceLock<Value> = OnceLock::new();

fn cold_catalog() -> &'static Value {
    COLD.get_or_init(|| {
        let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/agenterm_cu_verbs_catalog.z"
        )))
        .expect("embedded verb catalog must be valid zlib");
        assert_eq!(bytes.len(), VERB_CATALOG_BYTES);
        serde_json::from_slice(&bytes).expect("embedded verb catalog must be valid JSON")
    })
}

pub fn cold_text(field: &str) -> &'static str {
    cold_catalog()[field]
        .as_str()
        .unwrap_or_else(|| panic!("embedded verb catalog field {field:?} must be text"))
}

pub fn cold_verbs() -> &'static [Value] {
    cold_catalog()["verbs"]
        .as_array()
        .expect("embedded verb catalog verbs must be an array")
}

pub fn cold_verb(name: &str) -> &'static Value {
    cold_verbs()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("embedded verb catalog has no row for {name:?}"))
}

pub fn cold_help(name: &str) -> &'static str {
    cold_catalog()["help"]
        .as_array()
        .expect("embedded verb catalog help must be an array")
        .iter()
        .find(|row| row["name"] == name)
        .and_then(|row| row["text"].as_str())
        .unwrap_or_else(|| panic!("embedded verb catalog has no help for {name:?}"))
}

pub fn cold_verbs_json() -> String {
    serde_json::to_string_pretty(cold_verbs()).expect("validated verb rows serialize")
}

pub fn lookup(token: &str) -> Option<&'static VerbSpec> {
    VERBS
        .iter()
        .find(|spec| spec.spellings().any(|spelling| spelling == token))
        .or_else(|| {
            VERBS.iter().find(|spec| {
                spec.aliases
                    .iter()
                    .any(|alias| alias.split_once(' ').is_some_and(|(head, _)| head == token))
            })
        })
}

pub fn resolve(first: &str, second: Option<&str>) -> Option<&'static VerbSpec> {
    if let Some(second) = second {
        let joined = format!("{first} {second}");
        if let Some(spec) = VERBS
            .iter()
            .find(|spec| spec.aliases.contains(&joined.as_str()))
        {
            return Some(spec);
        }
    }
    lookup(first)
}

pub fn near_matches(token: &str) -> Vec<&'static str> {
    let token = token.to_ascii_lowercase();
    let mut scored: Vec<(u8, &'static str)> = VERBS
        .iter()
        .flat_map(VerbSpec::spellings)
        .filter_map(|spelling| {
            let score = if spelling.starts_with(&token) || token.starts_with(spelling) {
                0
            } else if spelling.contains(&token) || token.contains(spelling) {
                1
            } else if levenshtein(spelling, &token) <= 2 {
                2
            } else {
                return None;
            };
            Some((score, spelling))
        })
        .collect();
    scored.sort();
    scored.dedup();
    scored.into_iter().take(6).map(|(_, s)| s).collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
pub fn by_family(family: Family) -> impl Iterator<Item = &'static VerbSpec> {
    VERBS.iter().filter(move |spec| spec.family == family)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn hot_table_matches_the_validated_cold_catalog() {
        assert_eq!(VERBS.len(), cold_verbs().len());
        let mut seen = BTreeSet::new();
        for spec in VERBS {
            let row = cold_verb(spec.name);
            let aliases: Vec<&str> = row["aliases"]
                .as_array()
                .expect("aliases")
                .iter()
                .map(|value| value.as_str().expect("alias text"))
                .collect();
            assert_eq!(aliases, spec.aliases);
            for spelling in spec.spellings() {
                assert!(seen.insert(spelling), "duplicate spelling {spelling:?}");
                assert!(
                    spelling.chars().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '-'
                        || c == ' '),
                    "{spelling:?} is not lower kebab"
                );
            }
            assert!(cold_help(spec.name).starts_with(&format!("agenterm-cu {}", spec.name)));
        }
    }

    #[test]
    fn aliases_and_group_words_resolve() {
        for spec in VERBS {
            for alias in spec.aliases {
                let resolved = match alias.split_once(' ') {
                    Some((head, tail)) => resolve(head, Some(tail)),
                    None => resolve(alias, None),
                };
                assert_eq!(resolved.map(|row| row.name), Some(spec.name));
            }
        }
        assert_eq!(lookup("menu").map(|s| s.name), Some("menu-inspect"));
        assert_eq!(
            resolve("tab", Some("select")).map(|s| s.name),
            Some("tab-select")
        );
        assert_eq!(resolve("page", Some("zoom")).map(|s| s.name), Some("page"));
        assert!(lookup("no-such-verb").is_none());
    }

    #[test]
    fn every_family_is_populated_and_near_matches_stay_bounded() {
        for family in [
            Family::System,
            Family::Windows,
            Family::Process,
            Family::Network,
            Family::Terminal,
            Family::A11yObserve,
            Family::A11yActuate,
            Family::Browser,
            Family::Clipboard,
            Family::Placement,
            Family::Transports,
            Family::Host,
        ] {
            assert!(by_family(family).next().is_some(), "{family:?} is empty");
        }
        assert!(near_matches("menu").contains(&"menu inspect"));
        assert!(near_matches("windws").contains(&"windows"));
        assert!(near_matches("zzzzzzzz").is_empty());
        assert!(near_matches("c").len() <= 6);
    }

    #[test]
    fn scopes_follow_the_command_contract() {
        let actuate: BTreeSet<&str> = VERBS
            .iter()
            .filter(|spec| cold_verb(spec.name)["grant"] == "actuate")
            .map(|spec| cold_verb(spec.name)["command"].as_str().expect("command"))
            .collect();
        for expected in [
            "pointer-move",
            "process-kill",
            "pty-prune",
            "terminal-send",
            "invoke",
            "click",
            "focus",
            "send-text",
            "paste",
            "window-place",
            "close",
            "page-click",
            "app",
        ] {
            assert!(actuate.contains(expected), "{expected} must be actuate");
        }
        for expected in ["hit", "zoom", "snapshot", "diff"] {
            assert!(!actuate.contains(expected), "{expected} must be observe");
        }
        assert_eq!(actuate.len(), 59, "{actuate:?}");
    }
}
