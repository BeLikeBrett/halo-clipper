//! Scrub-friendly proxy generator.
//!
//! Long-GOP sources (OBS captures, game-clip exports) often place
//! keyframes 2–5 seconds apart, which means mpv's keyframe-snap seek
//! during a timeline drag jumps by huge chunks. This module produces a
//! tiny intermediate H.264 file with short GOPs (every 12 frames),
//! cached on disk by source identity, so the preview can scrub
//! frame-accurately without the user waiting for full-decode hr-seeks.
//!
//! Two key properties:
//!
//! - **Cache-keyed by content identity.** The cache key is a hash of
//!   `(absolute_path, size, mtime_ns)`, so re-opening the same file is
//!   instant, and modifying the source (re-recording the same path)
//!   invalidates automatically.
//!
//! - **Export never sees the proxy.** The proxy is purely a preview
//!   optimization — `ExportPlan` is always built from the original
//!   source path. Output quality is identical to a no-proxy build.

use anyhow::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Cache root. `$XDG_CACHE_HOME/halo-clipper/proxies` if set, otherwise
/// `$HOME/.cache/halo-clipper/proxies`. A cache left behind by the app's
/// previous identity (`brett-clipper`) is renamed over on first touch so
/// existing proxies stay valid.
fn cache_root() -> PathBuf {
    #[cfg(windows)]
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("Halo Clipper/cache/proxies");
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            home.join(".cache")
        });
    let root = base.join("halo-clipper").join("proxies");
    if !root.exists() {
        let legacy = base.join("brett-clipper").join("proxies");
        if legacy.exists() {
            let _ = std::fs::create_dir_all(root.parent().unwrap_or(&base));
            match std::fs::rename(&legacy, &root) {
                Ok(()) => log::info!("migrated proxy cache from {}", legacy.display()),
                Err(e) => log::warn!("proxy cache migration failed: {e}"),
            }
        }
    }
    root
}

/// Stable cache key for a source file: hash of (absolute_path, size,
/// mtime_ns), suffixed by the quality profile so switching profiles
/// regenerates rather than serving the wrong GOP structure. Standard
/// keeps the historical un-suffixed name so pre-existing caches stay
/// warm. Returns None if the file isn't accessible (in which case proxy
/// generation can't proceed anyway).
fn cache_key(source: &Path, quality: crate::settings::ProxyQuality) -> Option<String> {
    let abs = std::fs::canonicalize(source).ok()?;
    let meta = std::fs::metadata(&abs).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = DefaultHasher::new();
    abs.hash(&mut h);
    size.hash(&mut h);
    mtime.hash(&mut h);
    let suffix = match quality {
        crate::settings::ProxyQuality::Standard => "",
        crate::settings::ProxyQuality::AllIntra => "-ai",
    };
    Some(format!("{:016x}{suffix}.mp4", h.finish()))
}

/// Final cache path for a given source. None if the source path can't
/// be canonicalized (e.g., file removed under us).
pub fn proxy_path_for(source: &Path, quality: crate::settings::ProxyQuality) -> Option<PathBuf> {
    Some(cache_root().join(cache_key(source, quality)?))
}

