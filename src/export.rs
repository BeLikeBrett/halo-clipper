//! Async export pipeline.
//!
//! Two distinct paths:
//!
//! 1. [`ExportPlan::Simple`] — one source range. Either stream-copies
//!    (instant) or re-encodes (frame-accurate) via a single ffmpeg
//!    invocation. Used by Simple Clip mode.
//!
//! 2. [`ExportPlan::MultiTrack`] — separate video & audio clip lists,
//!    each placed at a `cut_start_ms` on the output timeline. Audio
//!    gaps are padded with silence. Built into one ffmpeg invocation via
//!    `-filter_complex` (trim+concat). Always re-encodes — filter graphs
//!    operate on decoded frames, so stream-copy isn't possible here.
//!
//! Both paths share the same progress reporting / cancellation contract.

use anyhow::Result;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::encode::{is_vaapi_decodable, EncodeSettings, HwDecode};

/// Encode mode for a Simple export. MultiTrack always re-encodes through
/// a filter graph and uses `EncodeSettings` directly.
#[derive(Clone, Debug)]
pub enum EncodeMode {
    /// Stream copy. Only valid when the cut is keyframe-aligned and no
    /// modifications need re-encoding.
    Copy,
    /// Re-encode with the given settings.
    Encode(EncodeSettings),
}

/// Half-open range on the output timeline, sourced from a half-open range
/// of the input. Used by [`ExportPlan::MultiTrack`].
#[derive(Clone, Copy, Debug)]
pub struct ClipRange {
    pub cut_start_ms: i64,
    pub src_start_ms: i64,
    pub src_end_ms: i64,
    /// 1.0 = unity. Only meaningful for audio chains — appended as a
    /// `volume=GAIN` filter after the atrim. Video tracks ignore this.
    pub gain: f32,
    /// Playback speed for this clip. 1.0 = normal. Video chains get
    /// `setpts=(PTS-STARTPTS)/SPEED`; audio chains get an `atempo` chain
    /// (chained when speed is outside 0.5–2.0 because atempo clamps).
    pub speed: f32,
}

impl ClipRange {
    fn src_duration_ms(&self) -> i64 {
        (self.src_end_ms - self.src_start_ms).max(0)
    }
    fn cut_end_ms(&self) -> i64 {
        self.cut_start_ms + self.src_duration_ms()
    }
}

#[derive(Clone)]
pub struct AudioTrackPlan {
    /// Source audio stream specifier — `0:a:N`. N is `source_audio_index`.
    pub source_audio_index: usize,
    pub muted: bool,
    /// Track-level gain (1.0 = unity). Applied after the concat so it
    /// works uniformly across all clips on the track.
    pub gain: f32,
    /// Ordered by cut_start_ms, non-overlapping. Gaps relative to the
    /// video total become silence in this track's output chain.
    pub clips: Vec<ClipRange>,
}

#[derive(Clone)]
pub enum ExportPlan {
    Layered {
        composition: crate::composition::Composition,
        destination: PathBuf,
        encode: EncodeSettings,
    },
    Simple {
        source: PathBuf,
        destination: PathBuf,
        /// ffprobe `codec_name` of the source's video stream. Used to
        /// gate the HW-decode fast path — see `is_vaapi_decodable`.
        source_codec: String,
        src_start_ms: i64,
        src_end_ms: i64,
        encode: EncodeMode,
        /// When true, the output has no audio stream (ffmpeg `-an`).
        audio_muted: bool,
    },
    #[cfg_attr(not(test), allow(dead_code))]
    MultiTrack {
        source: PathBuf,
        destination: PathBuf,
        /// ffprobe `codec_name` of the source's video stream. Used to
        /// gate the HW-decode fast path — see `is_vaapi_decodable`.
        source_codec: String,
        /// Source frame dimensions. Needed to synthesize black gap
        /// segments at the right size and to scale caption font sizes
        /// when the output resolution differs from the source.
        src_width: u32,
        src_height: u32,
        /// Source frame rate — black gap segments are generated at this
        /// rate so the concat doesn't mix cadences.
        src_fps: f64,
        /// Ordered by cut_start_ms, non-overlapping.
        video_clips: Vec<ClipRange>,
        /// Optional caption for each video segment, in video_clips order.
        /// Each caption is visible only during its segment (never gaps).
        video_text_overlays: Vec<Option<crate::track::TextOverlay>>,
        /// One entry per audio track. Non-muted tracks are mixed together
        /// in the output via `amix`. If all are muted/empty, the output
        /// has no audio stream.
        audio_tracks: Vec<AudioTrackPlan>,
        /// MultiTrack always re-encodes (filter graph requires decode).
        encode: EncodeSettings,
    },
}

impl ExportPlan {
    pub fn destination(&self) -> &Path {
        match self {
            Self::Simple { destination, .. }
            | Self::MultiTrack { destination, .. }
            | Self::Layered { destination, .. } => destination,
        }
    }

