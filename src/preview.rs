//! Debounced background composition preview. Never blocks UI or exports originals.
use crate::{
    composition::Composition,
    encode::{EncodeQuality, EncodeSettings, VideoCodec},
    export::{self, ExportHandle, ExportPhase, ExportPlan},
};
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    time::{Duration, Instant},
};
pub struct Preview {
    key: u64,
    ready_key: Option<u64>,
    changed: Instant,
    job: Option<(ExportHandle, PathBuf)>,
    retired: Vec<(ExportHandle, PathBuf)>,
    files: Vec<PathBuf>,
    pub ready: bool,
    pub message: String,
    failed: bool,
}
impl Default for Preview {
    fn default() -> Self {
        Self {
            key: 0,
            ready_key: None,
            changed: Instant::now(),
            job: None,
            retired: Vec::new(),
            files: Vec::new(),
            ready: false,
            message: String::new(),
            failed: false,
        }
    }
}
impl Preview {
    pub fn reset(&mut self) {
        if let Some((h, p)) = self.job.take() {
            h.cancel();
            self.retired.push((h, p));
        }
        self.key = 0;
        self.ready_key = None;
        self.ready = false;
        self.failed = false;
    }
    pub fn retry(&mut self) {
        self.failed = false;
        self.changed = Instant::now() - Duration::from_secs(1);
    }
    pub fn tick(&mut self, composition: &Composition) -> Option<PathBuf> {
        // Keep only the last two completed versions. Windows may hold the
        // immediately previous file briefly while mpv finishes a load.
        while self.files.len() > 2 {
            if std::fs::remove_file(&self.files[0]).is_ok() {
                self.files.remove(0);
            } else {
                break;
            }
        }
        if composition.duration_ms <= 0 {
            self.reset();
            self.message = "Timeline empty. Add media or undo.".into();
            return None;
        }
        self.retired.retain(|(h, p)| {
            if matches!(h.progress().phase, ExportPhase::Running) {
                true
            } else {
                let _ = std::fs::remove_file(p);
                false
            }
        });
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{composition:?}").hash(&mut hasher);
        let key = hasher.finish();
        if self.key != key {
            if let Some((h, p)) = self.job.take() {
                h.cancel();
                self.retired.push((h, p));
            }
            self.key = key;
            self.changed = Instant::now();
            self.ready = false;
            self.failed = false;
            self.message = "Updating layered preview…".into();
        }
        if self.ready_key == Some(key) {
            self.ready = true;
            return None;
        }
        if let Some((h, _)) = &self.job {
            let progress = h.progress();
            match progress.phase {
                ExportPhase::Done => {
                    let (_, path) = self.job.take().unwrap();
                    self.ready_key = Some(key);
                    self.ready = true;
                    self.message.clear();
                    self.files.push(path.clone());
                    return Some(path);
                }
                ExportPhase::Failed(ref err) => {
                    self.message = format!("Preview failed: {err}");
                    log::error!("composition preview: {err}");
                    let (_, path) = self.job.take().unwrap();
                    let _ = std::fs::remove_file(path);
                    self.failed = true;
                }
                ExportPhase::Cancelled => {
                    self.job.take();
                    self.failed = true;
                    self.message = "Preview cancelled. Retry to rebuild.".into();
                }
                _ => {
                    self.message = format!(
                        "Preparing layered preview… {:.0}%",
                        progress.fraction() * 100.0
                    );
                }
            }
        } else if !self.failed
            && self.changed.elapsed() >= Duration::from_millis(450)
            && composition.duration_ms > 0
        {
            let path = std::env::temp_dir().join(format!(
                "halo-preview-{}-{}.mp4",
                std::process::id(),
                key
            ));
            let mut settings =
                EncodeSettings::default_for_source(composition.width, composition.height);
            let ratio = (640.0 / composition.width.max(1) as f64)
                .min(360.0 / composition.height.max(1) as f64)
                .min(1.0);
            settings.resolution = Some((
                ((composition.width as f64 * ratio) as u32 / 2 * 2).max(2),
                ((composition.height as f64 * ratio) as u32 / 2 * 2).max(2),
            ));
            settings.video_codec = VideoCodec::X264;
            settings.video_preset = "ultrafast".into();
            settings.quality = EncodeQuality::ConstantQuality(25);
            settings.fps_limit = Some(30);
            match export::spawn(ExportPlan::Layered {
                composition: composition.clone(),
                destination: path.clone(),
                encode: settings,
            }) {
                Ok(h) => self.job = Some((h, path)),
                Err(e) => {
                    self.failed = true;
                    self.message = format!("Preview failed: {e:#}");
                }
            }
        }
        None
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.reset();
        for (h, p) in &self.retired {
            h.cancel();
            let _ = std::fs::remove_file(p);
        }
        for p in &self.files {
            let _ = std::fs::remove_file(p);
        }
    }
}
