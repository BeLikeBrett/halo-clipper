//! Persistent user preferences.
//!
//! Loaded from `$XDG_CONFIG_HOME/brett-clipper/config.json` (falls back
//! to `$HOME/.config/...`). Tolerates a missing file (returns defaults)
//! and a corrupt file (logs + returns defaults) — preferences should
//! never be a reason the app fails to start.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    /// When true, the proxy cache (~/.cache/brett-clipper/proxies/) is
    /// wiped on app exit. Default: off — leaving the cache populated
    /// makes second-opens of the same file instant. Users with tight
    /// disk space who rarely re-open the same source can flip it on.
    #[serde(default)]
    pub auto_clear_cache_on_close: bool,
}

/// Resolve the config file path. Honors `$XDG_CONFIG_HOME`, falls back
/// to `$HOME/.config`, falls back to `/tmp` so we never panic.
fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".config")
        });
    base.join("brett-clipper").join("config.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(e) => {
                log::warn!("settings read failed ({}): {e}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_slice::<Settings>(&bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("settings parse failed ({}): {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        // Atomic write: tmp + rename so a half-flushed config never
        // overwrites a good one.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