    pub fn total_output_ms(&self) -> i64 {
        match self {
            Self::Layered { composition, .. } => composition.duration_ms,
            Self::Simple {
                src_start_ms,
                src_end_ms,
                ..
            } => (src_end_ms - src_start_ms).max(0),
            Self::MultiTrack {
                video_clips,
                audio_tracks,
                ..
            } => {
                let end = timeline_end_ms(video_clips, audio_tracks);
                output_duration_ms(video_clips, end)
            }
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self {
            Self::Layered { .. } => "layered composition",
            Self::Simple {
                encode: EncodeMode::Encode(_),
                ..
            } => "re-encode",
            Self::Simple {
                encode: EncodeMode::Copy,
                ..
            } => "stream copy",
            Self::MultiTrack { .. } => "multi-track (filter graph)",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ExportPhase {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone)]
pub struct ExportProgress {
    pub phase: ExportPhase,
    pub current_ms: i64,
    pub total_ms: i64,
    pub fps: f64,
    pub speed: f64,
    pub started: Instant,
    pub finished_at: Option<Instant>,
    pub destination: PathBuf,
    pub mode_label: &'static str,
}

impl ExportProgress {
    pub fn fraction(&self) -> f32 {
        if self.total_ms <= 0 {
            return 0.0;
        }
        (self.current_ms as f32 / self.total_ms as f32).clamp(0.0, 1.0)
    }

    pub fn elapsed(&self) -> Duration {
        match self.finished_at {
            Some(end) => end.saturating_duration_since(self.started),
            None => self.started.elapsed(),
        }
    }

    pub fn eta(&self) -> Option<Duration> {
        let frac = self.fraction();
        if frac <= 0.001 {
            return None;
        }
        let elapsed = self.elapsed().as_secs_f64();
        let total_est = elapsed / frac as f64;
        let remaining = (total_est - elapsed).max(0.0);
        Some(Duration::from_secs_f64(remaining))
    }
}

pub struct ExportHandle {
    progress: Arc<Mutex<ExportProgress>>,
    cancel_flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    _thread: JoinHandle<()>,
}

impl ExportHandle {
    pub fn progress(&self) -> ExportProgress {
        self.progress.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.child.lock() {
            if let Some(child) = slot.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

pub fn spawn(plan: ExportPlan) -> Result<ExportHandle> {
    let total_ms = plan.total_output_ms();
    log::info!("{} job started; timeline={}ms", plan.mode_label(), total_ms);
    if total_ms <= 0 {
        anyhow::bail!("Nothing to export — output would be empty.");
    }

    let progress = Arc::new(Mutex::new(ExportProgress {
        phase: ExportPhase::Running,
        current_ms: 0,
        total_ms,
        fps: 0.0,
        speed: 0.0,
        started: Instant::now(),
        finished_at: None,
        destination: plan.destination().to_path_buf(),
        mode_label: plan.mode_label(),
    }));
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let child_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

    let p = progress.clone();
    let c = cancel_flag.clone();
    let cs = child_slot.clone();

    let thread = thread::spawn(move || run_plan(plan, p, c, cs));

    Ok(ExportHandle {
        progress,
        cancel_flag,
        child: child_slot,
        _thread: thread,
    })
}

fn run_plan(
    plan: ExportPlan,
    progress: Arc<Mutex<ExportProgress>>,
    cancel_flag: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) {
    let res = match &plan {
        ExportPlan::Layered {
            composition,
            destination,
            encode,
        } => composition
            .command(destination, encode)
            .and_then(|cmd| run_ffmpeg_with_progress(cmd, &progress, &cancel_flag, &child_slot)),
        ExportPlan::Simple {
            source,
            destination,
            source_codec,
            src_start_ms,
            src_end_ms,
            encode,
            audio_muted,
        } => run_simple(
            source,
            destination,
            source_codec,
            *src_start_ms,
            *src_end_ms,
            encode,
            *audio_muted,
            &progress,
            &cancel_flag,
            &child_slot,
        ),
        ExportPlan::MultiTrack {
            source,
            destination,
            source_codec,
            src_width,
            src_height,
            src_fps,
            video_clips,
            video_text_overlays,
            audio_tracks,
            encode,
        } => run_multitrack(
            source,
            destination,
            source_codec,
            (*src_width, *src_height),
            *src_fps,
            video_clips,
            video_text_overlays,
            audio_tracks,
            encode,
            &progress,
            &cancel_flag,
            &child_slot,
        ),
    };
    finalize(&progress, &cancel_flag, res);
}

fn finalize(progress: &Arc<Mutex<ExportProgress>>, cancel_flag: &Arc<AtomicBool>, res: Result<()>) {
    let mut p = progress.lock().unwrap();
    p.finished_at = Some(Instant::now());
    if cancel_flag.load(Ordering::SeqCst) {
        log::info!("{} job cancelled", p.mode_label);
        p.phase = ExportPhase::Cancelled;
    } else {
        match res {
            Ok(()) => {
                log::info!(
                    "{} job completed in {:.2}s",
                    p.mode_label,
                    p.started.elapsed().as_secs_f64()
                );
                p.phase = ExportPhase::Done;
                p.current_ms = p.total_ms;
            }
            Err(e) => {
                log::error!("{} job failed: {e:#}", p.mode_label);
                p.phase = ExportPhase::Failed(format!("{e:#}"));
            }
        }
    }
}

// ------------------------------------------------------------------
// Simple path: single source range, single ffmpeg run.
// ------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_simple(
    source: &Path,
    destination: &Path,
    source_codec: &str,
    src_start_ms: i64,
    src_end_ms: i64,
    encode: &EncodeMode,
    audio_muted: bool,
    progress: &Arc<Mutex<ExportProgress>>,
    cancel_flag: &Arc<AtomicBool>,
    child_slot: &Arc<Mutex<Option<Child>>>,
) -> Result<()> {
    let start = src_start_ms as f64 / 1000.0;
    let duration = (src_end_ms - src_start_ms) as f64 / 1000.0;

    // HW decode is opt-in per-encoder AND requires a source codec that
    // VAAPI can decode reliably; otherwise we fall back to SW decode so
    // exotic sources (AV1 on older silicon, VP8, etc.) can't break the
    // run.
    let hw_decode = match encode {
        EncodeMode::Encode(s) if is_vaapi_decodable(source_codec) => s.video_codec.hw_decode(),
        _ => HwDecode::None,
    };

    let mut cmd = crate::platform::command("ffmpeg");
    cmd.arg("-hide_banner")
        .args(["-loglevel", "error", "-y"])
        .args(["-progress", "pipe:1", "-nostats"]);

    // VAAPI: bind the device to both the encoder and the filter graph
    // (see comment in run_multitrack for why -vaapi_device alone isn't
    // enough).
    let needs_vaapi_device = matches!(hw_decode, HwDecode::Vaapi)
        || matches!(encode, EncodeMode::Encode(s) if s.video_codec.needs_vaapi_upload());
    if needs_vaapi_device {
        cmd.args([
            "-init_hw_device",
            &format!("vaapi=vahw:{}", default_vaapi_device()),
            "-filter_hw_device",
            "vahw",
        ]);
    }

    // HW decode flags must come before `-i`. They tell ffmpeg's input
    // demuxer to route decoded frames into VAAPI surfaces tied to vahw,
    // so subsequent filters and the encoder all see GPU frames.
    if matches!(hw_decode, HwDecode::Vaapi) {
        cmd.args([
            "-hwaccel",
            "vaapi",
            "-hwaccel_output_format",
            "vaapi",
            "-hwaccel_device",
            "vahw",
        ]);
    }

    cmd.args(["-ss", &format!("{start:.3}")])
        .arg("-i")
        .arg(source)
        .args(["-t", &format!("{duration:.3}")]);

    match encode {
        EncodeMode::Copy => {
            cmd.args(["-c", "copy"])
                .args(["-avoid_negative_ts", "make_zero"]);
            if audio_muted {
                cmd.arg("-an");
            }
        }
        EncodeMode::Encode(settings) => {
            // Same state machine as the multi-track path (just no
            // drawtext, since Simple mode doesn't expose captions).
            let vf_parts = build_video_post_chain(
                hw_decode,
                settings.resolution,
                None,
                settings.video_codec.needs_vaapi_upload(),
            );
            if !vf_parts.is_empty() {
                cmd.args(["-vf", &vf_parts.join(",")]);
            }
            for arg in settings.video_args() {
                cmd.arg(arg);
            }
            if audio_muted {
                cmd.arg("-an");
            } else {
                for arg in settings.audio_args() {
                    cmd.arg(arg);
                }
                for arg in audio_container_args(settings, destination) {
                    cmd.arg(arg);
                }
            }
            cmd.args(["-movflags", "+faststart"]);
        }
    }

    if let Some(fmt) = output_format_for(destination) {
        cmd.args(["-f", fmt]);
    }
    cmd.arg(destination)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    log::info!("simple export started");

    run_ffmpeg_with_progress(cmd, progress, cancel_flag, child_slot)
}

/// Resolves the VAAPI render-node device. Hardcoded for now —
/// /dev/dri/renderD128 is the default on every distro I've seen.
pub(crate) fn default_vaapi_device() -> &'static str {
    crate::encode::VAAPI_DEVICE
}

/// Pick an explicit ffmpeg muxer name for the destination so we don't
/// depend on `av_guess_format`. Some ffmpeg builds (notably the CachyOS
/// custom build) refuse to auto-detect when the input filter_complex is
/// complex enough — the symptom is "Unable to choose an output format".
/// Passing `-f` removes the ambiguity entirely.
pub(crate) fn output_format_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" | "m4v" => "mp4",
        "mkv" => "matroska",
        "mov" => "mov",
        "webm" => "webm",
        "avi" => "avi",
        _ => return None,
    })
}

// ------------------------------------------------------------------
// Multi-track path: filter_complex.
// ------------------------------------------------------------------

/// Earliest source timestamp (ms) referenced by any clip that will appear
/// in the filter graph — every video clip plus the audio clips of tracks
/// that actually emit (non-muted, non-empty). The multi-track path uses
/// this to fast-seek the input with `-ss` so ffmpeg decodes from just
/// before the first frame we need instead of from t=0; without it, a clip
/// taken from late in a long source forces a full decode-and-discard up to
/// the in-point (the "warming up" stall). Every `trim`/`atrim` is rebased
/// by this offset, which `-ss` makes correct: input seeking shifts output
/// timestamps so the seek point becomes 0.
///
/// Returns 0 when nothing is selected (callers guarantee ≥1 video clip).
/// The result is always ≤ every emitted clip's `src_start_ms`, so every
/// rebased trim value stays non-negative.
fn earliest_src_start_ms(video_clips: &[ClipRange], audio_tracks: &[AudioTrackPlan]) -> i64 {
    video_clips
        .iter()
        .map(|c| c.src_start_ms)
        .chain(
            audio_tracks
                .iter()
                .filter(|t| !t.muted && !t.clips.is_empty())
                .flat_map(|t| t.clips.iter())
                .map(|c| c.src_start_ms),
        )
        .min()
        .unwrap_or(0)
        .max(0)
}

/// The rightmost cut position anything places content at — video clips
/// plus the clips of audio tracks that actually emit. Both the video and
/// audio chains pad out to this point (black / silence) so their output
/// durations always match.
fn timeline_end_ms(video_clips: &[ClipRange], audio_tracks: &[AudioTrackPlan]) -> i64 {
    video_clips
        .iter()
        .map(ClipRange::cut_end_ms)
        .chain(
            audio_tracks
                .iter()
                .filter(|t| !t.muted && !t.clips.is_empty())
                .flat_map(|t| t.clips.iter())
                .map(ClipRange::cut_end_ms),
        )
        .max()
        .unwrap_or(0)
}

/// Real (wall-clock) duration a concat chain of `clips` produces when
/// padded out to `timeline_end_ms`: gaps keep their cut-time length,
/// content is compressed/stretched by each clip's speed. Used for the
/// progress bar's total, so speed clips don't leave the bar stuck at 60%
/// when ffmpeg finishes.
pub fn output_duration_ms(clips: &[ClipRange], timeline_end_ms: i64) -> i64 {
    let mut cursor: i64 = 0;
    let mut out: f64 = 0.0;
    for c in clips {
        if c.cut_start_ms > cursor {
            out += (c.cut_start_ms - cursor) as f64;
        }
        out += c.src_duration_ms() as f64 / c.speed.max(0.01) as f64;
        cursor = cursor.max(c.cut_end_ms());
    }
    if timeline_end_ms > cursor {
        out += (timeline_end_ms - cursor) as f64;
    }
    out.round() as i64
}

#[allow(clippy::too_many_arguments)]
fn run_multitrack(
    source: &Path,
    destination: &Path,
    source_codec: &str,
    src_dims: (u32, u32),
    src_fps: f64,
    video_clips: &[ClipRange],
    video_text_overlays: &[Option<crate::track::TextOverlay>],
    audio_tracks: &[AudioTrackPlan],
    encode: &EncodeSettings,
    progress: &Arc<Mutex<ExportProgress>>,
    cancel_flag: &Arc<AtomicBool>,
    child_slot: &Arc<Mutex<Option<Child>>>,
) -> Result<()> {
    if video_clips.is_empty() {
        anyhow::bail!("Multi-track export requires at least one video clip.");
    }

    let hw_decode = if is_vaapi_decodable(source_codec) {
        encode.video_codec.hw_decode()
    } else {
        HwDecode::None
    };

    // Fast-seek the input to the earliest frame any clip needs, then build
    // the trims relative to that point. Eliminates the decode-from-zero
    // stall for clips taken from late in a long source.
    let seek_ms = earliest_src_start_ms(video_clips, audio_tracks);

    // Both chains pad to this shared end so video and audio always come
    // out the same length (and stay in sync across gaps).
    let end_ms = timeline_end_ms(video_clips, audio_tracks);

    // Video chain. Gaps on the timeline (split + delete, dragged-apart
    // clips, a first clip that doesn't start at 0) become black segments,
    // matching the preview and the audio chain's silence padding. Without
    // them, concat squashes the gap out of the video while the audio keeps
    // it — everything after the first gap plays desynced.
    let (src_w, src_h) = src_dims;
    let (out_w, out_h) = encode.resolution.unwrap_or((src_w, src_h));
    let px_scale = if src_h > 0 {
        out_h as f64 / src_h as f64
    } else {
        1.0
    };
    let on_gpu_after_post = encode.video_codec.needs_vaapi_upload();
    let black_gap = |filter: &mut String, idx: usize, dur_ms: i64| -> String {
        // Every segment has already been scaled and handed to the
        // encoder's frame domain before concat. Match both for gaps.
        let bridge = if on_gpu_after_post {
            ",format=nv12,hwupload"
        } else {
            ""
        };
        filter.push_str(&format!(
            "color=c=black:s={}x{}:r={:.3}:d={:.3}{}[gap{}];\n",
            out_w.max(2),
            out_h.max(2),
            src_fps.clamp(1.0, 480.0),
            dur_ms as f64 / 1000.0,
            bridge,
            idx,
        ));
        format!("[gap{idx}]")
    };

    let mut filter = String::new();
    let mut video_labels: Vec<String> = Vec::with_capacity(video_clips.len());
    let mut gap_idx = 0usize;
    let mut video_cursor: i64 = 0;
    for (i, c) in video_clips.iter().enumerate() {
        if c.cut_start_ms > video_cursor {
            video_labels.push(black_gap(
                &mut filter,
                gap_idx,
                c.cut_start_ms - video_cursor,
            ));
            gap_idx += 1;
        }
        let speed = c.speed.max(0.01);
        let setpts_expr = if (speed - 1.0).abs() < 0.001 {
            "setpts=PTS-STARTPTS".to_string()
        } else {
            // (PTS-STARTPTS)/SPEED reverses the multiplier: speed=2
            // halves the timestamps, doubling effective playback rate.
            format!("setpts=(PTS-STARTPTS)/{speed:.4}")
        };
        // Burn the caption into this segment before joining segments.
        // Scope follows the actual frames, including speed changes and
        // concat's frame rounding, rather than an approximate time gate.
        let drawtext = video_text_overlays
            .get(i)
            .and_then(Option::as_ref)
            .filter(|o| !o.text.trim().is_empty())
            .map(|overlay| build_drawtext(overlay, px_scale));
        let post = build_video_post_chain(
            hw_decode,
            encode.resolution,
            drawtext.as_deref(),
            on_gpu_after_post,
        );
        let post_suffix = if post.is_empty() {
            String::new()
        } else {
            format!(",{}", post.join(","))
        };
        filter.push_str(&format!(
            "[0:v]trim=start={:.3}:end={:.3},{}{}[v{}];\n",
            (c.src_start_ms - seek_ms) as f64 / 1000.0,
            (c.src_end_ms - seek_ms) as f64 / 1000.0,
            setpts_expr,
            post_suffix,
            i,
        ));
        video_labels.push(format!("[v{}]", i));
        video_cursor = video_cursor.max(c.cut_end_ms());
    }
    if end_ms > video_cursor {
        video_labels.push(black_gap(&mut filter, gap_idx, end_ms - video_cursor));
    }

    filter.push_str(&video_labels.concat());
    filter.push_str(&format!("concat=n={}:v=1:a=0[vout];\n", video_labels.len()));

    // One audio chain per non-muted, non-empty track. Then amix them all.
    // If exactly one track survives, skip amix and route it straight to
    // [aout]. If none survive, output has no audio.
    let mut track_outputs: Vec<String> = Vec::new();
    let mut silence_idx: usize = 0;
    let mut clip_idx: usize = 0;
    for (track_i, track) in audio_tracks.iter().enumerate() {
        if track.muted || track.clips.is_empty() {
            continue;
        }
        let mut piece_labels: Vec<String> = Vec::new();
        let mut cursor: i64 = 0;
        for c in &track.clips {
            if c.cut_start_ms > cursor {
                let dur = (c.cut_start_ms - cursor) as f64 / 1000.0;
                filter.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo:duration={:.3}[s{}];\n",
                    dur, silence_idx,
                ));
                piece_labels.push(format!("[s{}]", silence_idx));
                silence_idx += 1;
            }
            let volume_suffix = if (c.gain - 1.0).abs() < 0.001 {
                String::new()
            } else {
                format!(",volume={:.3}", c.gain.max(0.0))
            };
            let tempo_suffix = if (c.speed - 1.0).abs() < 0.001 {
                String::new()
            } else {
                format!(",{}", atempo_chain(c.speed.max(0.01)))
            };
            filter.push_str(&format!(
                "[0:a:{}]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS{}{}[c{}];\n",
                track.source_audio_index,
                (c.src_start_ms - seek_ms) as f64 / 1000.0,
                (c.src_end_ms - seek_ms) as f64 / 1000.0,
                tempo_suffix,
                volume_suffix,
                clip_idx,
            ));
            piece_labels.push(format!("[c{}]", clip_idx));
            clip_idx += 1;
            cursor = c.cut_end_ms();
        }
        if end_ms > cursor {
            let dur = (end_ms - cursor) as f64 / 1000.0;
            filter.push_str(&format!(
                "anullsrc=r=48000:cl=stereo:duration={:.3}[s{}];\n",
                dur, silence_idx,
            ));
            piece_labels.push(format!("[s{}]", silence_idx));
            silence_idx += 1;
        }
        let concat_label = format!("[t{}]", track_i);
        filter.push_str(&piece_labels.concat());
        filter.push_str(&format!(
            "concat=n={}:v=0:a=1{};\n",
            piece_labels.len(),
            concat_label,
        ));
        // Apply track gain after concat so it weights the whole stream.
        let track_label = format!("[g{}]", track_i);
        filter.push_str(&format!(
            "{}volume={:.3}{};\n",
            concat_label,
            track.gain.max(0.0),
            track_label,
        ));
        track_outputs.push(track_label);
    }

    let want_audio = !track_outputs.is_empty();
    if want_audio {
        if track_outputs.len() == 1 {
            // Single audio track — just alias to [aout].
            filter.push_str(&format!("{}anull[aout];\n", track_outputs[0]));
        } else {
            // Mix all surviving tracks together.
            filter.push_str(&track_outputs.concat());
            filter.push_str(&format!(
                "amix=inputs={}:duration=longest:dropout_transition=0[aout];\n",
                track_outputs.len(),
            ));
        }
    }

    let mut cmd = crate::platform::command("ffmpeg");
    cmd.arg("-hide_banner")
        .args(["-loglevel", "error", "-y"])
        .args(["-progress", "pipe:1", "-nostats"]);
    let needs_vaapi_device =
        matches!(hw_decode, HwDecode::Vaapi) || encode.video_codec.needs_vaapi_upload();
    if needs_vaapi_device {
        // -init_hw_device binds the VAAPI device to a named handle that
        // -filter_hw_device then hands to filter_complex's hwupload step.
        // The older -vaapi_device alone wires up the encoder but not the
        // filter graph, which made hwupload silently stall.
        cmd.args([
            "-init_hw_device",
            &format!("vaapi=vahw:{}", default_vaapi_device()),
            "-filter_hw_device",
            "vahw",
        ]);
    }
    // HW decode flags must come before `-i`. They route input frames
    // directly into VAAPI surfaces so the filter graph and encoder both
    // see GPU frames — no per-frame CPU→GPU upload required.
    if matches!(hw_decode, HwDecode::Vaapi) {
        cmd.args([
            "-hwaccel",
            "vaapi",
            "-hwaccel_output_format",
            "vaapi",
            "-hwaccel_device",
            "vahw",
        ]);
    }
    // Input-side seek: must precede `-i`. Accurate-seeks to the keyframe
    // before `seek_ms`, decode-discards to the exact frame, and rebases
    // output timestamps to 0 — which is why the trims above subtract it.
    if seek_ms > 0 {
        cmd.args(["-ss", &format!("{:.3}", seek_ms as f64 / 1000.0)]);
    }
    cmd.arg("-i")
        .arg(source)
        .args(["-filter_complex", filter.trim_end().trim_end_matches(';')])
        .args(["-map", "[vout]"]);
    if want_audio {
        cmd.args(["-map", "[aout]"]);
    } else {
        cmd.arg("-an");
    }
    for arg in encode.video_args() {
        cmd.arg(arg);
    }
    if want_audio {
        for arg in encode.audio_args() {
            cmd.arg(arg);
        }
        for arg in audio_container_args(encode, destination) {
            cmd.arg(arg);
        }
    }
    cmd.args(["-movflags", "+faststart"]);
    if let Some(fmt) = output_format_for(destination) {
        cmd.args(["-f", fmt]);
    }
    cmd.arg(destination)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    log::info!("multitrack export started");

    run_ffmpeg_with_progress(cmd, progress, cancel_flag, child_slot)
}

