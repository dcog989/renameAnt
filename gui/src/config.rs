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

/// What the file holds: the rules, and one on/off flag per rule in the same
/// order. This is `RuleSet` plus the flags, flattened.
#[derive(Default, Serialize, Deserialize)]
struct StoredRules {
    #[serde(default)]
    rules: Vec<Rule>,
    /// A missing flag reads as on, so a hand-written file is not all off.
    #[serde(default)]
    enabled: Vec<bool>,
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

/// The saved rules, in order. A missing file is an empty pipeline.
pub fn load(path: &Path) -> io::Result<Vec<RuleEdit>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let stored: StoredRules = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(stored
        .rules
        .iter()
        .enumerate()
        .map(|(at, rule)| {
            RuleEdit::from_rule(rule, stored.enabled.get(at).copied().unwrap_or(true))
        })
        .collect())
}

/// Writes the rules and their on/off state, creating the directory.
pub fn save(path: &Path, edits: &[RuleEdit]) -> io::Result<()> {
    let stored = StoredRules {
        rules: edits.iter().map(RuleEdit::to_rule).collect(),
        enabled: edits.iter().map(|edit| edit.enabled).collect(),
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
