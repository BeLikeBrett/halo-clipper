// ffprobe wrapper. Reads container/stream metadata into a MediaInfo struct.
// We spawn ffprobe as a subprocess and parse its JSON output — no native
// ffmpeg linking, so updating system ffmpeg never requires a rebuild.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// ffprobe `codec_name` — used by the export pipeline to decide
    /// whether to take the VAAPI HW-decode fast path for this source.
    pub video_codec: String,
    #[allow(dead_code)]
    pub audio_codec: Option<String>,
    /// Number of audio streams in the source. Stream specifier `0:a:N`
    /// where N ∈ [0, audio_stream_count).
    pub audio_stream_count: usize,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .context("failed to spawn ffprobe — is it installed?")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .context("ffprobe returned unparseable JSON")?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .ok_or_else(|| anyhow!("no video stream in file"))?;

    let audio = parsed.streams.iter().find(|s| s.codec_type == "audio");
    let audio_stream_count = parsed
        .streams
        .iter()
        .filter(|s| s.codec_type == "audio")
        .count();

    let duration_secs: f64 = parsed
        .format
        .duration
        .as_deref()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("could not determine duration"))?;

    let fps = video
        .avg_frame_rate
        .as_deref()
        .or(video.r_frame_rate.as_deref())
        .and_then(parse_fraction)
        .unwrap_or(30.0);

    Ok(MediaInfo {
        duration_ms: (duration_secs * 1000.0) as i64,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        fps,
        video_codec: video.codec_name.clone().unwrap_or_default(),
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
        audio_stream_count,
    })
}

// ffmpeg reports framerate as a fraction string like "30000/1001".
fn parse_fraction(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let n: f64 = num.parse().ok()?;
    let d: f64 = den.parse().ok()?;
    if d == 0.0 { None } else { Some(n / d) }
}

/// Find the latest keyframe at-or-before `target_ms` in the source's
/// video stream. Used by smart-cut to decide whether a stream-copy export
/// will land cleanly on the In mark, or whether we need to re-encode.
///
/// Implementation: ffprobe with `-skip_frame nokey` lists only keyframes;
/// `-read_intervals START%+DURATION` constrains the scan to a small window
/// before the target so this stays cheap even for long videos.
pub fn nearest_keyframe_before(source: &Path, target_ms: i64) -> Result<i64> {
    let target_s = (target_ms as f64 / 1000.0).max(0.0);
    // 5-second look-back covers typical 1–2s GOPs plus the long end of
    // most encoder defaults. Files with huge keyframe intervals (10s+)
    // would benefit from a larger window, but those are rare for editing
    // sources.
    let window_start = (target_s - 5.0).max(0.0);
    let window_dur = (target_s - window_start) + 0.2;

    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-skip_frame", "nokey"])
        .args([
            "-read_intervals",
            &format!("{:.3}%+{:.3}", window_start, window_dur),
        ])
        .args(["-show_entries", "frame=pts_time"])
        .args(["-of", "csv=p=0"])
        .arg(source)
        .output()
        .context("failed to spawn ffprobe for keyframe probe")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffprobe keyframe probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut best: Option<f64> = None;
    for line in text.lines() {
        let s = line.trim().trim_end_matches(',');
        if s.is_empty() { continue; }
        if let Ok(t) = s.parse::<f64>() {
            // 1 ms slack on the upper bound — pts_time can be exactly at
            // target_s and we still want to count it as "at-or-before".
            if t <= target_s + 0.001 {
                best = Some(best.map(|b| b.max(t)).unwrap_or(t));
            }
        }
    }
    let kt = best.ok_or_else(|| anyhow!("no keyframe found at or before {target_ms}ms"))?;
    Ok((kt * 1000.0) as i64)
}

pub fn format_timestamp(ms: i64) -> String {
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    let frac = ms % 1000;
    if h > 0 {
        format!("{:02}:{:02}:{:02}.{:03}", h, m, s, frac)
    } else {
        format!("{:02}:{:02}.{:03}", m, s, frac)
    }
}