/// Extra muxer flags a (codec, container) pair needs. Opus-in-MP4 is
/// still gated behind `-strict -2` in ffmpeg's MP4 muxer even though
/// playback support is widespread.
pub(crate) fn audio_container_args(encode: &EncodeSettings, destination: &Path) -> Vec<String> {
    let is_mp4_family = matches!(output_format_for(destination), Some("mp4") | Some("mov"));
    if encode.audio_codec == crate::encode::AudioCodec::Opus && is_mp4_family {
        vec!["-strict".into(), "-2".into()]
    } else {
        Vec::new()
    }
}

/// Render a TextOverlay into a drawtext filter expression. `px_scale`
/// converts source-pixel values (font size, outline width, shadow
/// offset) into output pixels — drawtext runs after the scale step, so
/// a 4K-source caption exported at 1080p must halve its pixel sizes to
/// keep the same on-screen proportion the preview showed.
pub(crate) fn build_drawtext(overlay: &crate::track::TextOverlay, px_scale: f64) -> String {
    let safe = drawtext_escape(&overlay.text);
    let color_hex = ((overlay.color[0] as u32) << 16)
        | ((overlay.color[1] as u32) << 8)
        | (overlay.color[2] as u32);
    #[cfg(not(windows))]
    let font_spec = format!(
        "font='{}'",
        crate::track::font_choice(&overlay.font).fontconfig
    );
    #[cfg(windows)]
    let font_spec = format!(
        "fontfile='{}'",
        drawtext_escape(
            &crate::platform::caption_font(&overlay.font)
                .to_string_lossy()
                .replace('\\', "/")
        )
    );

    let opacity = overlay.opacity.clamp(0.1, 1.0);
    let scale_px = |px: u32| ((px as f64 * px_scale).round() as u32).max(1);

    let mut expr = format!(
        "drawtext=expansion=none:text='{}':{}:x='w*{:.3}-text_w/2':\
         y='h*{:.3}-text_h/2':fontsize={}:fontcolor=#{:06X}@{:.2}",
        safe,
        font_spec,
        overlay.x.clamp(0.0, 1.0),
        overlay.y.clamp(0.0, 1.0),
        scale_px(overlay.font_size_px.clamp(8, 1000)),
        color_hex,
        opacity,
    );
    if overlay.outline_px > 0 {
        expr.push_str(&format!(
            ":borderw={}:bordercolor=black@{:.2}",
            scale_px(overlay.outline_px.min(20)),
            opacity,
        ));
    }
    if overlay.shadow {
        let off = scale_px(2);
        expr.push_str(&format!(
            ":shadowx={off}:shadowy={off}:shadowcolor=black@{:.2}",
            0.6 * opacity,
        ));
    }
    expr
}

