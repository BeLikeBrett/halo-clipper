// Background thumbnail worker. The UI requests a frame at a given timestamp;
// the worker shells out to ffmpeg, decodes the JPEG, and sends the pixel
// buffer back. Channels keep the UI thread non-blocking.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

pub struct ThumbRequest {
    pub source: PathBuf,
    pub timestamp_ms: i64,
    pub target_width: u32,
}

pub struct ThumbResult {
    pub timestamp_ms: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct ThumbWorker {
    tx: Sender<ThumbRequest>,
    pub rx: Receiver<ThumbResult>,
}

impl ThumbWorker {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = channel::<ThumbRequest>();
        let (res_tx, res_rx) = channel::<ThumbResult>();

        thread::spawn(move || {
            // Drain pending requests, only processing the most recent one
            // when several stack up — scrubbing fires many requests but we
            // only care about the latest playhead position.
            while let Ok(first) = req_rx.recv() {
                let mut latest = first;
                while let Ok(next) = req_rx.try_recv() {
                    latest = next;
                }

                match extract(&latest) {
                    Ok(result) => {
                        let _ = res_tx.send(result);
                    }
                    Err(e) => log::warn!("thumb extract failed: {e:#}"),
                }
            }
        });

        Self { tx: req_tx, rx: res_rx }
    }

    pub fn request(&self, req: ThumbRequest) {
        let _ = self.tx.send(req);
    }
}

fn extract(req: &ThumbRequest) -> Result<ThumbResult> {
    let timestamp = req.timestamp_ms as f64 / 1000.0;

    // -ss before -i = fast input seek (snaps to nearest keyframe, which is
    // fine for preview thumbs). -frames:v 1 grabs one frame. We pipe JPEG
    // to stdout so there's no tempfile churn.
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "error",
            "-ss", &format!("{timestamp:.3}"),
            "-i",
        ])
        .arg(&req.source)
        .args([
            "-frames:v", "1",
            "-vf", &format!("scale={}:-2", req.target_width),
            "-f", "image2pipe",
            "-vcodec", "mjpeg",
            "-q:v", "5",
            "pipe:1",
        ])
        .output()
        .context("failed to spawn ffmpeg for thumbnail")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffmpeg thumb failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let img = image::load_from_memory(&output.stdout)
        .context("could not decode ffmpeg JPEG output")?
        .to_rgba8();

    Ok(ThumbResult {
        timestamp_ms: req.timestamp_ms,
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

#[allow(dead_code)]
pub fn _unused(_: &Path) {}
