//! Bounded local logs and explicit, reviewable support exports. No telemetry.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
const MAX_BYTES: u64 = 2 * 1024 * 1024;
static PRIVATE_PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
pub fn register_media(path: &Path) {
    let mut paths = PRIVATE_PATHS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap();
    paths.push(path.to_string_lossy().into_owned());
    if let Some(name) = path.file_name() {
        paths.push(name.to_string_lossy().into_owned());
    }
}
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(paths) = PRIVATE_PATHS.get() {
        if let Ok(paths) = paths.lock() {
            for p in paths.iter().filter(|p| !p.is_empty()) {
                out = out.replace(p, "[media]");
            }
        }
    }
    for var in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        if let Some(p) = std::env::var_os(var) {
            let p = p.to_string_lossy();
            if !p.is_empty() {
                out = out.replace(p.as_ref(), "[user-folder]");
            }
        }
    }
    out
}
pub fn log_dir() -> PathBuf {
    crate::platform::data_dir().join("logs")
}
struct Logger {
    file: Mutex<Option<std::fs::File>>,
}
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record<'_>) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let message = redact(&format!(
            "{} {} {}: {}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            r.level(),
            r.target(),
            r.args()
        ));
        if let Ok(mut slot) = self.file.lock() {
            if slot
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .is_some_and(|m| m.len() >= MAX_BYTES)
            {
                slot.take();
                let root = log_dir();
                let _ = std::fs::remove_file(root.join("halo-clipper.previous.log"));
                let _ = std::fs::rename(
                    root.join("halo-clipper.log"),
                    root.join("halo-clipper.previous.log"),
                );
                *slot = std::fs::File::create(root.join("halo-clipper.log")).ok();
            }
            if let Some(file) = slot.as_mut() {
                let _ = file.write_all(message.as_bytes());
                let _ = file.flush();
            }
        }
        eprint!("{message}");
    }
    fn flush(&self) {
        if let Ok(mut slot) = self.file.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = f.flush();
            }
        }
    }
}
pub fn init() {
    let root = log_dir();
    let _ = std::fs::create_dir_all(&root);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("halo-clipper.log"))
        .ok();
    if let Err(e) = log::set_boxed_logger(Box::new(Logger {
        file: Mutex::new(file),
    })) {
        eprintln!("Could not initialize local log: {e}");
    }
    log::set_max_level(log::LevelFilter::Info);
    std::panic::set_hook(Box::new(|info| log::error!("Application panic: {info}")));
    log::info!(
        "Halo Clipper {} startup; OS={} ARCH={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}
pub fn export(path: &Path, context: &str) -> anyhow::Result<()> {
    log::logger().flush();
    let mut report=format!("Halo Clipper support report\nVersion: {}\nOS: {}\nArchitecture: {}\n{}\n\nReview before sharing. Media files are never included.\n\n",env!("CARGO_PKG_VERSION"),std::env::consts::OS,std::env::consts::ARCH,context);
    for tool in ["ffmpeg", "ffprobe"] {
        if let Ok(output) = crate::platform::command(tool).arg("-version").output() {
            report.push_str(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("Unavailable"),
            );
            report.push('\n');
        }
    }
    for name in ["halo-clipper.previous.log", "halo-clipper.log"] {
        if let Ok(file) = std::fs::File::open(log_dir().join(name)) {
            let mut text = String::new();
            let _ = file.take(MAX_BYTES).read_to_string(&mut text);
            report.push_str(&format!("\n--- {name} ---\n{}", redact(&text)));
        }
    }
    std::fs::write(path, redact(&report))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn media_paths_and_names_are_redacted() {
        super::register_media(std::path::Path::new("/private/videos/game-secret.mp4"));
        let redacted = super::redact("failed /private/videos/game-secret.mp4: game-secret.mp4");
        assert!(!redacted.contains("game-secret"));
        assert!(redacted.contains("[media]"));
    }
}