// ------------------------------------------------------------------
// Shared: drive an ffmpeg child + parse -progress key=value stream.
// ------------------------------------------------------------------

fn run_ffmpeg_with_progress(
    mut cmd: Command,
    progress: &Arc<Mutex<ExportProgress>>,
    cancel_flag: &Arc<AtomicBool>,
    child_slot: &Arc<Mutex<Option<Child>>>,
) -> Result<()> {
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("ffmpeg spawn: {e}"))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    *child_slot.lock().unwrap() = Some(child);
    if cancel_flag.load(Ordering::SeqCst) {
        if let Some(c) = child_slot.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }

    let stderr_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stderr_buf_clone = stderr_buf.clone();
    let stderr_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut tail = std::collections::VecDeque::with_capacity(65536);
        let mut chunk = [0u8; 4096];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            tail.extend(&chunk[..n]);
            while tail.len() > 65536 {
                tail.pop_front();
            }
        }
        *stderr_buf_clone.lock().unwrap() =
            String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned();
    });

    let reader = BufReader::new(stdout);
    for line_res in reader.lines() {
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }
        let Ok(line) = line_res else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut p = progress.lock().unwrap();
        match key {
            // Both keys are microseconds — out_time_ms is a long-standing
            // ffmpeg misnomer.
            "out_time_us" | "out_time_ms" => {
                if let Ok(us) = value.parse::<i64>() {
                    p.current_ms = (us / 1000).clamp(0, p.total_ms);
                }
            }
            "fps" => {
                if let Ok(f) = value.parse::<f64>() {
                    p.fps = f;
                }
            }
            "speed" => {
                let v = value.trim_end_matches('x').trim();
                if let Ok(f) = v.parse::<f64>() {
                    p.speed = f;
                }
            }
            _ => {}
        }
    }

    let status = {
        let mut slot = child_slot.lock().unwrap();
        slot.take().and_then(|mut c| c.wait().ok())
    };
    let _ = stderr_thread.join();
    let err_text = stderr_buf.lock().unwrap().clone();

    if cancel_flag.load(Ordering::SeqCst) {
        anyhow::bail!("cancelled");
    }
    match status {
        Some(s) if s.success() => Ok(()),
        _ => {
            let detail = err_text.trim();
            if detail.is_empty() {
                anyhow::bail!("ffmpeg exited with an error")
            } else {
                anyhow::bail!("{detail}")
            }
        }
    }
}

