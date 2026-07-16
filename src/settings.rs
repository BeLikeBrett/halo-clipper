//! Persistent user preferences.
//!
//! Loaded from `$XDG_CONFIG_HOME/halo-clipper/config.json` (falls back
//! to `$HOME/.config/...`). Tolerates a missing file (returns defaults)
//! and a corrupt file (logs + returns defaults) — preferences should
//! never be a reason the app fails to start.
//!
//! A config left behind by the app's previous identity
//! (`brett-clipper/config.json`) is migrated in place on first load.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Scrub-proxy encode profile. See `proxy.rs` for what each maps to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyQuality {
    /// GOP-12 H.264 at ≤1080p — small files, scrubs land within ~0.2 s
    /// of the pointer. The right default for almost everyone.
    #[default]
    Standard,
    /// Every frame is a keyframe — scrubs land exactly where you point,
    /// at the cost of noticeably bigger proxy files and a slower
    /// generation pass.
    AllIntra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    /// When true, the proxy cache (~/.cache/halo-clipper/proxies/) is
    /// wiped on app exit. Default: off — leaving the cache populated
    /// makes second-opens of the same file instant. Users with tight
    /// disk space who rarely re-open the same source can flip it on.
    #[serde(default)]
    pub auto_clear_cache_on_close: bool,
    /// Scrub-proxy quality profile. Standard (GOP 12) unless the user
    /// opts into all-intra frame-exact scrubbing.
    #[serde(default)]
    pub proxy_quality: ProxyQuality,
}

/// Resolve a config file path under the given app dir name. Honors
/// `$XDG_CONFIG_HOME`, falls back to `$HOME/.config`, falls back to
/// `/tmp` so we never panic.
fn config_path_for(app_dir: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".config")
        });
    base.join(app_dir).join("config.json")
}

fn config_path() -> PathBuf {
    config_path_for("halo-clipper")
}

/// One-time migration from the pre-rename config location. Rename is
/// atomic on the same filesystem; on failure we just fall back to
/// defaults, which is the same outcome as no migration at all.
fn migrate_legacy_config(new_path: &PathBuf) {
    let legacy = config_path_for("brett-clipper");
    if new_path.exists() || !legacy.exists() {
        return;
    }
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(&legacy, new_path) {
        Ok(()) => log::info!("migrated settings from {}", legacy.display()),
        Err(e) => log::warn!("settings migration failed: {e}"),
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        migrate_legacy_config(&path);
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