/// True iff a finalized proxy file already exists on disk.
pub fn proxy_exists(source: &Path, quality: crate::settings::ProxyQuality) -> bool {
    proxy_path_for(source, quality)
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Total bytes consumed by the proxy cache directory. Returns 0 if the
/// directory doesn't exist or any read fails — this is purely for UI
/// display, so a "I don't know" result reads as "nothing to clear" and
/// is safe.
pub fn cache_size_bytes() -> u64 {
    let Ok(entries) = std::fs::read_dir(cache_root()) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Delete every file in the proxy cache directory. Leaves the
/// directory itself in place (so subsequent writes don't need to
/// recreate it). Reports per-file errors to the log but always
/// continues — a single locked file shouldn't abort the whole wipe.
pub fn clear_cache() -> Result<u64> {
    let dir = cache_root();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut freed: u64 = 0;
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let size = meta.len();
        match std::fs::remove_file(entry.path()) {
            Ok(()) => freed += size,
            Err(e) => log::warn!(
                "proxy cache: could not remove {}: {e}",
                entry.path().display()
            ),
        }
    }
    Ok(freed)
}

#[derive(Clone, Debug)]
pub struct ProxyProgress {
    pub current_ms: i64,
    pub total_ms: i64,
    pub done: bool,
    pub failed: Option<String>,
    pub destination: Option<PathBuf>,
}

impl ProxyProgress {
    pub fn fraction(&self) -> f32 {
        if self.total_ms <= 0 {
            return 0.0;
        }
        (self.current_ms as f32 / self.total_ms as f32).clamp(0.0, 1.0)
    }
}

/// Live handle to an in-flight proxy transcode. Cancelling kills the
/// ffmpeg child and removes the partial `.tmp` file.
pub struct ProxyHandle {
    progress: Arc<Mutex<ProxyProgress>>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    _thread: JoinHandle<()>,
}

impl ProxyHandle {
    pub fn progress(&self) -> ProxyProgress {
        self.progress.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.child.lock() {
            if let Some(c) = slot.as_mut() {
                let _ = c.kill();
            }
        }
    }
}

/// Kick off background transcoding. `total_ms` is the source duration,
/// used for progress percentage. `use_vaapi` enables the AMD/Intel HW
/// encode path — much faster for long clips. `quality` picks the GOP
/// structure (see [`crate::settings::ProxyQuality`]).
pub fn spawn_transcode(
    source: &Path,
    total_ms: i64,
    use_vaapi: bool,
    quality: crate::settings::ProxyQuality,
) -> Result<ProxyHandle> {
    let dest = proxy_path_for(source, quality)
        .ok_or_else(|| anyhow::anyhow!("source path is not canonicalizable"))?;
    std::fs::create_dir_all(dest.parent().unwrap_or_else(|| Path::new(".")))?;
    let dest_tmp = dest.with_extension("mp4.tmp");
    // Remove any leftover .tmp from a previous failed/cancelled run so
    // we don't see ENOTEMPTY on the final rename.
    let _ = std::fs::remove_file(&dest_tmp);

    let progress = Arc::new(Mutex::new(ProxyProgress {
        current_ms: 0,
        total_ms,
        done: false,
        failed: None,
        destination: None,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

    let progress_t = progress.clone();
    let cancel_t = cancel.clone();
    let child_t = child.clone();
    let source = source.to_path_buf();
    let dest_final = dest.clone();

    let thread = thread::spawn(move || {
        let result = run_ffmpeg(
            &source,
            &dest_tmp,
            use_vaapi,
            quality,
            &progress_t,
            &cancel_t,
            &child_t,
        );
        match result {
            Ok(()) => {
                if let Err(e) = std::fs::rename(&dest_tmp, &dest_final) {
                    let _ = std::fs::remove_file(&dest_tmp);
                    progress_t.lock().unwrap().failed = Some(format!("rename failed: {e}"));
                    return;
                }
                let mut p = progress_t.lock().unwrap();
                p.done = true;
                p.current_ms = p.total_ms;
                p.destination = Some(dest_final);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&dest_tmp);
                if cancel_t.load(Ordering::SeqCst) {
                    progress_t.lock().unwrap().failed = Some("cancelled".into());
                } else {
                    progress_t.lock().unwrap().failed = Some(format!("{e:#}"));
                }
            }
        }
    });

    Ok(ProxyHandle {
        progress,
        cancel,
        child,
        _thread: thread,
    })
}

fn run_ffmpeg(
    source: &Path,
    dest_tmp: &Path,
    use_vaapi: bool,
    quality: crate::settings::ProxyQuality,
    progress: &Arc<Mutex<ProxyProgress>>,
    cancel: &Arc<AtomicBool>,
    child_slot: &Arc<Mutex<Option<Child>>>,
) -> Result<()> {
    // GOP length is what the quality profiles differ on. 12 frames is
    // 0.2 s at 60fps — smooth drag-scrubbing for a small file. 1 makes
    // every frame a keyframe: exact scrubs, bigger proxy.
    let gop = match quality {
        crate::settings::ProxyQuality::Standard => "12",
        crate::settings::ProxyQuality::AllIntra => "1",
    };

    let mut cmd = crate::platform::command("ffmpeg");
    cmd.arg("-hide_banner")
        .args(["-loglevel", "error", "-y"])
        .args(["-progress", "pipe:1", "-nostats"]);

    if use_vaapi {
        // HW path. Device init + filter binding so hwupload/scale_vaapi
        // land on the right surface pool. min(1080p) scale keeps proxy
        // files small while staying visually fine for editing.
        cmd.args([
            "-init_hw_device",
            "vaapi=vahw:/dev/dri/renderD128",
            "-filter_hw_device",
            "vahw",
        ])
        .arg("-i")
        .arg(source)
        // Downscale to 1080p MAX (preserve aspect). The preview canvas
        // is almost always smaller than 1080p; encoding 4K source as
        // 4K proxy is wasted bandwidth.
        .args([
            "-vf",
            "scale='min(1920,iw)':'min(1080,ih)':\
             force_original_aspect_ratio=decrease,\
             format=nv12,hwupload",
        ])
        .args(["-c:v", "h264_vaapi"])
        // CQP rate-control. ~24 is visually fine for editing reference.
        .args(["-rc_mode", "CQP", "-qp", "24"])
        .args(["-g", gop]);
    } else {
        // SW path — libx264 veryfast keeps wall-clock reasonable on
        // multi-core CPUs. CRF 22 is visually fine for editing.
        cmd.arg("-i")
            .arg(source)
            .args([
                "-vf",
                "scale='min(1920,iw)':'min(1080,ih)':\
                 force_original_aspect_ratio=decrease",
            ])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "22"])
            .args(["-g", gop, "-keyint_min", gop, "-sc_threshold", "0"]);
    }

    cmd.args(["-c:a", "copy"])
        .args(["-movflags", "+faststart"])
        .args(["-f", "mp4"])
        .arg(dest_tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    log::info!("proxy started (vaapi={use_vaapi})");
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let errors = thread::spawn(move || {
        let mut tail = std::collections::VecDeque::new();
        let mut buffer = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            tail.extend(&buffer[..n]);
            while tail.len() > 65536 {
                tail.pop_front();
            }
        }
        String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned()
    });
    *child_slot.lock().unwrap() = Some(child);
    if cancel.load(Ordering::SeqCst) {
        if let Some(child) = child_slot.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    let reader = BufReader::new(stdout);
    for line in reader.lines().map_while(Result::ok) {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "out_time_us" || key == "out_time_ms" {
            if let Ok(us) = value.parse::<i64>() {
                let mut p = progress.lock().unwrap();
                p.current_ms = (us / 1000).max(0).min(p.total_ms);
            }
        }
    }

    let status = {
        let mut slot = child_slot.lock().unwrap();
        slot.take().and_then(|mut c| c.wait().ok())
    };

    let detail = errors
        .join()
        .unwrap_or_else(|_| "error reader stopped".into());
    if cancel.load(Ordering::SeqCst) {
        anyhow::bail!("cancelled");
    }
    match status {
        Some(s) if s.success() => Ok(()),
        Some(s) => anyhow::bail!("ffmpeg exited with {s}: {}", detail.trim()),
        None => anyhow::bail!("ffmpeg exited abnormally"),
    }
}