/// Build each video segment's post-decode filter chain pieces, in the
/// right order, with `hwdownload` / `hwupload` bridges inserted so that
/// each step sees the surface type it expects.
///
/// The state machine is just one bool — `on_gpu` — tracking whether the
/// frame currently lives as a VAAPI surface or a CPU pixel buffer:
///
/// - Start on GPU iff `hw_decode == HwDecode::Vaapi` (the input filter
///   produced VAAPI surfaces).
/// - `scale_vaapi` consumes & produces GPU frames; `scale` is CPU-only.
/// - `drawtext` is CPU-only — if we're on GPU when we hit it, bridge
///   back via `hwdownload,format=nv12`.
/// - At the end, bring the frame to whichever surface type the encoder
///   expects (`encoder_wants_gpu`).
///
/// Returns the filter parts joined by the caller with `,`. An empty
/// result means the frame can flow straight from the trimmed segment
/// into the encoder with no transformation.
fn build_video_post_chain(
    hw_decode: HwDecode,
    resolution: Option<(u32, u32)>,
    drawtext_expr: Option<&str>,
    encoder_wants_gpu: bool,
) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut on_gpu = matches!(hw_decode, HwDecode::Vaapi);

    if let Some((w, h)) = resolution {
        if on_gpu {
            parts.push(format!("scale_vaapi={w}:{h}"));
        } else {
            parts.push(format!("scale={w}:{h}"));
        }
    }

    if let Some(expr) = drawtext_expr {
        if on_gpu {
            parts.push("hwdownload".into());
            parts.push("format=nv12".into());
            on_gpu = false;
        }
        parts.push(expr.to_string());
    }

    if encoder_wants_gpu && !on_gpu {
        parts.push("format=nv12".into());
        parts.push("hwupload".into());
    } else if !encoder_wants_gpu && on_gpu {
        parts.push("hwdownload".into());
        parts.push("format=nv12".into());
    }

    parts
}

/// Escape user text for ffmpeg's `drawtext=text='...'` value. The text is
/// already wrapped in single quotes by the caller; inside quotes we need
/// to escape the quote character and a couple of others ffmpeg parses.
pub(crate) fn drawtext_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '\'' => out.push_str(r"\'"),
            ':' => out.push_str(r"\:"),
            // Drop control chars to avoid breaking the filter graph.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Build an `atempo` filter expression that handles speeds outside
/// ffmpeg's 0.5–2.0 per-stage range. Doubles or halves repeatedly until
/// the residual factor lands in-range, then emits the final atempo.
pub(crate) fn atempo_chain(speed: f32) -> String {
    let mut s = speed.max(0.01);
    let mut parts: Vec<String> = Vec::new();
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s *= 2.0;
    }
    while s > 2.0 {
        parts.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    parts.push(format!("atempo={s:.4}"));
    parts.join(",")
}

pub fn suggest_output(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip");
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}_clip.{ext}"))
}

