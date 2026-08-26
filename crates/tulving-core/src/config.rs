//! Optional user configuration at `~/.tulving/config.toml`. Absent by
//! default; every field has a working no-config behavior.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

/// Parsed `config.toml`. Only one knob exists so far, on purpose.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Default notifier command (argv). Runs with the envelope JSON on
    /// stdin whenever a schedule's `on` predicate fires and the schedule
    /// names no notifier of its own.
    #[serde(default)]
    pub notify: Option<Vec<String>>,
}

/// Load the config, or the default when the file does not exist.
pub fn load() -> Result<Config> {
    let path = paths::home().join("config.toml");
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
}
