//! The rule list on disk: `$XDG_CONFIG_HOME/renameant/rules.json`.
//!
//! Only the rules are kept, each with its on/off state, in order. The picked
//! paths, the filters and the removed rows are per-session.

use std::io;
use std::path::{Path, PathBuf};

use renameant_core::rule::Rule;
use serde::{Deserialize, Serialize};

use crate::edit::RuleEdit;

const CONFIG_DIR: &str = "renameant";
const CONFIG_FILE: &str = "rules.json";

/// What the file holds: the rules, and one on/off and one default flag per rule
/// in the same order. This is `RuleSet` plus the flags, flattened.
#[derive(Default, Serialize)]
struct StoredRules {
    rules: Vec<Rule>,
    /// A missing flag reads as on, so a hand-written file is not all off.
    enabled: Vec<bool>,
    /// A missing flag reads as custom, so a file from before the default cards
    /// existed gains them on the next start.
    builtin: Vec<bool>,
}

/// What `load` reads. A rule stays opaque until it parses, so one rule this
/// version no longer knows does not cost the whole pipeline.
#[derive(Deserialize)]
struct LoadedRules {
    #[serde(default)]
    rules: Vec<serde_json::Value>,
    #[serde(default)]
    enabled: Vec<bool>,
    #[serde(default)]
    builtin: Vec<bool>,
}

/// `$XDG_CONFIG_HOME/renameant/rules.json`, default
/// `~/.config/renameant/rules.json`.
pub fn default_path() -> io::Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_CONFIG_HOME nor HOME is set, so the rules have no location",
                )
            })?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join(CONFIG_DIR).join(CONFIG_FILE))
}

/// The saved rules, in order, and the tags of the rules this version could not
/// read. A missing file is an empty pipeline.
pub fn load(path: &Path) -> io::Result<(Vec<RuleEdit>, Vec<String>)> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(error) => return Err(error),
    };
    let stored: LoadedRules = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut unsupported = Vec::new();
    let edits = stored
        .rules
        .iter()
        .enumerate()
        .filter_map(|(at, value)| {
            let rule = match serde_json::from_value::<Rule>(value.clone()) {
                Ok(rule) => rule,
                Err(_) => {
                    let name = match value.get("rule").and_then(|tag| tag.as_str()) {
                        Some(tag) => tag.to_owned(),
                        None => format!("#{}", at + 1),
                    };
                    unsupported.push(name);
                    return None;
                }
            };
            Some(RuleEdit::from_rule(
                &rule,
                stored.enabled.get(at).copied().unwrap_or(true),
                stored.builtin.get(at).copied().unwrap_or(false),
            ))
        })
        .collect();
    Ok((edits, unsupported))
}

/// Writes the rules and their on/off state, creating the directory.
pub fn save(path: &Path, edits: &[RuleEdit]) -> io::Result<()> {
    let stored = StoredRules {
        rules: edits.iter().map(RuleEdit::to_rule).collect(),
        enabled: edits.iter().map(|edit| edit.enabled).collect(),
        builtin: edits.iter().map(|edit| edit.builtin).collect(),
    };
    let text = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write beside the target and rename over it: a crash mid-write leaves the
    // previous file intact instead of a truncated one.
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::RuleKind;

    #[test]
    fn an_unknown_rule_is_dropped_and_the_rest_still_load() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("rules.json");
        let saved = r#"{"rules":[{"rule":"translit","target":"stem"},{"rule":"case","style":"upper","target":"stem"}],"enabled":[true,true],"builtin":[true,false]}"#;
        std::fs::write(&path, saved).expect("write");

        let (edits, unsupported) = load(&path).expect("load");

        assert_eq!(unsupported, vec!["translit".to_owned()]);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].kind, RuleKind::Case);
        assert!(edits[0].enabled);
    }
}