pub fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- build_video_post_chain ---------------------------------------
    //
    // The state machine has four axes (HwDecode × scale × drawtext ×
    // encoder placement) — exhaustive coverage at this layer guards
    // against regressions in the filter graph that would only surface
    // as broken exports.

    #[test]
    fn sw_decode_sw_encode_no_transform_is_empty() {
        let parts = build_video_post_chain(HwDecode::None, None, None, false);
        assert!(parts.is_empty(), "expected empty chain, got {parts:?}");
    }

    #[test]
    fn sw_decode_vaapi_encode_uploads_at_end() {
        // libx264 input feeding h264_vaapi: we have CPU frames, encoder
        // needs GPU frames → upload.
        let parts = build_video_post_chain(HwDecode::None, None, None, true);
        assert_eq!(parts, vec!["format=nv12", "hwupload"]);
    }

    #[test]
    fn hw_decode_vaapi_encode_no_scale_no_text_is_empty() {
        // The ideal fast path: GPU frames in, GPU frames out, no
        // intermediate transforms. Filter chain should be empty so
        // frames flow concat → encoder directly.
        let parts = build_video_post_chain(HwDecode::Vaapi, None, None, true);
        assert!(parts.is_empty(), "expected empty chain, got {parts:?}");
    }

    #[test]
    fn hw_decode_with_scale_uses_scale_vaapi() {
        let parts = build_video_post_chain(HwDecode::Vaapi, Some((1280, 720)), None, true);
        assert_eq!(parts, vec!["scale_vaapi=1280:720"]);
    }

    #[test]
    fn sw_decode_with_scale_uses_plain_scale() {
        let parts = build_video_post_chain(HwDecode::None, Some((1280, 720)), None, false);
        assert_eq!(parts, vec!["scale=1280:720"]);
    }

    #[test]
    fn hw_decode_with_drawtext_bridges_to_cpu_then_back() {
        // GPU frames in, drawtext is CPU-only, encoder wants GPU again:
        // expect hwdownload → drawtext → hwupload sandwich.
        let parts = build_video_post_chain(HwDecode::Vaapi, None, Some("drawtext=text='hi'"), true);
        assert_eq!(
            parts,
            vec![
                "hwdownload",
                "format=nv12",
                "drawtext=text='hi'",
                "format=nv12",
                "hwupload",
            ],
            "got {parts:?}"
        );
    }

    #[test]
    fn sw_decode_with_drawtext_no_bridges_needed() {
        // CPU frames throughout — no hwdownload/hwupload, just drawtext.
        let parts = build_video_post_chain(HwDecode::None, None, Some("drawtext=text='hi'"), false);
        assert_eq!(parts, vec!["drawtext=text='hi'"]);
    }

    #[test]
    fn hw_decode_to_sw_encoder_downloads_at_end() {
        // Mismatched: HW decode but a non-VAAPI encoder (libx264). We
        // need to bring frames back to CPU before the encoder.
        let parts = build_video_post_chain(HwDecode::Vaapi, None, None, false);
        assert_eq!(parts, vec!["hwdownload", "format=nv12"]);
    }

    #[test]
    fn hw_decode_scale_then_drawtext_then_upload() {
        // Full chain: scale on GPU, hop to CPU for drawtext, hop back
        // for the VAAPI encoder. Validates that order is preserved
        // (scale before drawtext) and bridges sit at the right hops.
        let parts = build_video_post_chain(
            HwDecode::Vaapi,
            Some((1920, 1080)),
            Some("drawtext=text='x'"),
            true,
        );
        assert_eq!(
            parts,
            vec![
                "scale_vaapi=1920:1080",
                "hwdownload",
                "format=nv12",
                "drawtext=text='x'",
                "format=nv12",
                "hwupload",
            ],
            "got {parts:?}"
        );
    }

    // ---- earliest_src_start_ms (input-seek offset) -------------------
    //
    // This drives the `-ss` fast-seek and the trim rebasing. Getting it
    // wrong either re-introduces the decode-from-zero stall (too small) or
    // produces negative trim values / a wrong segment (too large), so the
    // selection rules are pinned here.

    fn clip(src_start_ms: i64, src_end_ms: i64) -> ClipRange {
        ClipRange {
            cut_start_ms: 0,
            src_start_ms,
            src_end_ms,
            gain: 1.0,
            speed: 1.0,
        }
    }

    fn track(muted: bool, clips: Vec<ClipRange>) -> AudioTrackPlan {
        AudioTrackPlan {
            source_audio_index: 0,
            muted,
            gain: 1.0,
            clips,
        }
    }

    #[test]
    fn earliest_src_start_video_only() {
        let v = vec![clip(5_000, 9_000), clip(20_000, 22_000)];
        assert_eq!(earliest_src_start_ms(&v, &[]), 5_000);
    }

    #[test]
    fn earliest_src_start_takes_min_across_video_and_audio() {
        // Audio reaches further back than any video clip — the seek must
        // cover it, since `-ss` rebases every stream together.
        let v = vec![clip(10_000, 12_000)];
        let a = vec![track(false, vec![clip(8_000, 12_000)])];
        assert_eq!(earliest_src_start_ms(&v, &a), 8_000);
    }

    #[test]
    fn earliest_src_start_ignores_muted_and_empty_tracks() {
        // A muted track's earlier start must NOT pull the seek back — it
        // never enters the filter graph, so seeking to it would decode
        // (and discard) frames nothing references.
        let v = vec![clip(30_000, 31_000)];
        let a = vec![
            track(true, vec![clip(1_000, 2_000)]), // muted: ignored
            track(false, vec![]),                  // empty: ignored
        ];
        assert_eq!(earliest_src_start_ms(&v, &a), 30_000);
    }

    #[test]
    fn earliest_src_start_rebased_trims_are_non_negative() {
        // Property the rebasing relies on: seek ≤ every emitted start.
        let v = vec![clip(841_950, 869_599)];
        let a = vec![track(false, vec![clip(841_950, 869_599)])];
        let seek = earliest_src_start_ms(&v, &a);
        for c in v.iter().chain(a.iter().flat_map(|t| t.clips.iter())) {
            assert!(c.src_start_ms - seek >= 0, "trim start went negative");
            assert!(c.src_end_ms - seek >= 0, "trim end went negative");
        }
    }

    // ---- output timeline math -----------------------------------------

    fn placed_clip(cut_start: i64, src_start: i64, src_end: i64, speed: f32) -> ClipRange {
        ClipRange {
            cut_start_ms: cut_start,
            src_start_ms: src_start,
            src_end_ms: src_end,
            gain: 1.0,
            speed,
        }
    }

    #[test]
    fn timeline_end_covers_video_and_unmuted_audio() {
        let v = vec![placed_clip(0, 0, 5_000, 1.0)];
        let a = vec![
            track(false, vec![placed_clip(8_000, 0, 2_000, 1.0)]), // ends at 10s
            track(true, vec![placed_clip(50_000, 0, 1_000, 1.0)]), // muted: ignored
        ];
        assert_eq!(timeline_end_ms(&v, &a), 10_000);
    }

    #[test]
    fn output_duration_counts_gaps_and_speed() {
        // [0..10s at 2×] → 5s of output, gap to 20s → +10s, [20..30s] → +10s.
        let clips = vec![
            placed_clip(0, 0, 10_000, 2.0),
            placed_clip(20_000, 10_000, 20_000, 1.0),
        ];
        assert_eq!(output_duration_ms(&clips, 30_000), 25_000);
        // Trailing pad beyond the last clip is added too.
        assert_eq!(output_duration_ms(&clips, 35_000), 30_000);
    }

    #[test]
    fn output_duration_matches_cut_length_when_no_speed() {
        let clips = vec![
            placed_clip(0, 0, 4_000, 1.0),
            placed_clip(6_000, 4_000, 9_000, 1.0), // 2s gap before it
        ];
        assert_eq!(output_duration_ms(&clips, 11_000), 11_000);
    }

    // ---- drawtext rendering --------------------------------------------

    #[test]
    fn drawtext_scales_pixel_values_to_output_height() {
        let overlay = crate::track::TextOverlay {
            text: "hi".into(),
            font_size_px: 48,
            outline_px: 4,
            shadow: true,
            ..Default::default()
        };
        // 4K source exported at 1080p → everything halves.
        let expr = build_drawtext(&overlay, 0.5);
        assert!(expr.contains("fontsize=24"), "got {expr}");
        assert!(expr.contains("borderw=2"), "got {expr}");
        assert!(expr.contains("shadowx=1"), "got {expr}");
    }

    #[test]
    fn drawtext_opacity_reaches_all_layers() {
        let overlay = crate::track::TextOverlay {
            text: "hi".into(),
            outline_px: 2,
            opacity: 0.5,
            ..Default::default()
        };
        let expr = build_drawtext(&overlay, 1.0);
        assert!(expr.contains("fontcolor=#FFFFFF@0.50"), "got {expr}");
        assert!(expr.contains("bordercolor=black@0.50"), "got {expr}");
    }

    #[test]
    fn drawtext_omits_optional_layers_when_off() {
        let overlay = crate::track::TextOverlay {
            text: "hi".into(),
            ..Default::default()
        };
        let expr = build_drawtext(&overlay, 1.0);
        assert!(!expr.contains("borderw"), "got {expr}");
        assert!(!expr.contains("shadowx"), "got {expr}");
    }

    // ---- container gating ------------------------------------------------

    #[test]
    fn opus_in_mp4_needs_strict_flag() {
        let mut s = EncodeSettings::default_for_source(1920, 1080);
        s.audio_codec = crate::encode::AudioCodec::Opus;
        assert_eq!(
            audio_container_args(&s, Path::new("/tmp/out.mp4")),
            vec!["-strict".to_string(), "-2".to_string()],
        );
        // MKV muxes opus natively — no flag.
        assert!(audio_container_args(&s, Path::new("/tmp/out.mkv")).is_empty());
        // AAC never needs it.
        s.audio_codec = crate::encode::AudioCodec::Aac;
        assert!(audio_container_args(&s, Path::new("/tmp/out.mp4")).is_empty());
    }

    // ---- codec gating ------------------------------------------------
    //
    // The HW-decode fast path must only trigger when the source codec is
    // VAAPI-decodable. These tests pin the compatibility list so
    // accidentally widening it (and breaking exports of exotic sources)
    // gets caught.

    #[test]
    fn vaapi_decodable_accepts_common_codecs() {
        for codec in ["h264", "hevc", "h265", "vp9", "mpeg2video", "vc1"] {
            assert!(
                crate::encode::is_vaapi_decodable(codec),
                "expected {codec} to be accepted"
            );
        }
    }

    #[test]
    fn vaapi_decodable_rejects_av1_and_vp8() {
        // AV1 needs RDNA 3+/Arc + recent driver; safer to SW-decode it.
        // VP8 has spotty VAAPI support across vendors.
        for codec in ["av1", "vp8", "theora", "mpeg4", ""] {
            assert!(
                !crate::encode::is_vaapi_decodable(codec),
                "expected {codec} to be rejected"
            );
        }
    }

    #[test]
    fn vaapi_decodable_is_case_insensitive() {
        assert!(crate::encode::is_vaapi_decodable("H264"));
        assert!(crate::encode::is_vaapi_decodable("HEVC"));
    }

    // ---- VideoCodec::hw_decode -----------------------------------------

    #[test]
    fn vaapi_encoders_request_vaapi_decode() {
        assert_eq!(
            crate::encode::VideoCodec::VaapiH264.hw_decode(),
            HwDecode::Vaapi
        );
        assert_eq!(
            crate::encode::VideoCodec::VaapiHevc.hw_decode(),
            HwDecode::Vaapi
        );
    }

    #[test]
    fn non_vaapi_encoders_stay_on_sw_decode() {
        for c in [
            crate::encode::VideoCodec::X264,
            crate::encode::VideoCodec::X265,
            crate::encode::VideoCodec::NvencH264,
            crate::encode::VideoCodec::NvencHevc,
            crate::encode::VideoCodec::QsvH264,
            crate::encode::VideoCodec::AmfH264,
        ] {
            assert_eq!(
                c.hw_decode(),
                HwDecode::None,
                "{c:?} should stay on SW decode"
            );
        }
    }

    // ---- end-to-end pipeline tests (real ffmpeg) -----------------------
    //
    // ffmpeg is a hard runtime dependency of the app, so these run it for
    // real: generate a tiny synthetic source, export through the actual
    // spawn() pipeline, and probe the result. This is the only layer that
    // can catch filter-graph mistakes ffmpeg itself would reject (label
    // typos, surface-type mismatches, bad option/codec pairings).

    use crate::encode::{AudioCodec, EncodeQuality, VideoCodec};
    use std::time::Duration as StdDuration;

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("halo-clipper-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 10s 1280x720@30 test pattern + 440 Hz tone, h264+aac. Generated
    /// once, shared by every test (guarded by the .done marker because
    /// tests run in parallel).
    fn synth_source(with_audio: bool) -> PathBuf {
        static GENERATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = GENERATE.lock().unwrap();
        let name = if with_audio {
            "src_av.mp4"
        } else {
            "src_v.mp4"
        };
        let path = test_dir().join(name);
        let done = test_dir().join(format!("{name}.done"));
        if done.exists() {
            return path;
        }
        let mut cmd = crate::platform::command("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30:duration=10",
            ]);
        if with_audio {
            cmd.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=10"])
                .args(["-c:a", "aac", "-b:a", "96k"]);
        }
        cmd.args(["-c:v", "libx264", "-preset", "ultrafast", "-g", "30"])
            .arg(&path);
        let status = cmd.status().expect("ffmpeg not installed?");
        assert!(status.success(), "test-source generation failed");
        std::fs::write(&done, b"ok").unwrap();
        path
    }

    /// Run a plan through the real pipeline and wait for it to finish.
    fn run_to_completion(plan: ExportPlan) -> ExportProgress {
        let handle = spawn(plan).expect("spawn failed");
        let deadline = Instant::now() + StdDuration::from_secs(120);
        loop {
            let p = handle.progress();
            match p.phase {
                ExportPhase::Running => {
                    assert!(Instant::now() < deadline, "export timed out");
                    thread::sleep(StdDuration::from_millis(100));
                }
                _ => return p,
            }
        }
    }

    /// (duration_secs, codec_name) of the named stream, via ffprobe.
    fn probe_stream(path: &Path, selector: &str) -> Option<(f64, String)> {
        let out = crate::platform::command("ffprobe")
            .args(["-v", "error", "-select_streams", selector])
            .args(["-show_entries", "stream=codec_name,duration"])
            .args(["-of", "csv=p=0"])
            .arg(path)
            .output()
            .expect("ffprobe not installed?");
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next()?.trim();
        if line.is_empty() {
            return None;
        }
        let (codec, dur) = line.split_once(',')?;
        Some((dur.trim().parse().ok()?, codec.to_string()))
    }

    fn base_encode(codec: VideoCodec) -> EncodeSettings {
        EncodeSettings {
            resolution: None,
            video_codec: codec,
            quality: EncodeQuality::ConstantQuality(30),
            video_preset: if matches!(codec, VideoCodec::X264) {
                "ultrafast".into()
            } else {
                codec.default_preset().into()
            },
            fps_limit: None,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: 96,
        }
    }

    #[test]
    fn e2e_segment_captions_stay_on_their_own_segments() {
        let source = test_dir().join("caption_black.mp4");
        let status = crate::platform::command("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=320x180:rate=30:duration=7",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let dest = test_dir().join("caption_segments.mp4");
        let caption = |text: &str, x| {
            Some(crate::track::TextOverlay {
                text: text.into(),
                x,
                y: 0.5,
                font_size_px: 32,
                ..Default::default()
            })
        };
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 320,
            src_height: 180,
            src_fps: 30.0,
            video_clips: vec![
                placed_clip(0, 1_000, 3_000, 2.0),     // 0..1 output
                placed_clip(2_000, 3_000, 4_000, 1.0), // 1..2 output, adjacent
                placed_clip(4_000, 4_000, 5_000, 1.0), // gap 2..3, uncaptioned 3..4
            ],
            video_text_overlays: vec![caption("LEFT", 0.25), caption("RIGHT", 0.75), None],
            audio_tracks: Vec::new(),
            encode: base_encode(VideoCodec::X264),
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "{:?}",
            progress.phase
        );
        let (duration, _) = probe_stream(&dest, "v:0").unwrap();
        assert!((duration - 4.0).abs() < 0.1, "duration {duration}");
        for (time, left_expected, right_expected) in [
            (0.5, true, false),
            (0.967, true, false),
            (1.034, false, true),
            (1.5, false, true),
            (2.5, false, false),
            (3.5, false, false),
        ] {
            let out = crate::platform::command("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-ss",
                    &format!("{time:.3}"),
                    "-i",
                ])
                .arg(&dest)
                .args(["-frames:v", "1", "-pix_fmt", "gray", "-f", "rawvideo", "-"])
                .output()
                .unwrap();
            assert!(out.status.success());
            assert_eq!(out.stdout.len(), 320 * 180);
            let whites = |left: bool| {
                out.stdout
                    .iter()
                    .enumerate()
                    .filter(|(i, px)| (**px > 180) && ((*i % 320 < 160) == left))
                    .count()
            };
            assert_eq!(whites(true) > 100, left_expected, "left at {time}");
            assert_eq!(whites(false) > 100, right_expected, "right at {time}");
        }
    }

    #[test]
    fn e2e_gap_exports_black_and_keeps_av_sync() {
        // Two clips with a 2s gap between them ([0..3s], gap, [5s..8s]).
        // The output must be 8s on BOTH streams — before the gap fix,
        // video came out 6s (gap squashed) while audio came out 8s.
        let source = synth_source(true);
        let dest = test_dir().join("out_gap.mp4");
        let clips = vec![
            placed_clip(0, 1_000, 4_000, 1.0),
            placed_clip(5_000, 6_000, 9_000, 1.0),
        ];
        let overlay = crate::track::TextOverlay {
            text: "hi".into(),
            outline_px: 2,
            shadow: true,
            opacity: 0.9,
            ..Default::default()
        };
        let mut encode = base_encode(VideoCodec::X264);
        encode.resolution = Some((640, 360));
        encode.fps_limit = Some(30);
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 1280,
            src_height: 720,
            src_fps: 30.0,
            video_clips: clips.clone(),
            video_text_overlays: vec![Some(overlay); clips.len()],
            audio_tracks: vec![AudioTrackPlan {
                source_audio_index: 0,
                muted: false,
                gain: 1.0,
                clips,
            }],
            encode,
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "export failed: {:?}",
            progress.phase
        );
        let (vdur, _) = probe_stream(&dest, "v:0").expect("no video stream");
        let (adur, _) = probe_stream(&dest, "a:0").expect("no audio stream");
        assert!(
            (vdur - 8.0).abs() < 0.3,
            "video duration {vdur}, expected ~8s"
        );
        assert!(
            (adur - 8.0).abs() < 0.3,
            "audio duration {adur}, expected ~8s"
        );

        // The gap (3s..5s on the output) must actually be black.
        let out = crate::platform::command("ffmpeg")
            .args(["-hide_banner", "-i"])
            .arg(&dest)
            .args([
                "-vf",
                "blackdetect=d=1.5:pix_th=0.10",
                "-an",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let black = stderr
            .lines()
            .find(|l| l.contains("black_start"))
            .unwrap_or_else(|| panic!("no black interval detected:\n{stderr}"));
        let start: f64 = black
            .split("black_start:")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap();
        assert!(
            (start - 3.0).abs() < 0.4,
            "black gap starts at {start}, expected ~3s"
        );
    }

    #[test]
    fn e2e_speed_clip_shortens_both_streams() {
        // One 4s source range at 2× → ~2s of output on both streams.
        let source = synth_source(true);
        let dest = test_dir().join("out_speed.mp4");
        let clips = vec![placed_clip(0, 0, 4_000, 2.0)];
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 1280,
            src_height: 720,
            src_fps: 30.0,
            video_clips: clips.clone(),
            video_text_overlays: Vec::new(),
            audio_tracks: vec![AudioTrackPlan {
                source_audio_index: 0,
                muted: false,
                gain: 1.0,
                clips,
            }],
            encode: base_encode(VideoCodec::X264),
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "{:?}",
            progress.phase
        );
        assert_eq!(
            progress.total_ms, 2_000,
            "progress total should be speed-aware"
        );
        let (vdur, _) = probe_stream(&dest, "v:0").unwrap();
        let (adur, _) = probe_stream(&dest, "a:0").unwrap();
        assert!(
            (vdur - 2.0).abs() < 0.3,
            "video duration {vdur}, expected ~2s"
        );
        assert!(
            (adur - 2.0).abs() < 0.3,
            "audio duration {adur}, expected ~2s"
        );
    }

    #[test]
    fn e2e_silent_source_exports_without_audio() {
        // A source with no audio stream: the plan carries no audio tracks
        // (the app filters them out) and the export must still succeed.
        let source = synth_source(false);
        let dest = test_dir().join("out_silent.mp4");
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 1280,
            src_height: 720,
            src_fps: 30.0,
            video_clips: vec![placed_clip(0, 0, 3_000, 1.0)],
            video_text_overlays: Vec::new(),
            audio_tracks: Vec::new(),
            encode: base_encode(VideoCodec::X264),
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "{:?}",
            progress.phase
        );
        let (vdur, _) = probe_stream(&dest, "v:0").unwrap();
        assert!((vdur - 3.0).abs() < 0.3);
        assert!(
            probe_stream(&dest, "a:0").is_none(),
            "expected no audio stream"
        );
    }

    #[test]
    fn e2e_opus_in_mp4_muxes() {
        let source = synth_source(true);
        let dest = test_dir().join("out_opus.mp4");
        let mut encode = base_encode(VideoCodec::X264);
        encode.audio_codec = AudioCodec::Opus;
        let plan = ExportPlan::Simple {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_start_ms: 1_000,
            src_end_ms: 4_000,
            encode: EncodeMode::Encode(encode),
            audio_muted: false,
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "{:?}",
            progress.phase
        );
        let (_, acodec) = probe_stream(&dest, "a:0").expect("no audio stream");
        assert_eq!(acodec, "opus");
    }

    #[test]
    fn e2e_discord_av1_export_muxes_av1_in_mp4() {
        // The Discord planner can pick an AV1 encoder unattended, so the
        // whole path — plan → bitrate args → mp4 mux — gets exercised for
        // real. Uses whichever AV1 encoder this machine actually has;
        // skips (passes) when it has none.
        let available =
            crate::encode::verify_av1_encoders(crate::encode::detect_available_codecs());
        let Some(codec) =
            crate::encode::DiscordNitro::best_av1(crate::encode::detect_gpu(), &available)
        else {
            eprintln!("skipping: no usable AV1 encoder on this machine");
            return;
        };
        let source = synth_source(true);
        let dest = test_dir().join("out_av1.mp4");
        let clips = vec![placed_clip(0, 1_000, 4_000, 1.0)];
        // Exactly what a 4 MB Discord target on a 3s 720p clip produces.
        let encode = crate::encode::DiscordNitro::settings_for(
            3_000,
            crate::encode::DiscordNitro::target_bytes_from_mb(4),
            1280,
            720,
            codec,
        );
        assert!(matches!(encode.quality, EncodeQuality::Bitrate(_)));
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 1280,
            src_height: 720,
            src_fps: 30.0,
            video_clips: clips.clone(),
            video_text_overlays: Vec::new(),
            audio_tracks: vec![AudioTrackPlan {
                source_audio_index: 0,
                muted: false,
                gain: 1.0,
                clips,
            }],
            encode,
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "AV1 export with {} failed: {:?}",
            codec.ffmpeg_name(),
            progress.phase
        );
        let (vdur, vcodec) = probe_stream(&dest, "v:0").unwrap();
        assert_eq!(vcodec, "av1");
        assert!(
            (vdur - 3.0).abs() < 0.3,
            "video duration {vdur}, expected ~3s"
        );
        let (adur, _) = probe_stream(&dest, "a:0").unwrap();
        assert!(
            (adur - 3.0).abs() < 0.3,
            "audio duration {adur}, expected ~3s"
        );
    }

    #[test]
    fn e2e_vaapi_gap_graph_when_hardware_present() {
        // Full GPU path: VAAPI decode → per-segment scale_vaapi and
        // hwdownload/drawtext/hwupload → concat with black GPU gaps →
        // h264_vaapi CQP encode. Skips (passes) on machines without a
        // VAAPI-capable ffmpeg + render node.
        let have_vaapi = crate::encode::detect_available_codecs().contains(&VideoCodec::VaapiH264)
            && Path::new("/dev/dri/renderD128").exists();
        if !have_vaapi {
            eprintln!("skipping: no VAAPI encoder/render node on this machine");
            return;
        }
        let source = synth_source(true);
        let dest = test_dir().join("out_vaapi.mp4");
        let clips = vec![
            placed_clip(0, 1_000, 3_000, 1.0),
            placed_clip(4_000, 6_000, 8_000, 1.0),
        ];
        let overlay = crate::track::TextOverlay {
            text: "vaapi".into(),
            outline_px: 2,
            ..Default::default()
        };
        let mut encode = base_encode(VideoCodec::VaapiH264);
        encode.resolution = Some((640, 360));
        encode.fps_limit = Some(30);
        let plan = ExportPlan::MultiTrack {
            source,
            destination: dest.clone(),
            source_codec: "h264".into(),
            src_width: 1280,
            src_height: 720,
            src_fps: 30.0,
            video_clips: clips.clone(),
            video_text_overlays: vec![Some(overlay); clips.len()],
            audio_tracks: vec![AudioTrackPlan {
                source_audio_index: 0,
                muted: false,
                gain: 1.0,
                clips,
            }],
            encode,
        };
        let progress = run_to_completion(plan);
        assert!(
            matches!(progress.phase, ExportPhase::Done),
            "VAAPI export failed: {:?}",
            progress.phase
        );
        let (vdur, vcodec) = probe_stream(&dest, "v:0").unwrap();
        assert_eq!(vcodec, "h264");
        assert!(
            (vdur - 6.0).abs() < 0.3,
            "video duration {vdur}, expected ~6s"
        );
        let (adur, _) = probe_stream(&dest, "a:0").unwrap();
        assert!(
            (adur - 6.0).abs() < 0.3,
            "audio duration {adur}, expected ~6s"
        );
    }
}
