// Top-level UI + app state.
//
// Each frame we (1) poll the proxy transcode + player state, (2) consume
// keyboard shortcuts, (3) render top/bottom/central panels, and (4) draw
// the export modal on top if an export is in progress or just finished.
//
// Long-running work (proxy generation, ffmpeg export) lives on background
// threads. The UI never blocks on subprocess output.

#[path = "layers.rs"]
mod layers;
#[path = "support.rs"]
mod support;

use crate::encode::{
    self, AudioCodec, EncodeQuality, EncodeSettings, GpuVendor, ResolutionPreset, VideoCodec,
};
use crate::export::{self, EncodeMode, ExportHandle, ExportPhase, ExportPlan};
use crate::media::{self, MediaInfo};
use crate::player::Player;
use crate::proxy;
use crate::settings::{ProxyQuality, Settings};
use crate::theme;
use crate::timeline::{self, TimelineState};
use crate::track::{self, Clip, ClipId, IdGen, LinkId, TextOverlay, Track, TrackId, TrackKind};
use crate::video_fbo::{self, VideoFbo};

use egui::{Align2, Color32, Frame, Margin, RichText, Rounding, Stroke};
use glow::HasContext;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    /// Single In/Out range; instant export.
    SimpleClip,
    /// Multi-track timeline (V + A lanes, linked by default, detachable).
    MultiTrack,
}

struct Project {
    source: PathBuf,
    /// Scrub proxy file path (in the OS cache dir). When Some, mpv loads
    /// this instead of `source` for the preview, so scrubbing snaps to
    /// the proxy's short GOPs rather than the source's long ones. The
    /// EXPORT pipeline always uses `source` — proxy is preview-only and
    /// has zero impact on output quality.
    proxy: Option<PathBuf>,
    media: MediaInfo,
    /// In `SimpleClip` mode: source-time. In `MultiTrack`: cut-time.
    playhead_ms: i64,
    // SimpleClip state
    in_ms: Option<i64>,
    out_ms: Option<i64>,
    /// Simple Clip's V1 mute toggle. Skips the audio stream in the
    /// preview filter graph + adds `-an` on export.
    simple_audio_muted: bool,
    // MultiTrack state
    tracks: Vec<Track>,
    clips: Vec<Clip>,
    selected_clips: HashSet<ClipId>,
    /// Optional caption per video segment. Splitting copies the caption
    /// to both halves; each half can then be edited independently.
    text_overlays: HashMap<ClipId, TextOverlay>,
    id_gen: IdGen,
    mode: EditMode,
}

impl Project {
    /// Create V1 + one audio track per source audio stream (A1, A2, …).
    /// All linked by a single LinkId so edits propagate together until
    /// the user explicitly Detaches. No-op if tracks already exist.
    fn ensure_tracks_initialized(&mut self) {
        if !self.tracks.is_empty() {
            return;
        }
        let v_id = self.id_gen.next_track();
        let link = self.id_gen.next_link();
        self.tracks.push(Track {
            id: v_id,
            kind: TrackKind::Video,
            name: "V1".into(),
            muted: false,
            gain: 1.0,
            source_audio_index: None,
            source: None,
        });
        self.clips.push(Clip {
            id: self.id_gen.next_clip(),
            track_id: v_id,
            cut_start_ms: 0,
            src_start_ms: 0,
            src_end_ms: self.media.duration_ms,
            link: Some(link),
            gain: 1.0,
            speed: 1.0,
            transform: track::Transform::default(),
        });
        // One audio track per source audio stream. Guarantee at least
        // one so the data model has consistent shape even for silent
        // sources (export will just emit silence on the lone track).
        let audio_count = self.media.audio_stream_count.max(1);
        for i in 0..audio_count {
            let a_id = self.id_gen.next_track();
            self.tracks.push(Track {
                id: a_id,
                kind: TrackKind::Audio,
                name: format!("A{}", i + 1),
                muted: false,
                gain: 1.0,
                source_audio_index: Some(i),
                source: None,
            });
            self.clips.push(Clip {
                id: self.id_gen.next_clip(),
                track_id: a_id,
                cut_start_ms: 0,
                src_start_ms: 0,
                src_end_ms: self.media.duration_ms,
                link: Some(link),
                gain: 1.0,
                speed: 1.0,
                transform: track::Transform::default(),
            });
        }
    }

    fn needs_composition(&self) -> bool {
        if self.media.audio_stream_count > 1 || self.clips.iter().any(|c| c.link.is_none()) {
            return true;
        }
        if self
            .tracks
            .iter()
            .any(|t| t.kind == TrackKind::Video && t.muted)
        {
            return true;
        }
        self.tracks.iter().any(|t| t.source.is_some())
            || self
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .count()
                > 1
            || self.tracks.len() > self.media.audio_stream_count.max(1) + 1
            || self
                .clips
                .iter()
                .any(|c| c.transform != track::Transform::default())
    }

    fn composition(&self, preview: bool) -> crate::composition::Composition {
        let mut segments = Vec::new();
        // Top lane is painted last. All audio streams remain independent.
        for t in self.tracks.iter().rev().filter(|t| !t.muted) {
            let (source, info) = t
                .source
                .as_ref()
                .map(|s| (&s.path, &s.media))
                .unwrap_or((&self.source, &self.media));
            if t.kind == TrackKind::Audio
                && t.source_audio_index.unwrap_or(0) >= info.audio_stream_count
            {
                continue;
            }
            for c in self.clips.iter().filter(|c| c.track_id == t.id) {
                segments.push(crate::composition::Segment {
                    source: if preview && t.source.is_none() && t.kind == TrackKind::Video {
                        self.proxy.clone().unwrap_or_else(|| source.clone())
                    } else {
                        source.clone()
                    },
                    audio_index: (t.kind == TrackKind::Audio)
                        .then_some(t.source_audio_index.unwrap_or(0)),
                    start_ms: c.cut_start_ms,
                    end_ms: c.cut_end_ms(),
                    src_start_ms: c.src_start_ms,
                    src_end_ms: c.src_end_ms,
                    speed: c.speed,
                    gain: c.gain * t.gain,
                    transform: c.transform,
                    caption: self.text_overlays.get(&c.id).cloned(),
                    source_height: info.height,
                });
            }
        }
        crate::composition::Composition {
            width: self.media.width,
            height: self.media.height,
            fps: self.media.fps,
            duration_ms: self.content_end_ms(),
            segments,
        }
    }

    /// Preserve the caption on both independently editable split halves.
    fn split_segments(&mut self, target_ids: &[ClipId], cut_ms: i64) -> Vec<ClipId> {
        let captions_to_copy: Vec<(ClipId, TextOverlay)> = target_ids
            .iter()
            .filter_map(|id| self.text_overlays.get(id).cloned().map(|o| (*id, o)))
            .collect();
        let new_ids = track::split_clips(&mut self.clips, target_ids, cut_ms, &self.id_gen);
        for (left_id, overlay) in captions_to_copy {
            // split_clip inserts the right half immediately after its
            // original. Failed/boundary splits copy nothing.
            if let Some(left_index) = self.clips.iter().position(|c| c.id == left_id) {
                if let Some(right) = self.clips.get(left_index + 1) {
                    if new_ids.contains(&right.id) {
                        self.text_overlays.insert(right.id, overlay);
                    }
                }
            }
        }
        new_ids
    }

    fn active_caption(&self) -> Option<(ClipId, &TextOverlay)> {
        if self.mode != EditMode::MultiTrack {
            return None;
        }
        let v = self.first_video_track()?;
        let clip = track::clip_on_track_at(&self.clips, v, self.playhead_ms)?;
        let overlay = self.text_overlays.get(&clip.id)?;
        (!overlay.text.trim().is_empty()).then_some((clip.id, overlay))
    }

    #[cfg(test)]
    fn video_captions(&self, track_id: TrackId) -> Vec<Option<TextOverlay>> {
        sorted_track_clips(&self.clips, track_id)
            .into_iter()
            .map(|c| self.text_overlays.get(&c.id).cloned())
            .collect()
    }

    /// Stable canvas length for the Advanced timeline. Never shrinks
    /// below the original source duration — so deleting / left-dragging
    /// clips leaves empty space at the right edge instead of collapsing
    /// the rail (which would yank the visual reference out from under
    /// the editor). Grows past source duration only if a clip extends
    /// further (which the current UI doesn't permit, but kept for
    /// defense-in-depth).
    fn canvas_length_ms(&self) -> i64 {
        self.media
            .duration_ms
            .max(track::project_cut_duration(&self.clips))
    }

    /// True end of the cut — the rightmost clip-end position. Used as
    /// the playhead-readout total so the "where will this clip end"
    /// number tracks edits in real time, even though the canvas itself
    /// (above) stays put.
    fn content_end_ms(&self) -> i64 {
        track::project_cut_duration(&self.clips)
    }

    fn first_video_track(&self) -> Option<TrackId> {
        self.tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .map(|t| t.id)
    }
}

/// Snapshot of editable project state for undo/redo. Playhead and mode
/// are intentionally excluded — they're navigation/view state, not edits.
#[derive(Clone)]
struct UndoSnapshot {
    in_ms: Option<i64>,
    out_ms: Option<i64>,
    simple_audio_muted: bool,
    tracks: Vec<Track>,
    clips: Vec<Clip>,
    selected_clips: HashSet<ClipId>,
    text_overlays: HashMap<ClipId, TextOverlay>,
}

impl UndoSnapshot {
    fn capture(p: &Project) -> Self {
        Self {
            in_ms: p.in_ms,
            out_ms: p.out_ms,
            simple_audio_muted: p.simple_audio_muted,
            tracks: p.tracks.clone(),
            clips: p.clips.clone(),
            selected_clips: p.selected_clips.clone(),
            text_overlays: p.text_overlays.clone(),
        }
    }

    fn apply(self, p: &mut Project) {
        p.in_ms = self.in_ms;
        p.out_ms = self.out_ms;
        p.simple_audio_muted = self.simple_audio_muted;
        p.tracks = self.tracks;
        p.clips = self.clips;
        p.selected_clips = self.selected_clips;
        p.text_overlays = self.text_overlays;
    }
}

const UNDO_LIMIT: usize = 200;

/// Top-level export mode picked in the modal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    /// "Just do the right thing". Smart-cut for SimpleClip when nothing
    /// was modified beyond cutting; otherwise re-encode with default
    /// preserve-quality settings (libx264 CRF 18 + AAC 192k).
    Auto,
    /// Size-target encode that fits under a Discord upload cap (per-tier
    /// presets + slider). Always re-encodes via CBR with a resolution
    /// that matches the bitrate budget.
    Discord,
    /// User-controlled. Resolution / codec / quality / fps / audio all
    /// surfaced in the modal. Always re-encodes.
    Custom,
}

/// How Custom mode drives the encoder's rate control.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Constant quality — CRF / QP / CQ / ICQ depending on codec.
    /// Predictable quality, variable file size.
    ConstantQuality,
    /// Target bitrate with a maxrate cap. Predictable size, variable
    /// quality.
    Bitrate,
}

/// Persistent Custom-encode settings — kept on `ClipperApp` so the
/// user's last choices survive opening/closing the modal.
#[derive(Clone)]
pub struct CustomEncode {
    pub resolution: ResolutionPreset,
    pub video_codec: VideoCodec,
    pub rate_control: RateControl,
    /// Constant-quality value, interpreted per codec (see
    /// `VideoCodec::quality_name`). Reset to the codec's default when
    /// the codec changes because the scales don't line up.
    pub quality: u32,
    pub video_bitrate_kbps: u32,
    pub video_preset: String,
    /// None = source frame rate.
    pub fps_limit: Option<u32>,
    pub audio_codec: AudioCodec,
    pub audio_bitrate_kbps: u32,
    /// When false, only the headline controls are exposed. When true,
    /// the full set (encoder preset, audio codec/bitrate) unlocks.
    pub advanced_unlocked: bool,
}

impl CustomEncode {
    fn default_for(codec: VideoCodec) -> Self {
        Self {
            resolution: ResolutionPreset::Source,
            video_codec: codec,
            rate_control: RateControl::ConstantQuality,
            quality: codec.default_quality(),
            video_bitrate_kbps: ResolutionPreset::Source.default_bitrate_kbps(),
            video_preset: codec.default_preset().into(),
            fps_limit: None,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: 192,
            advanced_unlocked: false,
        }
    }

    /// Switch codec, resetting the codec-scale-dependent fields.
    fn set_codec(&mut self, codec: VideoCodec) {
        self.video_codec = codec;
        self.video_preset = codec.default_preset().into();
        self.quality = codec.default_quality();
    }

    fn to_encode_settings(&self, src_w: u32, src_h: u32) -> EncodeSettings {
        let quality = match self.rate_control {
            RateControl::ConstantQuality => EncodeQuality::ConstantQuality(self.quality),
            RateControl::Bitrate => EncodeQuality::Bitrate(self.video_bitrate_kbps),
        };
        EncodeSettings {
            resolution: self.resolution.dimensions(src_w, src_h),
            video_codec: self.video_codec,
            quality,
            video_preset: self.video_preset.clone(),
            fps_limit: self.fps_limit,
            audio_codec: self.audio_codec,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
        }
    }
}

/// Selection delta queued from the timeline widget. Resolved after the
/// per-frame `&mut self.project` borrow ends.
enum SelectionChange {
    /// Plain click — replace whatever was selected with this single clip.
    ReplaceWith(ClipId),
    /// Ctrl/Cmd+click — toggle this clip in/out of the selection.
    Toggle(ClipId),
    /// Click on bare lane background — drop the selection entirely.
    Clear,
}

// Editing intents queued from the bottom controls UI and applied after
// the per-frame `&mut self.project` borrow ends (so push_undo() can run).
enum ClipAction {
    SetIn,
    SetOut,
    Clear,
    /// Simple Clip "S" key: cycles In → Out → restart.
    CycleMark,
    /// Multi-track "S": split every clip intersecting the playhead.
    Split,
    /// Multi-track "Del": delete selected clip (and its linked partners).
    DeleteSelected,
    /// Multi-track "Detach": break the link on the selected clip's group.
    DetachSelected,
}

#[derive(Default)]
struct ShortcutActions {
    toggle_play: bool,
    toggle_mute: bool,
    undo: bool,
    redo: bool,
}

pub struct ClipperApp {
    project: Option<Project>,
    updater: crate::updater::Updater,
    support_open: bool,
    layered_preview: crate::preview::Preview,
    preview_is_composed: bool,
    layer_edit_active: bool,
    status: String,
    export_mode: ExportMode,
    /// Custom-encode settings persist between modal sessions so the user
    /// doesn't lose their preferences each time they cancel + reopen.
    custom_encode: CustomEncode,
    /// Optional encoders this machine can actually run — every hardware
    /// encoder ffmpeg advertises, plus software AV1. Detected once at
    /// startup (hardware AV1 entries are additionally test-encoded, see
    /// `encode::verify_av1_encoders`); surfaced in the Custom codec
    /// dropdown and used by the Discord planner.
    available_codecs: Vec<VideoCodec>,
    /// Discord mode's codec row: Auto (the size-aware picker) or a
    /// pinned family. Session state, like the target-size slider.
    discord_codec_choice: encode::CodecChoice,
    /// GPU vendor probe from /sys/class/drm. Used together with
    /// `available_codecs` to recommend the best encoder.
    detected_gpu: Option<GpuVendor>,
    export: Option<ExportHandle>,
    player: Arc<Player>,
    // Cached volume value for the slider so dragging feels instant while we
    // still respect mpv-reported changes.
    volume_ui: f64,
    // Offscreen FBO mpv renders into. Created lazily on first paint when
    // we have a glow::Context to allocate it against.
    video_fbo: Arc<Mutex<Option<VideoFbo>>>,
    // User-controlled preview width (height derives from source aspect).
    // None until the user first interacts with the resize grip; defaults
    // each frame to "fill most of available width" if None.
    preview_width: Option<f32>,
    // Playhead-ownership tracking. While the user is dragging the
    // scrubber or timeline, we DON'T mirror mpv's reported position back
    // onto the playhead — otherwise mpv's lagging time-pos events fight
    // the user's drag. On drag-release we kick off a short cooldown so
    // mpv has time to settle on the final exact-seek before mirror
    // resumes.
    user_dragging_seek: bool,
    was_dragging_seek: bool,
    seek_cooldown_until: Option<std::time::Instant>,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
    /// When true, the Export Options modal is shown. The actual export
    /// is kicked off by the modal's "Begin Export" button.
    export_options_open: bool,
    /// Edge-trigger state so mpv's `lavfi-complex` and `mute` properties
    /// are only updated when the desired state actually changes.
    last_lavfi_complex: String,
    last_audio_track_muted: bool,
    /// Edge-trigger state for `mpv.speed` — only push a property change
    /// when the V1 clip under the playhead changes its speed setting.
    last_clip_speed: f32,
    /// Active clip-drag session (None when no drag in progress). Tracks
    /// the mouse-driven "intended" cut position separately from the
    /// (possibly snapped) actual position so snap stickiness has hysteresis.
    clip_drag_session: Option<ClipDragSession>,
    /// In-flight scrub-proxy transcode (None when cache hit or not yet
    /// started). On completion, update() swaps mpv to the finished file
    /// at the current playhead.
    proxy_handle: Option<proxy::ProxyHandle>,
    /// Persistent user preferences (auto-clear-cache, etc). Loaded once
    /// at startup, mutated in the Settings modal, saved on close.
    settings: Settings,
    /// True while the File → Settings modal is open.
    settings_open: bool,
    /// User-selected Discord target file size, in MB. Tier chips set it
    /// to the matching upload cap; the slider allows anything between
    /// the free tier's 10 MB and Nitro's headroom-adjusted 475 MB.
    /// Persists for the session; not saved to disk.
    discord_target_mb: u32,
}

struct ClipDragSession {
    primary: ClipId,
    /// cut-ms per pixel — used to convert px-based snap thresholds.
    cut_per_px: f32,
    /// Where the mouse "wants" the primary clip's cut_start_ms to be,
    /// before snap is applied. Drifts forward with every drag delta.
    intended_cut: i64,
    /// Currently-snapped target cut_start (if any). Stays sticky while
    /// the mouse is within `SNAP_OUT_PX` of this value.
    snapped_to: Option<i64>,
}

impl ClipperApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        theme::apply(&cc.egui_ctx);
        let player = Arc::new(Player::new(cc.egui_ctx.clone())?);
        // eframe has made the GL context current here. Attach the video
        // output before main() can open a command-line file: loading first
        // races the initial paint, and mpv disables video permanently if
        // vo=libmpv starts without a render context (audio still plays).
        player.ensure_render_context()?;
        // One probe each — the results feed both the stored fields and
        // the Custom-encode defaults.
        // Hardware AV1 gets a real test encode: ffmpeg lists the encoder
        // on machines whose silicon has no AV1 encode block, and the
        // Discord planner picks from this list without asking.
        let available_codecs = encode::verify_av1_encoders(encode::detect_available_codecs());
        let detected_gpu = encode::detect_gpu();
        let recommended = encode::recommend_codec(detected_gpu, &available_codecs);
        let settings = Settings::load();
        let mut updater = crate::updater::Updater::default();
        if settings.check_updates_on_startup {
            updater.check();
        }
        Ok(Self {
            project: None,
            updater,
            support_open: false,
            layered_preview: crate::preview::Preview::default(),
            preview_is_composed: false,
            layer_edit_active: false,
            status: "Open a video to begin.".into(),
            export_mode: ExportMode::Auto,
            custom_encode: CustomEncode::default_for(recommended),
            available_codecs,
            detected_gpu,
            export: None,
            player,
            volume_ui: 100.0,
            video_fbo: Arc::new(Mutex::new(None)),
            preview_width: None,
            user_dragging_seek: false,
            was_dragging_seek: false,
            seek_cooldown_until: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            export_options_open: false,
            last_lavfi_complex: String::new(),
            last_audio_track_muted: false,
            last_clip_speed: 1.0,
            clip_drag_session: None,
            proxy_handle: None,
            settings,
            settings_open: false,
            discord_target_mb: encode::DiscordNitro::MAX_TARGET_MB,
            discord_codec_choice: encode::CodecChoice::default(),
        })
    }

    /// Mirror the V1 clip's `speed` onto mpv's `speed` property. mpv
    /// has only one speed knob so this follows whichever V1 clip is
    /// under the playhead in MultiTrack mode; SimpleClip and absence-of-
    /// project both fall back to 1.0.
    ///
    /// Edge-triggered to avoid hammering mpv every frame.
    fn sync_clip_speed(&mut self) {
        let desired_speed: f32 = match self.project.as_ref() {
            Some(p) if p.mode == EditMode::MultiTrack && !p.needs_composition() => p
                .first_video_track()
                .and_then(|v| track::clip_on_track_at(&p.clips, v, p.playhead_ms))
                .map(|c| c.speed)
                .unwrap_or(1.0),
            _ => 1.0,
        };
        if (desired_speed - self.last_clip_speed).abs() > 0.001 {
            if let Err(e) = self.player.set_speed(desired_speed as f64) {
                log::warn!("set_speed failed: {e:#}");
            }
            self.last_clip_speed = desired_speed;
        }
    }

    /// Build mpv's complete lavfi-complex filter graph from the current
    /// project state (audio mix) and
    /// push it to mpv when it differs from last frame. Edge-triggered so
    /// it doesn't fight the user's manual speaker-button mute every frame.
    ///
    /// SimpleClip: no filter — mpv plays the source's default streams;
    /// only the mute toggle is honored.
    ///
    /// MultiTrack: audio mix from unmuted tracks. Segment captions are
    /// composited in egui, independently of mpv's audio filter graph.
    fn sync_audio_mix(&mut self) {
        let (desired_filter, desired_mute) = self.build_lavfi_complex();
        if desired_filter != self.last_lavfi_complex {
            if let Err(e) = self.player.set_lavfi_complex(&desired_filter) {
                log::warn!("set lavfi-complex failed: {e:#}");
            }
            self.last_lavfi_complex = desired_filter;
            // The preview's video frame doesn't depend on the audio
            // filter; text overlays are composited in egui on top of the
            // preview now, so no seek-to-redraw is needed. mpv applies
            // the new audio chain on the next decoded frame naturally.
        }
        if desired_mute != self.last_audio_track_muted {
            let _ = self.player.set_mute(desired_mute);
            self.last_audio_track_muted = desired_mute;
        }
    }

    fn build_lavfi_complex(&self) -> (String, bool) {
        let Some(p) = self.project.as_ref() else {
            return (String::new(), false);
        };
        if p.mode != EditMode::MultiTrack {
            return (String::new(), p.simple_audio_muted);
        }
        if p.needs_composition() {
            return (String::new(), false);
        }
        // Preview filter graph is audio-only — text overlays composite
        // in egui (see show_text_overlay_handle). Keeps drag smooth.
        let audio_chain = build_audio_chain(p);
        let mute = audio_chain.is_none();
        (audio_chain.unwrap_or_default(), mute)
    }

    /// Capture the current edit state before a mutating action.
    /// Clears the redo stack — any new edit invalidates the redo history.
    fn push_undo(&mut self) {
        let Some(project) = &self.project else { return };
        self.undo_stack.push(UndoSnapshot::capture(project));
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        let Some(snap) = self.undo_stack.pop() else {
            return;
        };
        let Some(project) = &mut self.project else {
            return;
        };
        let current = UndoSnapshot::capture(project);
        snap.apply(project);
        self.redo_stack.push(current);
        self.clamp_playhead_after_state_change();
    }

    fn redo(&mut self) {
        let Some(snap) = self.redo_stack.pop() else {
            return;
        };
        let Some(project) = &mut self.project else {
            return;
        };
        let current = UndoSnapshot::capture(project);
        snap.apply(project);
        self.undo_stack.push(current);
        self.clamp_playhead_after_state_change();
    }

    /// After an undo/redo restores segments, the playhead may be past the
    /// new cut duration. Pull it back in range so the timeline doesn't lie.
    fn clamp_playhead_after_state_change(&mut self) {
        let Some(project) = &mut self.project else {
            return;
        };
        if project.mode == EditMode::MultiTrack {
            let total = project.canvas_length_ms();
            if project.playhead_ms > total {
                project.playhead_ms = total;
            }
            // Drop any selection IDs that no longer point at a real clip.
            project
                .selected_clips
                .retain(|id| track::clip_by_id(&project.clips, *id).is_some());
        }
    }

    /// True when a VAAPI encoder is available — proxy generation can
    /// then ride on the GPU instead of libx264. Drops the wall-clock
    /// for a 14-min 4K source from ~6 minutes to ~2 minutes on AMD VCN.
    fn proxy_can_use_vaapi(&self) -> bool {
        if cfg!(windows) {
            return false;
        }
        self.available_codecs
            .iter()
            .any(|c| matches!(c, VideoCodec::VaapiH264))
    }

    /// Poll the active proxy transcode. When it finishes, swap mpv to
    /// the proxy at the current playhead so the scrub-snappy version
    /// takes over transparently.
    fn tick_proxy(&mut self) {
        let Some(handle) = self.proxy_handle.as_ref() else {
            return;
        };
        let progress = handle.progress();

        if let Some(err) = &progress.failed {
            log::warn!("scrub-proxy generation failed: {err}");
            self.status = "Scrub proxy generation failed — falling back to source decode.".into();
            self.proxy_handle = None;
            return;
        }
        if progress.done {
            if let (Some(dest), Some(project)) = (progress.destination, self.project.as_mut()) {
                // Mid-edit swap: preserve playhead. mpv re-loads at the
                // current frame (rounded to its keyframe-step internally).
                let start_ms = project.playhead_ms;
                if project.mode == EditMode::MultiTrack && project.needs_composition() {
                    project.proxy = Some(dest);
                    self.status = "Scrub proxy ready for layered preview.".into();
                } else if let Err(e) = self.player.set_file(&dest, start_ms) {
                    log::warn!("proxy swap failed: {e:#}");
                } else {
                    project.proxy = Some(dest);
                    self.status = "Scrub proxy ready. Timeline scrubbing is now smooth.".into();
                }
            }
            self.proxy_handle = None;
            return;
        }

        // Mid-transcode: keep the status bar updated with %.
        let pct = (progress.fraction() * 100.0).round() as i32;
        self.status = format!("Generating scrub proxy… {pct}%");
    }

    fn open_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v"])
            .pick_file()
        else {
            return;
        };
        self.open_path(path);
    }

    /// Open a source file directly — used by the file dialog, and by
    /// main() when a path arrives on the command line ("Open With" from
    /// a file manager).
    pub fn open_path(&mut self, path: PathBuf) {
        crate::diagnostics::register_media(&path);
        match media::probe(&path) {
            Ok(info) => {
                self.layered_preview.reset();
                self.preview_is_composed = false;
                let size_str = std::fs::metadata(&path)
                    .ok()
                    .map(|m| format_bytes(m.len()))
                    .unwrap_or_else(|| "?".into());
                self.status = format!(
                    "{}  ·  {}  ·  {}×{}  ·  {:.0}fps  ·  {}",
                    path.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    size_str,
                    info.width,
                    info.height,
                    info.fps,
                    media::format_timestamp(info.duration_ms),
                );
                // Cancel any in-flight proxy from a previous open.
                if let Some(h) = self.proxy_handle.take() {
                    h.cancel();
                }

                // Decide preview source: cached proxy if available,
                // otherwise the original (we'll kick off a transcode
                // below so subsequent opens hit the cache).
                let proxy_quality = self.settings.proxy_quality;
                let cached_proxy = if proxy::proxy_exists(&path, proxy_quality) {
                    proxy::proxy_path_for(&path, proxy_quality)
                } else {
                    None
                };
                let initial_preview = cached_proxy.as_deref().unwrap_or(&path);

                if let Err(e) = self.player.set_file(initial_preview, 0) {
                    self.status = format!("mpv could not load file: {e:#}");
                    log::error!("player set_file failed: {e:#}");
                }

                // No cache hit → start a background transcode. The
                // preview stays on the original source until the proxy
                // finishes; then update() swaps mpv over.
                if cached_proxy.is_none() {
                    match proxy::spawn_transcode(
                        &path,
                        info.duration_ms,
                        self.proxy_can_use_vaapi(),
                        proxy_quality,
                    ) {
                        Ok(h) => {
                            self.proxy_handle = Some(h);
                            self.status = format!(
                                "{} loaded. Generating scrub proxy…",
                                path.file_name().and_then(|s| s.to_str()).unwrap_or("video")
                            );
                        }
                        Err(e) => {
                            log::warn!("proxy spawn failed: {e:#}");
                        }
                    }
                }

                self.project = Some(Project {
                    source: path,
                    proxy: cached_proxy,
                    media: info.clone(),
                    playhead_ms: 0,
                    in_ms: None,
                    out_ms: None,
                    simple_audio_muted: false,
                    tracks: Vec::new(),
                    clips: Vec::new(),
                    selected_clips: HashSet::new(),
                    text_overlays: HashMap::new(),
                    id_gen: IdGen::default(),
                    mode: EditMode::SimpleClip,
                });
                self.undo_stack.clear();
                self.redo_stack.clear();
            }
            Err(e) => {
                self.status = format!("Could not open file: {e:#}");
                log::error!("probe failed: {e:#}");
            }
        }
    }

    /// Drive playback in MultiTrack mode. mpv plays the source linearly;
    /// when it crosses the end of the current video clip we either jump
    /// to the next clip on V1 (if any) or pause + reset to the start of
    /// the cut.
    fn tick_multitrack_playback(&mut self, mpv_src_ms: i64) {
        let Some(project) = &self.project else { return };
        if project.needs_composition() {
            if self.layered_preview.ready {
                if let Some(p) = &mut self.project {
                    p.playhead_ms = mpv_src_ms.min(p.content_end_ms());
                }
            }
            return;
        }
        let Some(video_track) = project.first_video_track() else {
            return;
        };

        let cut_ms = project.playhead_ms;
        let clip = match track::clip_on_track_at(&project.clips, video_track, cut_ms).copied() {
            Some(c) => c,
            None => {
                // Playhead landed in a gap on V1 (split + delete leaves
                // holes). Skip forward to the next clip and keep playing
                // — pausing here would fight the user's Play press because
                // the next frame's tick would re-pause immediately.
                let next = project
                    .clips
                    .iter()
                    .filter(|c| c.track_id == video_track && c.cut_start_ms > cut_ms)
                    .min_by_key(|c| c.cut_start_ms)
                    .copied();
                match next {
                    Some(n) => {
                        if let Some(p) = &mut self.project {
                            p.playhead_ms = n.cut_start_ms;
                        }
                        let _ = self.player.seek(n.src_start_ms);
                    }
                    None => {
                        // Nothing ahead — pause at end of content, don't
                        // snap to 0 (that would silently rewind the user).
                        let _ = self.player.set_paused(true);
                        if let Some(p) = &mut self.project {
                            p.playhead_ms = p.content_end_ms();
                        }
                    }
                }
                return;
            }
        };

        // mpv lags a couple frames behind seeks. Small epsilon prevents
        // false boundary crossings immediately after a jump.
        const BOUNDARY_EPS_MS: i64 = 15;

        if mpv_src_ms + BOUNDARY_EPS_MS >= clip.src_end_ms {
            let next = project
                .clips
                .iter()
                .filter(|c| c.track_id == video_track && c.cut_start_ms >= clip.cut_end_ms())
                .min_by_key(|c| c.cut_start_ms)
                .copied();
            match next {
                Some(n) => {
                    if let Some(p) = &mut self.project {
                        p.playhead_ms = n.cut_start_ms;
                    }
                    let _ = self.player.seek(n.src_start_ms);
                }
                None => {
                    let first_src = project
                        .clips
                        .iter()
                        .filter(|c| c.track_id == video_track)
                        .min_by_key(|c| c.cut_start_ms)
                        .map(|c| c.src_start_ms)
                        .unwrap_or(0);
                    let _ = self.player.set_paused(true);
                    if let Some(p) = &mut self.project {
                        p.playhead_ms = 0;
                    }
                    let _ = self.player.seek(first_src);
                }
            }
            return;
        }

        // Normal mirror: walk through the current clip's source range.
        if mpv_src_ms >= clip.src_start_ms {
            let offset =
                ((mpv_src_ms - clip.src_start_ms).max(0) as f64 / clip.speed as f64).round() as i64;
            if let Some(p) = &mut self.project {
                p.playhead_ms = clip.cut_start_ms + offset;
            }
        }
    }

    /// Translate the playhead (mode-dependent units) into a source-time
    /// suitable for handing to mpv. In MultiTrack mode this maps via the
    /// V1 clip at the playhead — returns None if no clip covers it (mpv
    /// shouldn't seek anywhere in that case).
    fn playhead_to_src(&self) -> Option<i64> {
        let p = self.project.as_ref()?;
        match p.mode {
            EditMode::SimpleClip => Some(p.playhead_ms),
            EditMode::MultiTrack => {
                if p.needs_composition() {
                    return self.layered_preview.ready.then_some(p.playhead_ms);
                }
                let v = p.first_video_track()?;
                let clip = track::clip_on_track_at(&p.clips, v, p.playhead_ms)?;
                clip.cut_to_src(p.playhead_ms)
            }
        }
    }

    /// Slide a clip (and any linked partners) along the cut timeline by
    /// `delta_ms`. Magnetic snap engages near the playhead, other clips,
    /// and the canvas boundaries with hysteresis so the user "feels a
    /// tug" before breaking through. Group clamping keeps linked clips
    /// aligned when one hits a boundary.
    fn apply_clip_drag(&mut self, id: ClipId, delta_ms: i64) {
        // Pixel-space snap thresholds — kept here so they're a single
        // place to tune the feel.
        const SNAP_IN_PX: f32 = 8.0;
        const SNAP_OUT_PX: f32 = 16.0;

        let Some(session) = self.clip_drag_session.as_mut() else {
            return;
        };
        if session.primary != id {
            return;
        }
        session.intended_cut += delta_ms;

        let Some(p) = &mut self.project else { return };
        let Some(primary) = track::clip_by_id(&p.clips, id).copied() else {
            return;
        };
        let primary_dur = primary.duration_ms();
        let primary_link = primary.link;
        let canvas = p.canvas_length_ms();
        let playhead = p.playhead_ms;

        let snap_in_ms = ((SNAP_IN_PX * session.cut_per_px) as i64).max(1);
        let snap_out_ms = ((SNAP_OUT_PX * session.cut_per_px) as i64).max(2);

        // Hysteresis: stay snapped to the previous target while the mouse
        // is within snap_out_ms; once it breaks free, try a fresh snap.
        let effective = match session.snapped_to {
            Some(prev) if (session.intended_cut - prev).abs() <= snap_out_ms => prev,
            _ => {
                session.snapped_to = None;
                if let Some(t) = find_snap_target(
                    session.intended_cut,
                    primary_dur,
                    id,
                    primary_link,
                    &p.clips,
                    playhead,
                    canvas,
                    snap_in_ms,
                ) {
                    session.snapped_to = Some(t);
                    t
                } else {
                    session.intended_cut
                }
            }
        };
        let effective = effective.max(0);

        let group_delta = effective - primary.cut_start_ms;
        if group_delta == 0 {
            return;
        }

        // Apply the delta uniformly to the link group — clamped against
        // every member's individual bounds so the group keeps alignment.
        let group: Vec<ClipId> = match primary_link {
            Some(l) => p
                .clips
                .iter()
                .filter(|c| c.link == Some(l))
                .map(|c| c.id)
                .collect(),
            None => vec![id],
        };
        let mut allowed = group_delta;
        for cid in &group {
            let Some(c) = track::clip_by_id(&p.clips, *cid) else {
                continue;
            };
            if group_delta < 0 {
                allowed = allowed.max(-c.cut_start_ms);
            }
        }
        if allowed == 0 {
            return;
        }
        for cid in &group {
            let c = track::clip_by_id(&p.clips, *cid).unwrap();
            let start = c.cut_start_ms + allowed;
            if p.clips.iter().any(|other| {
                other.track_id == c.track_id
                    && !group.contains(&other.id)
                    && start < other.cut_end_ms()
                    && start + c.duration_ms() > other.cut_start_ms
            }) {
                return;
            }
        }
        for cid in group {
            if let Some(c) = p.clips.iter_mut().find(|c| c.id == cid) {
                c.cut_start_ms += allowed;
            }
        }
    }

    fn apply_clip_action(&mut self, action: ClipAction) {
        self.push_undo();
        let Some(p) = &mut self.project else { return };
        match action {
            ClipAction::SetIn => p.in_ms = Some(p.playhead_ms),
            ClipAction::SetOut => p.out_ms = Some(p.playhead_ms),
            ClipAction::Clear => {
                p.in_ms = None;
                p.out_ms = None;
            }
            ClipAction::CycleMark => match (p.in_ms, p.out_ms) {
                (None, _) => p.in_ms = Some(p.playhead_ms),
                (Some(_), None) => p.out_ms = Some(p.playhead_ms),
                (Some(_), Some(_)) => {
                    p.in_ms = Some(p.playhead_ms);
                    p.out_ms = None;
                }
            },
            ClipAction::Split => {
                // Build the target set. Selection-aware, but always
                // expand to the full link group(s) so the split keeps
                // V/A pairs in sync — otherwise the unsplit partners
                // still share the old LinkId and a later Delete would
                // cascade across all of them.
                let target_ids: Vec<ClipId> = if p.selected_clips.is_empty() {
                    p.clips
                        .iter()
                        .filter(|c| c.contains_cut(p.playhead_ms))
                        .map(|c| c.id)
                        .collect()
                } else {
                    let mut linked: HashSet<LinkId> = HashSet::new();
                    let mut unlinked: HashSet<ClipId> = HashSet::new();
                    for id in &p.selected_clips {
                        let Some(c) = track::clip_by_id(&p.clips, *id) else {
                            continue;
                        };
                        match c.link {
                            Some(l) => {
                                linked.insert(l);
                            }
                            None => {
                                unlinked.insert(c.id);
                            }
                        }
                    }
                    p.clips
                        .iter()
                        .filter(|c| c.contains_cut(p.playhead_ms))
                        .filter(|c| match c.link {
                            Some(l) => linked.contains(&l),
                            None => unlinked.contains(&c.id),
                        })
                        .map(|c| c.id)
                        .collect()
                };
                let new_ids = p.split_segments(&target_ids, p.playhead_ms);
                if new_ids.is_empty() {
                    self.undo_stack.pop();
                }
            }
            ClipAction::DeleteSelected => {
                // Build the set of target IDs first so we can decide
                // whether the operation will leave the project empty.
                let targets: Vec<ClipId> = if p.selected_clips.is_empty() {
                    p.first_video_track()
                        .and_then(|v| {
                            track::clip_on_track_at(&p.clips, v, p.playhead_ms).map(|c| c.id)
                        })
                        .into_iter()
                        .collect()
                } else {
                    p.selected_clips.iter().copied().collect()
                };
                if targets.is_empty() {
                    self.undo_stack.pop();
                    return;
                }
                // Note: we used to refuse a delete that would empty the
                // project. That made the very first Delete a no-op in the
                // default (everything-linked) state, since every clip
                // shares a single LinkId. Trust Ctrl+Z instead.
                for id in targets {
                    track::delete_with_links(&mut p.clips, id);
                }
                p.text_overlays
                    .retain(|id, _| p.clips.iter().any(|c| c.id == *id));
                p.selected_clips.clear();
                // Playhead stays where it was. Canvas length is preserved
                // via Project::canvas_length_ms, so deleted clips leave a
                // visible hole rather than collapsing the timeline.
            }
            ClipAction::DetachSelected => {
                let targets: Vec<ClipId> = p.selected_clips.iter().copied().collect();
                if targets.is_empty() {
                    self.undo_stack.pop();
                    return;
                }
                let mut any = false;
                for id in targets {
                    if track::detach_links(&mut p.clips, id) {
                        any = true;
                    }
                }
                if !any {
                    self.undo_stack.pop();
                }
            }
        }
    }

    pub fn set_mode(&mut self, new_mode: EditMode) {
        let Some(project) = &mut self.project else {
            return;
        };
        if project.mode == new_mode {
            return;
        }
        match (project.mode, new_mode) {
            (EditMode::SimpleClip, EditMode::MultiTrack) => {
                project.ensure_tracks_initialized();
                // Source-time → cut-time via the V1 clip that contains the
                // source point (initially the whole video, so they match).
                let src = project.playhead_ms;
                let cut = project
                    .first_video_track()
                    .and_then(|v| {
                        project.clips.iter().find(|c| {
                            c.track_id == v && c.src_start_ms <= src && src < c.src_end_ms
                        })
                    })
                    .map(|c| c.cut_start_ms + (src - c.src_start_ms))
                    .unwrap_or(0);
                project.playhead_ms = cut;
                project.selected_clips.clear();
                let seek_src = project
                    .first_video_track()
                    .and_then(|v| track::clip_on_track_at(&project.clips, v, cut))
                    .and_then(|c| c.cut_to_src(cut));
                if let Some(s) = seek_src {
                    let _ = self.player.seek(s);
                }
            }
            (EditMode::MultiTrack, EditMode::SimpleClip) => {
                // Cut-time → source-time via V1 so the user lands on the
                // same picture in the source timeline.
                let cut = project.playhead_ms;
                let src = project
                    .first_video_track()
                    .and_then(|v| track::clip_on_track_at(&project.clips, v, cut))
                    .and_then(|c| c.cut_to_src(cut))
                    .unwrap_or(0);
                project.playhead_ms = src.clamp(0, project.media.duration_ms);
                project.selected_clips.clear();
                let _ = self.player.seek(project.playhead_ms);
            }
            _ => {}
        }
        project.mode = new_mode;
        if new_mode == EditMode::SimpleClip {
            self.layered_preview.reset();
            self.preview_is_composed = false;
            let _ = self.player.set_lavfi_complex("");
            self.last_lavfi_complex.clear();
            let _ = self.player.set_file(
                project.proxy.as_deref().unwrap_or(&project.source),
                project.playhead_ms,
            );
        }
        let _ = self.player.set_paused(true);
    }

    fn toggle_play(&mut self) {
        if let Some(p) = &mut self.project {
            if p.mode == EditMode::MultiTrack && p.needs_composition() {
                if !self.layered_preview.ready {
                    self.status =
                        "Preparing layered preview. Editing and export remain available.".into();
                    return;
                }
                if p.playhead_ms >= p.content_end_ms() {
                    p.playhead_ms = 0;
                    let _ = self.player.seek(0);
                }
                let _ = self.player.toggle_play();
                return;
            }
        }
        if self.project.is_none() {
            return;
        }
        // In Simple Clip mode with both marks set: if we're about to start
        // playing and the playhead is outside [In, Out], snap it to In so
        // playback starts at the clip's beginning.
        let about_to_play = self.player.state().paused;
        if about_to_play {
            if let Some(project) = &mut self.project {
                match project.mode {
                    EditMode::SimpleClip => {
                        if let (Some(in_ms), Some(out_ms)) = (project.in_ms, project.out_ms) {
                            let lo = in_ms.min(out_ms);
                            let hi = in_ms.max(out_ms);
                            if project.playhead_ms < lo || project.playhead_ms >= hi {
                                project.playhead_ms = lo;
                                let _ = self.player.seek(lo);
                            }
                        }
                    }
                    EditMode::MultiTrack => {
                        // If the playhead sits at (or past) the end of the
                        // canvas, wrap back to the start so play actually
                        // plays. Uses the static canvas length, not the
                        // current content extent.
                        let total = project.canvas_length_ms();
                        let v = project.first_video_track();
                        let in_clip = v.is_some_and(|t| {
                            track::clip_on_track_at(&project.clips, t, project.playhead_ms)
                                .is_some()
                        });
                        if project.playhead_ms >= total && total > 0 {
                            project.playhead_ms = 0;
                            let first_src = v.and_then(|t| {
                                project
                                    .clips
                                    .iter()
                                    .filter(|c| c.track_id == t)
                                    .min_by_key(|c| c.cut_start_ms)
                                    .map(|c| c.src_start_ms)
                            });
                            if let Some(s) = first_src {
                                let _ = self.player.seek(s);
                            }
                        } else if !in_clip {
                            // Pre-snap forward when the playhead is in a
                            // gap on V1 (left over from split + delete), so
                            // pressing Play actually plays from real content
                            // instead of fighting tick_multitrack_playback's
                            // gap handler.
                            let snap = v.and_then(|t| {
                                project
                                    .clips
                                    .iter()
                                    .filter(|c| {
                                        c.track_id == t && c.cut_start_ms > project.playhead_ms
                                    })
                                    .min_by_key(|c| c.cut_start_ms)
                                    .copied()
                                    .or_else(|| {
                                        // Nothing after — wrap to the first
                                        // clip so Play means "play from the
                                        // beginning of the cut."
                                        project
                                            .clips
                                            .iter()
                                            .filter(|c| c.track_id == t)
                                            .min_by_key(|c| c.cut_start_ms)
                                            .copied()
                                    })
                            });
                            if let Some(c) = snap {
                                project.playhead_ms = c.cut_start_ms;
                                let _ = self.player.seek(c.src_start_ms);
                            }
                        }
                    }
                }
            }
        }
        if let Err(e) = self.player.toggle_play() {
            self.status = format!("Player error: {e:#}");
            log::error!("toggle_play failed: {e:#}");
        }
    }

    /// The total output duration in ms — used by size-targeted modes
    /// (Discord) to compute the bitrate budget.
    /// - SimpleClip: out - in
    /// - MultiTrack: real output length — gaps kept (they export as
    ///   black), per-clip speed applied (a 2× clip renders half as long)
    fn cut_duration_ms_for_export(&self, project: &Project) -> i64 {
        match project.mode {
            EditMode::SimpleClip => match (project.in_ms, project.out_ms) {
                (Some(a), Some(b)) => (b - a).abs(),
                _ => project.media.duration_ms,
            },
            EditMode::MultiTrack => project.content_end_ms().max(1),
        }
    }

    fn start_export(&mut self) {
        let Some(project) = &self.project else { return };

        let suggested = export::suggest_output(&project.source);
        let Some(destination) = rfd::FileDialog::new()
            .set_file_name(
                suggested
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("clip.mp4"),
            )
            .set_directory(
                suggested
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            )
            .save_file()
        else {
            return;
        };

        let same_file = |source: &std::path::Path| {
            source == destination
                || source
                    .canonicalize()
                    .ok()
                    .zip(destination.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b)
        };
        if same_file(&project.source)
            || project
                .tracks
                .iter()
                .filter_map(|t| t.source.as_ref())
                .any(|s| same_file(&s.path))
        {
            self.status =
                "Choose a different output file — original media cannot be overwritten.".into();
            return;
        }

        // Source dimensions for resolution-preset math.
        let (src_w, src_h) = (project.media.width, project.media.height);
        let cut_duration_ms = self.cut_duration_ms_for_export(project);
        // One plan, shared by both export paths below and computed the
        // same way the Discord stats panel computes it, so what the user
        // was shown is what gets encoded.
        let nitro_plan = encode::NitroPlan::compute(
            cut_duration_ms,
            encode::DiscordNitro::target_bytes_from_mb(self.discord_target_mb),
            src_w,
            src_h,
            project.media.fps,
            self.discord_codec_choice,
            self.detected_gpu,
            &self.available_codecs,
        );

        // Resolve the encode plan based on the user's chosen export mode.
        let resolve_encode = |has_modifications: bool,
                              aligned_copy_possible: bool|
         -> (EncodeMode, Option<String>) {
            match self.export_mode {
                ExportMode::Custom => (
                    EncodeMode::Encode(self.custom_encode.to_encode_settings(src_w, src_h)),
                    Some(format!(
                        "Custom → {} at {}",
                        self.custom_encode.video_codec.display(),
                        self.custom_encode.resolution.label(),
                    )),
                ),
                ExportMode::Discord => (
                    EncodeMode::Encode(nitro_plan.settings.clone()),
                    Some(format!(
                        "Discord → {} CBR at {} (≈ {:.0} MB target).",
                        nitro_plan.codec.display(),
                        nitro_plan.resolution.label(),
                        nitro_plan.est_mb(),
                    )),
                ),
                ExportMode::Auto => {
                    if !has_modifications && aligned_copy_possible {
                        (
                            EncodeMode::Copy,
                            Some("Auto → stream copy (in mark keyframe-aligned).".into()),
                        )
                    } else {
                        (
                            EncodeMode::Encode(EncodeSettings::default_for_source(src_w, src_h)),
                            Some("Auto → re-encode (preserves source quality).".into()),
                        )
                    }
                }
            }
        };

        let (plan, note) = match project.mode {
            EditMode::SimpleClip => {
                let (Some(in_ms), Some(out_ms)) = (project.in_ms, project.out_ms) else {
                    self.status = "Set both an In and Out point before exporting.".into();
                    return;
                };
                let (lo, hi) = (in_ms.min(out_ms), in_ms.max(out_ms));
                // SimpleClip has no modifications other than the cut
                // itself — text/speed/gain don't exist outside MultiTrack.
                // Copy is possible if the In mark is keyframe-aligned.
                let aligned_copy_possible = match self.resolve_smart_cut(&project.source, lo) {
                    (false, _) => true,
                    (true, _) => false,
                };
                let (encode, note) = resolve_encode(false, aligned_copy_possible);
                (
                    ExportPlan::Simple {
                        source: project.source.clone(),
                        destination,
                        source_codec: project.media.video_codec.clone(),
                        src_start_ms: lo,
                        src_end_ms: hi,
                        encode,
                        audio_muted: project.simple_audio_muted,
                    },
                    note,
                )
            }
            EditMode::MultiTrack => {
                if project.clips.is_empty() {
                    self.status = "Nothing to export — the timeline is empty.".into();
                    return;
                }
                // MultiTrack always re-encodes (filter_complex requires
                // decode). Auto/Custom/Discord all route through the
                // filter graph; they just produce different
                // EncodeSettings.
                let settings = match self.export_mode {
                    ExportMode::Custom => self.custom_encode.to_encode_settings(src_w, src_h),
                    ExportMode::Discord => nitro_plan.settings.clone(),
                    ExportMode::Auto => EncodeSettings::default_for_source(src_w, src_h),
                };
                let note = Some(match self.export_mode {
                    ExportMode::Custom => format!(
                        "Custom → {} at {} (multi-track filter graph).",
                        settings.video_codec.display(),
                        self.custom_encode.resolution.label(),
                    ),
                    ExportMode::Discord => format!(
                        "Discord → {} CBR at {} (≈ {:.0} MB target; multi-track \
                         filter graph).",
                        nitro_plan.codec.display(),
                        nitro_plan.resolution.label(),
                        nitro_plan.est_mb(),
                    ),
                    ExportMode::Auto => {
                        "Auto → re-encode (multi-track filter graph; preserves quality).".into()
                    }
                });
                (
                    ExportPlan::Layered {
                        composition: project.composition(false),
                        destination,
                        encode: settings,
                    },
                    note,
                )
            }
        };

        match export::spawn(plan) {
            Ok(handle) => {
                self.export = Some(handle);
                self.status = match note {
                    Some(n) => format!("Export started. {n}"),
                    None => "Export started.".into(),
                };
            }
            Err(e) => {
                self.status = format!("Could not start export: {e:#}");
            }
        }
    }

    /// Smart-cut probe for SimpleClip (single In point). Returns
    /// `(frame_accurate, status_note)`. 50 ms tolerance: PTS drift this
    /// small isn't user-visible.
    fn resolve_smart_cut(&self, source: &std::path::Path, in_ms: i64) -> (bool, Option<String>) {
        const TOLERANCE_MS: i64 = 50;
        if in_ms <= TOLERANCE_MS {
            return (
                false,
                Some("Smart-cut → stream copy (In at file start).".into()),
            );
        }
        match media::nearest_keyframe_before(source, in_ms) {
            Ok(kt) => {
                let drift = in_ms - kt;
                if drift > TOLERANCE_MS {
                    (
                        true,
                        Some(format!(
                            "Smart-cut → re-encode (In is {drift}ms past nearest keyframe)."
                        )),
                    )
                } else {
                    (
                        false,
                        Some("Smart-cut → stream copy (In aligned with keyframe).".into()),
                    )
                }
            }
            Err(e) => {
                log::warn!("smart-cut probe failed: {e:#}; falling back to re-encode");
                (
                    true,
                    Some("Smart-cut → re-encode (keyframe probe failed).".into()),
                )
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> ShortcutActions {
        let mut actions = ShortcutActions::default();
        if self.export.is_some() {
            return actions;
        }
        // While a text input has focus, hand the keys to it — otherwise
        // Backspace/Delete would delete the selected clip while the user
        // is typing a caption, Space would toggle play, etc.
        if ctx.wants_keyboard_input() {
            return actions;
        }
        let Some(project) = &mut self.project else {
            // Even without a project, swallow ctrl+z so it doesn't reach
            // unrelated widgets. With a project, undo is handled below.
            return actions;
        };

        // Edits that need an undo snapshot. We can't call self.push_undo()
        // inside the input closure (already borrowing self.project), so
        // queue intents here and apply them after.
        let mut deferred_action: Option<ClipAction> = None;

        ctx.input(|i| {
            let step = if i.modifiers.shift { 5000 } else { 1000 };
            let max_playhead = match project.mode {
                EditMode::SimpleClip => project.media.duration_ms,
                EditMode::MultiTrack => project.canvas_length_ms(),
            };
            // Ctrl+Z = undo, Ctrl+Shift+Z = redo (Ctrl+Y also redo).
            if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    actions.redo = true;
                } else {
                    actions.undo = true;
                }
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) {
                actions.redo = true;
            }
            match project.mode {
                EditMode::SimpleClip => {
                    if i.key_pressed(egui::Key::I) {
                        deferred_action = Some(ClipAction::SetIn);
                    }
                    if i.key_pressed(egui::Key::O) {
                        deferred_action = Some(ClipAction::SetOut);
                    }
                    // Match the "press S" feel of Split mode: one key
                    // cycles In → Out → reset.
                    if i.key_pressed(egui::Key::S) {
                        deferred_action = Some(ClipAction::CycleMark);
                    }
                }
                EditMode::MultiTrack => {
                    if i.key_pressed(egui::Key::S) {
                        deferred_action = Some(ClipAction::Split);
                    }
                    if i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace) {
                        deferred_action = Some(ClipAction::DeleteSelected);
                    }
                    // D = Detach (only meaningful when a clip is selected
                    // and its link group has more than one member).
                    if i.key_pressed(egui::Key::D) && i.modifiers.is_none() {
                        deferred_action = Some(ClipAction::DetachSelected);
                    }
                }
            }
            if i.key_pressed(egui::Key::J) || i.key_pressed(egui::Key::ArrowLeft) {
                project.playhead_ms = (project.playhead_ms - step).max(0);
            }
            if i.key_pressed(egui::Key::L) || i.key_pressed(egui::Key::ArrowRight) {
                project.playhead_ms = (project.playhead_ms + step).min(max_playhead);
            }
            if i.key_pressed(egui::Key::Space) {
                actions.toggle_play = true;
            }
            if i.key_pressed(egui::Key::M) {
                actions.toggle_mute = true;
            }
        });

        if let Some(action) = deferred_action {
            self.apply_clip_action(action);
        }
        actions
    }
}

impl eframe::App for ClipperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.updater.poll();
        if self.proxy_handle.is_some()
            || matches!(self.updater.state, crate::updater::State::Working(_))
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.tick_proxy();
        self.tick_layered_preview(ctx);
        self.sync_audio_mix();
        self.sync_clip_speed();

        // Pull current player state and use it to drive the playhead while
        // mpv is playing. Local state owns the playhead otherwise.
        let player_state = self.player.state();
        let has_file = player_state.loaded_file.is_some();
        let mpv_playing = has_file && !player_state.paused;

        // While the user is dragging (or within ~300ms of a release) we
        // skip the mpv mirror — otherwise mpv's lagging time-pos events
        // would yank the playhead away from where the user is dragging it.
        let in_seek_cooldown = self
            .seek_cooldown_until
            .map(|t| std::time::Instant::now() < t)
            .unwrap_or(false);
        let user_owns_playhead = self.was_dragging_seek || in_seek_cooldown;

        if mpv_playing && !user_owns_playhead {
            // Collect the inputs we need without holding &mut self.project
            // across player calls (which take &self).
            let mode = self.project.as_ref().map(|p| p.mode);
            match mode {
                Some(EditMode::SimpleClip) => {
                    let project = self.project.as_mut().unwrap();
                    project.playhead_ms = player_state.position_ms;
                    // Stop at Out: pause + seek back to In when playback
                    // crosses the Out mark.
                    if let (Some(in_ms), Some(out_ms)) = (project.in_ms, project.out_ms) {
                        let lo = in_ms.min(out_ms);
                        let hi = in_ms.max(out_ms);
                        if project.playhead_ms >= hi {
                            let _ = self.player.set_paused(true);
                            project.playhead_ms = lo;
                            let _ = self.player.seek(lo);
                        }
                    }
                }
                Some(EditMode::MultiTrack) => {
                    self.tick_multitrack_playback(player_state.position_ms);
                }
                None => {}
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        // Reset per-frame drag flag; show_controls sets it when dragging.
        self.user_dragging_seek = false;
        // Mirror mpv-reported volume to the slider unless the user is
        // actively dragging it.
        if (player_state.volume - self.volume_ui).abs() > 0.5 {
            self.volume_ui = player_state.volume;
        }

        // Record playhead pre-UI so we can detect user-initiated seeks.
        let pre_playhead = self.project.as_ref().map(|p| p.playhead_ms);
        let actions = self.handle_shortcuts(ctx);
        if actions.undo {
            self.undo();
        }
        if actions.redo {
            self.redo();
        }
        if actions.toggle_play {
            self.toggle_play();
        }
        if actions.toggle_mute {
            let _ = self.player.toggle_mute();
        }

        if self.support_open {
            self.show_support(ctx);
        }

        // Top bar
        egui::TopBottomPanel::top("top_bar")
            .frame(
                Frame::none()
                    .fill(theme::BG_HEADER)
                    .inner_margin(Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    egui::menu::menu_button(ui, "File", |ui| {
                        if ui.button("Open Video…").clicked() {
                            ui.close_menu();
                            self.open_file();
                        }
                        ui.separator();
                        if ui.button("Settings…").clicked() {
                            ui.close_menu();
                            self.settings_open = true;
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    if ui
                        .button(
                            if matches!(self.updater.state, crate::updater::State::Available(_)) {
                                "Update available"
                            } else {
                                "Help / Updates"
                            },
                        )
                        .clicked()
                    {
                        self.support_open = true;
                    }
                    ui.separator();
                    ui.label(
                        RichText::new("✂  Halo Clipper")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.separator();

                    // Edit-mode toggle (pill tabs).
                    let mode = self.project.as_ref().map(|p| p.mode);
                    let has_project = self.project.is_some();
                    let mut want_mode: Option<EditMode> = None;
                    if pill_button(
                        ui,
                        "Simple Clip",
                        mode == Some(EditMode::SimpleClip),
                        has_project,
                    )
                    .on_hover_text(
                        "Mark In/Out and export one range. Instant; \
                         single-cut workflow.",
                    )
                    .clicked()
                    {
                        want_mode = Some(EditMode::SimpleClip);
                    }
                    if pill_button(
                        ui,
                        "Advanced",
                        mode == Some(EditMode::MultiTrack),
                        has_project,
                    )
                    .on_hover_text(
                        "Split / Delete + per-stream audio editing. Audio \
                         follows video by default; press Detach (D) on a \
                         clip to expose all source audio streams as their \
                         own lanes.",
                    )
                    .clicked()
                    {
                        want_mode = Some(EditMode::MultiTrack);
                    }
                    if let Some(m) = want_mode {
                        self.set_mode(m);
                    }

                    // (Export-mode picker moved into the Export Options
                    // modal that appears when you click Export.)

                    // (Filename moved to the bottom-left status bar — no
                    // need to show it here too.)
                });
            });

        // Bottom controls + status
        egui::TopBottomPanel::bottom("controls")
            .frame(
                Frame::none()
                    .fill(theme::BG_PANEL)
                    .inner_margin(Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                self.show_controls(ui);
                ui.add_space(6.0);
                self.show_status_bar(ui);
            });

        // Left: media list
        egui::SidePanel::left("media_panel")
            .resizable(true)
            .default_width(240.0)
            .frame(
                Frame::none()
                    .fill(theme::BG_PANEL)
                    .inner_margin(Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                self.show_media_list(ui);
            });

        // Right: Edit panel. Shows whenever a clip is selected in
        // Advanced mode — click a clip to edit it, click empty lane (or
        // switch modes) and it goes away. No open/close ceremony.
        if self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack && !p.selected_clips.is_empty())
        {
            egui::SidePanel::right("edit_panel")
                .resizable(true)
                .default_width(280.0)
                .frame(
                    Frame::none()
                        .fill(theme::BG_PANEL)
                        .inner_margin(Margin::same(12.0)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.show_edit_panel(ui));
                });
        }

        // Central: preview top-right, rest empty (room for future tracks/effects)
        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(theme::BG_CANVAS)
                    .inner_margin(Margin::same(14.0)),
            )
            .show(ctx, |ui| {
                self.show_preview(ui);
            });

        // Export options modal (pre-export). Mutually exclusive with the
        // progress modal — once an export is running, the options modal is
        // forced closed.
        if self.export.is_some() {
            self.export_options_open = false;
        }
        if self.export_options_open {
            self.show_export_options_modal(ctx);
        }

        if self.settings_open {
            self.show_settings_modal(ctx);
        }

        // Export progress modal (during/after).
        if self.export.is_some() {
            self.show_export_modal(ctx);
            ctx.request_repaint_after(Duration::from_millis(80));
        }

        // Decide what kind of seek (if any) to issue this frame.
        //   - Currently dragging   → fast keyframe seek (cheap, snappy)
        //   - Just released drag  → final exact seek + start cooldown
        //   - Click without drag  → exact seek (normal click-to-seek)
        let post_playhead = self.project.as_ref().map(|p| p.playhead_ms);
        let drag_released = self.was_dragging_seek && !self.user_dragging_seek;
        let playhead_changed = matches!(
            (pre_playhead, post_playhead),
            (Some(a), Some(b)) if a != b
        );

        let post_src = self.playhead_to_src();
        if self.user_dragging_seek && playhead_changed {
            if let Some(src) = post_src {
                let _ = self.player.seek_fast(src);
            }
        } else if drag_released {
            if let Some(src) = post_src {
                let _ = self.player.seek(src);
            }
            self.seek_cooldown_until = Some(std::time::Instant::now() + Duration::from_millis(300));
        } else if playhead_changed {
            if let Some(src) = post_src {
                let _ = self.player.seek(src);
            }
        }
        let _ = post_playhead;

        // While the user is dragging the scrubber or timeline, force a
        // continuous repaint. egui's default is "repaint only on input
        // events", which means if the cursor pauses mid-drag the GL
        // paint callback also stops firing — even though mpv has been
        // seeking in the background and has a fresher frame ready. The
        // unconditional repaint here is what makes scrubbing feel like
        // a native video player instead of a slideshow.
        if self.user_dragging_seek {
            ctx.request_repaint();
        }

        self.was_dragging_seek = self.user_dragging_seek;
    }

    /// Final shutdown hook. eframe calls this once the user has closed
    /// the window. If the user opted in, wipe the scrub-proxy cache so
    /// the next launch starts with a fresh disk footprint. We also
    /// cancel any in-flight transcode so the ffmpeg child doesn't keep
    /// running after our process exits.
    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.layered_preview.reset();
        if let Some(h) = self.proxy_handle.take() {
            h.cancel();
        }
        if self.settings.auto_clear_cache_on_close {
            match proxy::clear_cache() {
                Ok(freed) if freed > 0 => {
                    log::info!("auto-clear: freed {} from proxy cache", format_bytes(freed))
                }
                Ok(_) => log::info!("auto-clear: nothing to remove"),
                Err(e) => log::warn!("auto-clear failed: {e}"),
            }
        }
    }
}

impl ClipperApp {
    /// Right-side Edit panel. Appears automatically while a clip is
    /// selected in Advanced mode and reflects that clip. Settings act on
    /// the audio TRACK that owns the clip (or every linked audio partner
    /// track if a video clip is selected), so changes are immediately
    /// audible in preview and consistent between collapsed and detached
    /// states. The ✕ deselects, which hides the panel — clicking empty
    /// timeline does the same.
    fn show_edit_panel(&mut self, ui: &mut egui::Ui) {
        let mut want_close = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Edit")
                    .size(17.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui
                    .add(
                        egui::Button::new(RichText::new("✕").size(14.0).color(theme::TEXT_MUTED))
                            .fill(Color32::TRANSPARENT)
                            .rounding(Rounding::same(4.0))
                            .min_size(egui::vec2(28.0, 28.0)),
                    )
                    .on_hover_text("Deselect")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if close.clicked() {
                    want_close = true;
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(10.0);

        self.show_layer_properties(ui);

        // Snapshot all data we need before touching `self.project` mutably.
        // The intermediate block scopes the borrow so the writes later
        // (push_undo, track / clip mutation) don't conflict.
        struct Snapshot {
            clip_id: ClipId,
            track_kind: Option<TrackKind>,
            track_name: String,
            cut_start: i64,
            cut_end: i64,
            src_start: i64,
            src_end: i64,
            /// Which audio CLIPS this Edit affects. Gain is per-clip so
            /// one loud segment can be pulled down without touching the
            /// rest of the track. For an audio clip: just itself. For a
            /// linked video clip: every audio partner clip in that link
            /// group. For an unlinked video clip: none.
            audio_clip_ids: Vec<ClipId>,
            initial_gain: f32,
            /// Every clip whose speed should change together — the
            /// selected clip + every link partner. Speed must match
            /// across V/A or A/V sync drifts.
            speed_target_clip_ids: Vec<ClipId>,
            initial_speed: f32,
        }
        let snap: Option<Snapshot> = 'snap: {
            let Some(project) = self.project.as_ref() else {
                break 'snap None;
            };
            let Some(&clip_id) = project.selected_clips.iter().min() else {
                break 'snap None;
            };
            let Some(clip) = track::clip_by_id(&project.clips, clip_id) else {
                break 'snap None;
            };
            let track = project.tracks.iter().find(|t| t.id == clip.track_id);
            let kind = track.map(|t| t.kind);

            let audio_clip_ids: Vec<ClipId> = match kind {
                Some(TrackKind::Audio) => vec![clip_id],
                Some(TrackKind::Video) => match clip.link {
                    // Only the audio partners of THIS link group — i.e.
                    // this segment's audio, not the whole track.
                    Some(link) => project
                        .clips
                        .iter()
                        .filter(|c| c.link == Some(link))
                        .filter(|c| {
                            project
                                .tracks
                                .iter()
                                .any(|t| t.id == c.track_id && t.kind == TrackKind::Audio)
                        })
                        .map(|c| c.id)
                        .collect(),
                    None => Vec::new(),
                },
                None => Vec::new(),
            };
            let initial_gain = audio_clip_ids
                .first()
                .and_then(|cid| track::clip_by_id(&project.clips, *cid))
                .map(|c| c.gain)
                .unwrap_or(1.0);
            // Speed applies to clip + link partners (so V/A stay synced).
            let speed_target_clip_ids: Vec<ClipId> = match clip.link {
                Some(link) => project
                    .clips
                    .iter()
                    .filter(|c| c.link == Some(link))
                    .map(|c| c.id)
                    .collect(),
                None => vec![clip_id],
            };
            Some(Snapshot {
                clip_id,
                track_kind: kind,
                track_name: track.map(|t| t.name.clone()).unwrap_or_default(),
                cut_start: clip.cut_start_ms,
                cut_end: clip.cut_end_ms(),
                src_start: clip.src_start_ms,
                src_end: clip.src_end_ms,
                audio_clip_ids,
                initial_gain,
                speed_target_clip_ids,
                initial_speed: clip.speed,
            })
        };

        let Some(snap) = snap else {
            // Shouldn't happen (the panel only renders with a selection),
            // but stay graceful if the selected clip vanished mid-frame.
            ui.label(RichText::new("Select a clip on the timeline.").color(theme::TEXT_MUTED));
            return;
        };

        // Info block — what this Edit panel is editing.
        ui.label(
            RichText::new(format!(
                "{} clip on {}",
                match snap.track_kind {
                    Some(TrackKind::Video) => "Video",
                    Some(TrackKind::Audio) => "Audio",
                    None => "Unknown",
                },
                snap.track_name,
            ))
            .size(14.0)
            .color(theme::TEXT_PRIMARY)
            .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Position  {} → {}",
                media::format_timestamp(snap.cut_start),
                media::format_timestamp(snap.cut_end),
            ))
            .size(12.0)
            .color(theme::TEXT_MUTED)
            .monospace(),
        );
        ui.label(
            RichText::new(format!(
                "Source    {} → {}",
                media::format_timestamp(snap.src_start),
                media::format_timestamp(snap.src_end),
            ))
            .size(12.0)
            .color(theme::TEXT_MUTED)
            .monospace(),
        );

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(14.0);

        // ─────────── Audio gain ───────────
        if !snap.audio_clip_ids.is_empty() {
            ui.label(
                RichText::new("Audio")
                    .size(13.5)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(6.0);
            let mut gain = snap.initial_gain;
            let resp = ui.add(
                egui::Slider::new(&mut gain, 0.0..=2.0)
                    .step_by(0.01)
                    .text("Gain"),
            );
            if resp.drag_started() {
                self.push_undo();
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (preset, label) in [
                    (0.0_f32, "Mute"),
                    (0.5, "-6 dB"),
                    (1.0, "1×"),
                    (1.5, "+3.5 dB"),
                ] {
                    let active = (gain - preset).abs() < 0.005;
                    if speed_preset_chip(ui, label, active).clicked() {
                        self.push_undo();
                        gain = preset;
                    }
                }
            });
            if (gain - snap.initial_gain).abs() > 0.0001 {
                if let Some(p) = self.project.as_mut() {
                    for cid in &snap.audio_clip_ids {
                        if let Some(c) = p.clips.iter_mut().find(|c| c.id == *cid) {
                            c.gain = gain.clamp(0.0, 2.0);
                        }
                    }
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "0 = silent · 1 = original · 2 = +6 dB. This segment only — \
                 split the clip to make one part quieter. Live in preview + export.",
                )
                .size(11.5)
                .color(theme::TEXT_MUTED),
            );
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(14.0);
        }

        // ─────────── Speed ───────────
        ui.label(
            RichText::new("Speed")
                .size(13.5)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(6.0);
        let mut speed = snap.initial_speed;
        let speed_resp = ui.add(
            egui::Slider::new(&mut speed, 0.25..=4.0)
                .step_by(0.05)
                .text(if snap.speed_target_clip_ids.len() > 1 {
                    "× (all linked)"
                } else {
                    "×"
                }),
        );
        if speed_resp.drag_started() {
            self.push_undo();
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (preset, label) in [(0.5_f32, "0.5×"), (1.0, "1×"), (2.0, "2×"), (4.0, "4×")] {
                let active = (speed - preset).abs() < 0.001;
                if speed_preset_chip(ui, label, active).clicked() {
                    self.push_undo();
                    speed = preset;
                }
            }
        });
        if (speed - snap.initial_speed).abs() > 0.0001 {
            if let Some(p) = self.project.as_mut() {
                for cid in &snap.speed_target_clip_ids {
                    if let Some(c) = p.clips.iter_mut().find(|c| c.id == *cid) {
                        c.speed = speed.clamp(0.25, 4.0);
                    }
                }
            }
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new("Linked V+A move together. Live in preview + export.")
                .size(11.5)
                .color(theme::TEXT_MUTED),
        );

        // ─────────── Text overlay (per-segment, V tracks only) ───────────
        if snap.track_kind == Some(TrackKind::Video) {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.label(
                RichText::new("Text overlay")
                    .color(theme::TEXT_MUTED)
                    .small(),
            );

            // Caption settings belong only to the video segment being edited.
            let clip_id_for_text = snap.clip_id;
            let mut overlay_state = self
                .project
                .as_ref()
                .and_then(|p| p.text_overlays.get(&clip_id_for_text).cloned());
            let mut enabled = overlay_state.is_some();
            let prev_enabled = enabled;

            ui.checkbox(&mut enabled, "Burn caption into this segment");
            if enabled != prev_enabled {
                self.push_undo();
                if let Some(p) = self.project.as_mut() {
                    if enabled {
                        p.text_overlays
                            .insert(clip_id_for_text, TextOverlay::default());
                    } else {
                        p.text_overlays.remove(&clip_id_for_text);
                    }
                }
                overlay_state = self
                    .project
                    .as_ref()
                    .and_then(|p| p.text_overlays.get(&clip_id_for_text).cloned());
            }

            if let Some(mut overlay) = overlay_state {
                let original = overlay.clone();
                ui.add_space(4.0);
                let text_resp = ui.add(
                    egui::TextEdit::singleline(&mut overlay.text)
                        .hint_text("Caption…")
                        .desired_width(f32::INFINITY),
                );
                if text_resp.gained_focus() {
                    self.push_undo();
                }

                let x_resp = ui.add(
                    egui::Slider::new(&mut overlay.x, 0.0..=1.0)
                        .step_by(0.01)
                        .text("Horiz"),
                );
                if x_resp.drag_started() {
                    self.push_undo();
                }
                let y_resp = ui.add(
                    egui::Slider::new(&mut overlay.y, 0.0..=1.0)
                        .step_by(0.01)
                        .text("Vert"),
                );
                if y_resp.drag_started() {
                    self.push_undo();
                }
                let size_resp =
                    ui.add(egui::Slider::new(&mut overlay.font_size_px, 12..=500).text("Size"));
                if size_resp.drag_started() {
                    self.push_undo();
                }

                // Font family/style dropdown.
                let mut font_choice = overlay.font.clone();
                let prev_font = font_choice.clone();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Font").color(theme::TEXT_MUTED).small());
                    egui::ComboBox::from_id_salt("text_overlay_font")
                        .selected_text(&font_choice)
                        .show_ui(ui, |ui| {
                            for option in track::FONT_CHOICES {
                                ui.selectable_value(
                                    &mut font_choice,
                                    option.display.into(),
                                    option.display,
                                );
                            }
                        });
                });
                if font_choice != prev_font {
                    self.push_undo();
                    overlay.font = font_choice;
                }

                let mut color = overlay.color;
                let color_resp = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new("Color").color(theme::TEXT_MUTED).small());
                        ui.color_edit_button_srgb(&mut color)
                    })
                    .inner;
                if color_resp.changed() {
                    self.push_undo();
                    overlay.color = color;
                }

                // Typography extras — outline, shadow, opacity. All baked
                // into the export via drawtext; the preview approximates
                // them (see show_text_overlay_handle).
                let outline_resp =
                    ui.add(egui::Slider::new(&mut overlay.outline_px, 0..=12).text("Outline"));
                if outline_resp.drag_started() {
                    self.push_undo();
                }
                let opacity_resp = ui.add(
                    egui::Slider::new(&mut overlay.opacity, 0.1..=1.0)
                        .step_by(0.05)
                        .text("Opacity"),
                );
                if opacity_resp.drag_started() {
                    self.push_undo();
                }
                let mut shadow = overlay.shadow;
                if ui.checkbox(&mut shadow, "Drop shadow").changed() {
                    self.push_undo();
                    overlay.shadow = shadow;
                }

                ui.label(RichText::new("Applies to this segment only. Use Horiz / Vert to position; layered previews rebuild after edits.")
                    .small().color(theme::TEXT_MUTED.linear_multiply(0.7)));

                // Write back if anything actually changed.
                if overlay != original {
                    if let Some(p) = self.project.as_mut() {
                        p.text_overlays.insert(clip_id_for_text, overlay);
                    }
                }
            }
        }

        if want_close {
            if let Some(p) = self.project.as_mut() {
                p.selected_clips.clear();
            }
        }
    }

    fn show_media_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Media")
                    .size(14.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+").on_hover_text("Add media…").clicked() {
                    if self
                        .project
                        .as_ref()
                        .is_some_and(|p| p.mode == EditMode::MultiTrack)
                    {
                        self.import_layer();
                    } else {
                        self.open_file();
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        self.show_layers(ui);

        if let Some(project) = &self.project {
            let name = project
                .source
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            // Single-item "active" list for now. When we support stacking,
            // this becomes a Vec rendered as a list with drag handles.
            Frame::none()
                .fill(theme::BG_BUTTON_HOVER)
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::symmetric(10.0, 8.0))
                .stroke(Stroke::new(1.0_f32, theme::ACCENT.linear_multiply(0.6)))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(name).strong().color(theme::TEXT_PRIMARY));
                        ui.label(
                            RichText::new(format!(
                                "{}×{}  ·  {:.0}fps  ·  {}",
                                project.media.width,
                                project.media.height,
                                project.media.fps,
                                media::format_timestamp(project.media.duration_ms),
                            ))
                            .small()
                            .color(theme::TEXT_MUTED),
                        );
                    });
                });
        } else {
            ui.label(RichText::new("No media yet.").color(theme::TEXT_MUTED));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Use File → Open Video, or the “+” above.")
                    .small()
                    .color(theme::TEXT_MUTED.linear_multiply(0.7)),
            );
        }
    }

    /// Live caption overlay on top of the preview. We paint the text in
    /// egui (matching the user's font/size/color choices) and keep a
    /// draggable hit area around it. Because there's no ffmpeg filter
    /// involved here, drag is just per-frame repaint — fast and smooth
    /// regardless of how big the font gets. The same overlay struct is
    /// then baked into the export by ffmpeg's drawtext at render time.
    fn show_text_overlay_handle(
        &mut self,
        ui: &mut egui::Ui,
        preview_rect: &egui::Rect,
        clip_id: ClipId,
        overlay: &TextOverlay,
    ) {
        // font_size_px is in SOURCE pixels; scale by preview height / source
        // height so the on-screen size matches what export will produce.
        let source_h = self
            .project
            .as_ref()
            .map(|p| p.media.height.max(1) as f32)
            .unwrap_or(720.0);
        let font_screen = (overlay.font_size_px as f32 * preview_rect.height() / source_h).max(6.0);

        // Pick the egui family that matches the user's font choice. We
        // try the loaded DejaVu variant first; if it wasn't found at
        // startup, egui falls back to its built-in Proportional family.
        let choice = track::font_choice(&overlay.font);
        let family = if choice.display == "Mono" {
            egui::FontFamily::Monospace
        } else {
            egui::FontFamily::Name(choice.egui_family.into())
        };
        let font_id = egui::FontId::new(font_screen, family);

        let cx = preview_rect.left() + overlay.x.clamp(0.0, 1.0) * preview_rect.width();
        let cy = preview_rect.top() + overlay.y.clamp(0.0, 1.0) * preview_rect.height();
        let opacity = overlay.opacity.clamp(0.1, 1.0);
        let alpha = (opacity * 255.0) as u8;
        let color = Color32::from_rgba_unmultiplied(
            overlay.color[0],
            overlay.color[1],
            overlay.color[2],
            alpha,
        );
        // Preview approximations of the export-side drawtext layers. The
        // pixel scale matches the font scaling above so proportions hold.
        let px = preview_rect.height() / source_h;

        let galley = ui
            .painter()
            .layout_no_wrap(overlay.text.clone(), font_id.clone(), color);
        let text_size = galley.size();
        let text_pos = egui::pos2(cx - text_size.x * 0.5, cy - text_size.y * 0.5);

        // Drop shadow: one offset black pass under everything.
        if overlay.shadow {
            let off = (2.0 * px).max(1.0);
            let shadow_color =
                Color32::from_rgba_unmultiplied(0, 0, 0, (0.6 * opacity * 255.0) as u8);
            let g =
                ui.painter()
                    .layout_no_wrap(overlay.text.clone(), font_id.clone(), shadow_color);
            ui.painter()
                .galley(text_pos + egui::vec2(off, off), g, shadow_color);
        }
        // Outline: eight offset black passes ringing the fill. Cruder
        // than drawtext's real border but reads the same at preview size.
        if overlay.outline_px > 0 {
            let r = (overlay.outline_px as f32 * px).max(1.0);
            let outline_color = Color32::from_rgba_unmultiplied(0, 0, 0, alpha);
            for (dx, dy) in [
                (-r, 0.0),
                (r, 0.0),
                (0.0, -r),
                (0.0, r),
                (-r, -r),
                (r, -r),
                (-r, r),
                (r, r),
            ] {
                let g = ui.painter().layout_no_wrap(
                    overlay.text.clone(),
                    font_id.clone(),
                    outline_color,
                );
                ui.painter()
                    .galley(text_pos + egui::vec2(dx, dy), g, outline_color);
            }
        }
        ui.painter().galley(text_pos, galley, color);

        // Drag-hit area: a touch larger than the rendered text so it's
        // easy to grab even with tiny captions.
        let handle = egui::Rect::from_center_size(
            egui::pos2(cx, cy),
            egui::vec2(
                (text_size.x + 16.0).max(40.0),
                (text_size.y + 8.0).max(20.0),
            ),
        );
        let resp = ui.interact(
            handle,
            egui::Id::new(("text_handle", clip_id)),
            egui::Sense::click_and_drag(),
        );
        if resp.hovered() || resp.dragged() {
            let stroke = if resp.dragged() {
                egui::Stroke::new(2.0_f32, theme::ACCENT)
            } else {
                egui::Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(150, 200, 255, 180))
            };
            ui.painter().rect_stroke(handle, 4.0, stroke);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }

        if resp.drag_started() {
            self.push_undo();
        }
        if resp.dragged() {
            let delta = resp.drag_delta();
            if delta != egui::Vec2::ZERO {
                let dx = delta.x / preview_rect.width().max(1.0);
                let dy = delta.y / preview_rect.height().max(1.0);
                if let Some(p) = self.project.as_mut() {
                    if let Some(o) = p.text_overlays.get_mut(&clip_id) {
                        o.x = (o.x + dx).clamp(0.0, 1.0);
                        o.y = (o.y + dy).clamp(0.0, 1.0);
                    }
                }
            }
        }
    }

    fn show_preview(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        let aspect = match self.player.state().video_size {
            Some((w, h)) if w > 0 && h > 0 => w as f32 / h as f32,
            _ => 16.0 / 9.0,
        };

        // Reserve space at the bottom of the central panel for the Pop
        // Out + Volume controls row that sits beneath the preview. Without
        // this, an aspect-locked preview can grow to the full panel height
        // and push the controls row off-screen (the bug visible in image 6).
        let controls_h = 36.0_f32;
        let controls_gap = 6.0_f32;
        let reserved_below = controls_h + controls_gap + 2.0;
        let avail_h_for_preview = (avail.height() - reserved_below).max(80.0);

        // Largest preview that fits in (avail.width, avail_h_for_preview),
        // locked to source aspect.
        let max_w_by_height = avail_h_for_preview * aspect;
        let default_w = avail.width().min(max_w_by_height);
        let current_w = self.preview_width.unwrap_or(default_w);
        let max_w = avail.width().min(max_w_by_height).max(1.0);
        let pw = current_w.clamp(280.0_f32.min(max_w), max_w);
        let ph = pw / aspect;

        // Center the preview horizontally in the available width — keeps
        // empty space balanced on both sides when the panel is wider than
        // the aspect-locked frame.
        let preview_x = avail.center().x - pw * 0.5;
        let preview_rect =
            egui::Rect::from_min_size(egui::pos2(preview_x, avail.top()), egui::vec2(pw, ph));

        // Render preview frame
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(preview_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        Frame::none()
            .fill(theme::BG_PREVIEW)
            .rounding(Rounding::same(8.0))
            .stroke(Stroke::new(1.0_f32, theme::BORDER))
            .show(&mut child, |ui| {
                ui.set_min_size(preview_rect.size());
                ui.set_max_size(preview_rect.size());
                if self.project.is_none() {
                    ui.allocate_ui_with_layout(
                        preview_rect.size(),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.label(
                                RichText::new("No media")
                                    .color(theme::TEXT_MUTED)
                                    .size(13.0),
                            );
                        },
                    );
                    return;
                }
                let (rect, _) = ui.allocate_exact_size(preview_rect.size(), egui::Sense::hover());
                // In Advanced mode, the playhead can sit in a gap between
                // clips on V1 (e.g. after split + delete). The export bakes
                // those gaps as black, so the preview matches by painting
                // black instead of leaving mpv's stale frame on screen.
                let in_v1_gap = self.project.as_ref().is_some_and(|p| {
                    p.mode == EditMode::MultiTrack
                        && !p.needs_composition()
                        && p.first_video_track().is_some_and(|v| {
                            track::clip_on_track_at(&p.clips, v, p.playhead_ms).is_none()
                        })
                });
                if in_v1_gap {
                    ui.painter()
                        .rect_filled(rect, Rounding::same(0.0), Color32::BLACK);
                } else {
                    let player = self.player.clone();
                    let video_fbo = self.video_fbo.clone();
                    let callback = egui::PaintCallback {
                        rect,
                        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                            paint_mpv(&player, &video_fbo, info, painter);
                        })),
                    };
                    ui.painter().add(callback);
                }
            });

        if self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack && p.needs_composition())
            && !self.layered_preview.ready
        {
            ui.painter()
                .rect_filled(preview_rect, 8.0, Color32::from_black_alpha(220));
            ui.painter().text(
                preview_rect.center(),
                Align2::CENTER_CENTER,
                "Updating layered preview…",
                egui::FontId::proportional(16.0),
                theme::TEXT_PRIMARY,
            );
            ui.label(
                RichText::new(&self.layered_preview.message)
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            if ui.button("Retry preview").clicked() {
                self.layered_preview.retry();
            }
        }

        // Draw only the caption on the video segment under the playhead.
        let active_caption = self
            .project
            .as_ref()
            .and_then(Project::active_caption)
            .map(|(id, overlay)| (id, overlay.clone()));
        if let Some((clip_id, overlay)) =
            active_caption.filter(|_| !self.project.as_ref().is_some_and(|p| p.needs_composition()))
        {
            self.show_text_overlay_handle(ui, &preview_rect, clip_id, &overlay);
        }

        // Preview-local controls row directly under the preview frame:
        // pop out + volume. These only affect playback, not editing, so
        // they live with the preview rather than in the bottom toolbar.
        let player_state = self.player.state();
        let controls_rect = egui::Rect::from_min_size(
            egui::pos2(preview_rect.left(), preview_rect.bottom() + controls_gap),
            egui::vec2(pw, controls_h),
        );
        let mut controls_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(controls_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        controls_ui.set_min_size(controls_rect.size());

        let mut want_popout = false;
        let mut want_mute = false;
        let mut new_volume: Option<f64> = None;
        controls_ui.horizontal(|ui| {
            if secondary_button(ui, "⧉  Pop Out", true)
                .on_hover_text(
                    "Open this video in a separate mpv window\n\
                                (the embedded preview keeps working)",
                )
                .clicked()
            {
                want_popout = true;
            }
            ui.separator();
            let mute_label = if player_state.muted { "🔇" } else { "🔊" };
            if ui.button(mute_label).on_hover_text("Mute (M)").clicked() {
                want_mute = true;
            }
            let vol_response = ui
                .scope(|ui| {
                    ui.spacing_mut().slider_width = 160.0;
                    ui.add(egui::Slider::new(&mut self.volume_ui, 0.0..=130.0).show_value(false))
                })
                .inner;
            if vol_response.changed() {
                new_volume = Some(self.volume_ui);
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{:.0}", self.volume_ui))
                    .monospace()
                    .color(theme::TEXT_MUTED)
                    .small(),
            );
        });
        if want_popout {
            if let Some(project) = &self.project {
                let start = project.playhead_ms as f64 / 1000.0;
                let _ = crate::platform::command("mpv")
                    .arg(format!("--start={start:.3}"))
                    .arg(&project.source)
                    .spawn();
            }
        }
        if want_mute {
            let _ = self.player.toggle_mute();
        }
        if let Some(v) = new_volume {
            let _ = self.player.set_volume(v);
        }

        // Resize grip at the bottom-left corner. Drag down/left to grow,
        // up/right to shrink. Aspect-ratio locked via width.
        let grip_size = 22.0;
        let grip_rect = egui::Rect::from_min_size(
            egui::pos2(preview_rect.left(), preview_rect.bottom() - grip_size),
            egui::vec2(grip_size, grip_size),
        );
        let grip_resp = ui.interact(
            grip_rect,
            egui::Id::new("preview_resize_grip"),
            egui::Sense::drag(),
        );
        if grip_resp.dragged() {
            let delta = grip_resp.drag_delta();
            // Down (delta.y > 0) and Left (delta.x < 0) both grow.
            let dw = delta.y - delta.x;
            // Same height-for-preview cap used above — keeps the resize
            // grip from growing the preview into the controls row.
            self.preview_width = Some(
                (self.preview_width.unwrap_or(default_w) + dw).clamp(280.0_f32.min(max_w), max_w),
            );
        }
        // Draw the grip glyph (three diagonal lines, bottom-left corner)
        let grip_painter = ui.painter_at(grip_rect);
        let grip_color = if grip_resp.hovered() || grip_resp.dragged() {
            theme::ACCENT
        } else {
            theme::TEXT_MUTED.linear_multiply(0.7)
        };
        for i in 0..3 {
            let off = 5.0 + (i as f32) * 4.0;
            grip_painter.line_segment(
                [
                    egui::pos2(grip_rect.left() + 3.0, grip_rect.bottom() - 3.0 - off),
                    egui::pos2(grip_rect.left() + 3.0 + off, grip_rect.bottom() - 3.0),
                ],
                Stroke::new(1.5_f32, grip_color),
            );
        }
        if grip_resp.hovered() || grip_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeSouthWest);
        }
    }

    fn show_controls(&mut self, ui: &mut egui::Ui) {
        let mut want_open = false;
        let mut want_play = false;
        let mut want_export = false;
        let mut dragging_any = false;
        let mut clip_action: Option<ClipAction> = None;
        let player_state = self.player.state();
        let is_playing = player_state.loaded_file.is_some() && !player_state.paused;

        let mut want_mute_track: Option<TrackId> = None;
        let mut want_toggle_simple_mute = false;
        let mut want_toggle_linked_audio = false;
        // Pending selection change from the timeline widget. None outer
        // means "no change this frame"; Some(SelectionChange::*) describes
        // what to do.
        let mut want_selection: Option<SelectionChange> = None;
        let mut clip_drag_started: Option<(ClipId, f32)> = None;
        let mut clip_drag_delta: Option<(ClipId, i64)> = None;
        let mut clip_drag_ended: bool = false;
        if let Some(project) = &mut self.project {
            let mode = project.mode;
            // Display length stays put when clips are deleted — the canvas
            // is static so the user sees gaps instead of a collapsing rail.
            let cut_duration = project.canvas_length_ms();
            let scrubber_duration = match mode {
                EditMode::SimpleClip => project.media.duration_ms,
                EditMode::MultiTrack => cut_duration,
            };
            let mut tl_state = TimelineState {
                duration_ms: scrubber_duration,
                playhead_ms: project.playhead_ms,
                in_ms: if mode == EditMode::SimpleClip {
                    project.in_ms
                } else {
                    None
                },
                out_ms: if mode == EditMode::SimpleClip {
                    project.out_ms
                } else {
                    None
                },
                audio_muted: project.simple_audio_muted,
            };

            // Playhead readout above the timeline — large monospace
            // figures so the user can read the current frame at a
            // glance. For Advanced mode the total tracks the rightmost
            // clip-end (live), so editing reshapes the "/ 12:34" half
            // even though the timeline canvas itself stays put.
            let display_total = match mode {
                EditMode::SimpleClip => project.media.duration_ms,
                EditMode::MultiTrack => project.content_end_ms(),
            };
            show_playhead_readout(ui, project.playhead_ms, display_total);
            ui.add_space(6.0);

            // Scrubber row: Play button + slim progress bar
            ui.horizontal(|ui| {
                let play_label = if is_playing { "⏸" } else { "▶" };
                let play_btn = egui::Button::new(RichText::new(play_label).size(18.0).strong())
                    .fill(theme::ACCENT)
                    .min_size(egui::vec2(46.0, 30.0));
                if ui.add(play_btn).on_hover_text("Space").clicked() {
                    want_play = true;
                }
                let scrubber_out = timeline::scrubber(ui, &mut tl_state);
                dragging_any |= scrubber_out.dragging;
            });

            ui.add_space(4.0);

            // Main timeline — different widget per mode.
            match mode {
                EditMode::SimpleClip => {
                    let (_, tl_out) = timeline::show(ui, &mut tl_state);
                    dragging_any |= tl_out.dragging;
                    project.playhead_ms = tl_state.playhead_ms;
                    if tl_out.mute_toggled {
                        want_toggle_simple_mute = true;
                    }
                }
                EditMode::MultiTrack => {
                    project.playhead_ms = tl_state.playhead_ms;
                    let mut mt_state = timeline::MultiTrackTimelineState {
                        tracks: &project.tracks,
                        clips: &project.clips,
                        playhead_cut_ms: project.playhead_ms,
                        cut_duration_ms: cut_duration,
                        selected_clips: &project.selected_clips,
                    };
                    let (_, mt_out) = egui::ScrollArea::vertical()
                        .id_salt("layer_timeline")
                        .max_height(220.0)
                        .show(ui, |ui| timeline::show_multitrack(ui, &mut mt_state))
                        .inner;
                    dragging_any |= mt_out.dragging;
                    project.playhead_ms = mt_state.playhead_cut_ms;
                    want_mute_track = mt_out.mute_toggled;
                    if let Some(c) = mt_out.clicked_clip {
                        want_selection = Some(if c.additive {
                            SelectionChange::Toggle(c.id)
                        } else {
                            SelectionChange::ReplaceWith(c.id)
                        });
                    } else if mt_out.clicked_empty {
                        want_selection = Some(SelectionChange::Clear);
                    }
                    clip_drag_started = mt_out.clip_drag_started;
                    clip_drag_delta = mt_out.clip_drag_delta;
                    clip_drag_ended = mt_out.clip_drag_ended;
                    if mt_out.linked_audio_mute_toggled {
                        want_toggle_linked_audio = true;
                    }
                }
            }

            ui.add_space(8.0);

            // Info row — different stats per mode.
            match mode {
                EditMode::SimpleClip => {
                    show_simpleclip_info(ui, project);
                }
                EditMode::MultiTrack => {
                    show_multitrack_info(ui, project, cut_duration);
                }
            }

            ui.add_space(8.0);

            // Action row.
            ui.horizontal(|ui| match mode {
                EditMode::SimpleClip => {
                    if accent_button(ui, "Set In  (I)", theme::ACCENT_IN, true).clicked() {
                        clip_action = Some(ClipAction::SetIn);
                    }
                    if accent_button(ui, "Set Out  (O)", theme::ACCENT_OUT, true).clicked() {
                        clip_action = Some(ClipAction::SetOut);
                    }
                    if secondary_button(ui, "Clear", true).clicked() {
                        clip_action = Some(ClipAction::Clear);
                    }

                    let can_export = project.in_ms.is_some()
                        && project.out_ms.is_some()
                        && self.export.is_none();

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Export Clip…", can_export).clicked() {
                            want_export = true;
                        }
                    });
                }
                EditMode::MultiTrack => {
                    let can_split = !project.clips.is_empty()
                        && project.playhead_ms > 0
                        && project.playhead_ms < cut_duration;
                    if accent_button(ui, "Split  (S)", theme::ACCENT_OUT, can_split)
                        .on_hover_text("Split every clip the playhead is on")
                        .clicked()
                    {
                        clip_action = Some(ClipAction::Split);
                    }
                    let can_delete = !project.clips.is_empty()
                        && (!project.selected_clips.is_empty()
                            || project.first_video_track().is_some());
                    if accent_button(ui, "Delete  (Del)", theme::ACCENT_ERR, can_delete)
                        .on_hover_text("Delete the selected clip(s) + linked partners")
                        .clicked()
                    {
                        clip_action = Some(ClipAction::DeleteSelected);
                    }
                    let can_detach = project.selected_clips.iter().any(|id| {
                        track::clip_by_id(&project.clips, *id).is_some_and(|c| c.link.is_some())
                    });
                    if secondary_button(ui, "Detach Audio  (D)", can_detach)
                        .on_hover_text("Detach the selected clip(s) — exposes their audio lanes")
                        .clicked()
                    {
                        clip_action = Some(ClipAction::DetachSelected);
                    }

                    let can_export = !project.clips.is_empty() && self.export.is_none();

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Export Cut…", can_export).clicked() {
                            want_export = true;
                        }
                    });
                }
            });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                if primary_button(ui, "Open Video…", true).clicked() {
                    want_open = true;
                }
                ui.add_space(8.0);
            });
        }

        if want_open {
            self.open_file();
        }
        if want_play {
            self.toggle_play();
        }
        if want_export {
            self.export_options_open = true;
        }
        if let Some(action) = clip_action {
            self.apply_clip_action(action);
        }
        // Selection change from the multi-track widget (a click on a clip
        // selects it; a click on bare lane clears the selection).
        // Clip drag — push_undo + open a session on the frame the drag
        // begins, apply per-frame deltas through the snap-aware handler,
        // then clear the session on drag end.
        if let Some((id, cut_per_px)) = clip_drag_started {
            self.push_undo();
            if let Some(p) = self.project.as_ref() {
                if let Some(primary) = track::clip_by_id(&p.clips, id) {
                    self.clip_drag_session = Some(ClipDragSession {
                        primary: id,
                        cut_per_px,
                        intended_cut: primary.cut_start_ms,
                        snapped_to: None,
                    });
                }
            }
        }
        if let Some((id, delta_ms)) = clip_drag_delta {
            self.apply_clip_drag(id, delta_ms);
        }
        if clip_drag_ended {
            self.clip_drag_session = None;
        }
        if let Some(change) = want_selection {
            if let Some(p) = &mut self.project {
                match change {
                    SelectionChange::ReplaceWith(id) => {
                        p.selected_clips.clear();
                        p.selected_clips.insert(id);
                    }
                    SelectionChange::Toggle(id) => {
                        if !p.selected_clips.remove(&id) {
                            p.selected_clips.insert(id);
                        }
                    }
                    SelectionChange::Clear => {
                        p.selected_clips.clear();
                    }
                }
            }
        }
        // Mute toggle from a track header — undoable.
        if let Some(track_id) = want_mute_track {
            self.push_undo();
            if let Some(p) = &mut self.project {
                if let Some(t) = p.tracks.iter_mut().find(|t| t.id == track_id) {
                    t.muted = !t.muted;
                }
            }
        }
        // Simple Clip V1 mute toggle.
        if want_toggle_simple_mute {
            self.push_undo();
            if let Some(p) = &mut self.project {
                p.simple_audio_muted = !p.simple_audio_muted;
            }
        }
        // Advanced V1 mute (collapsed-audio state) — toggles every audio
        // track's muted flag together. If they're mixed currently, force
        // them all to muted; otherwise unmute all.
        if want_toggle_linked_audio {
            self.push_undo();
            if let Some(p) = &mut self.project {
                let all_muted = p
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio)
                    .all(|t| t.muted);
                let new_muted = !all_muted;
                for t in p.tracks.iter_mut().filter(|t| t.kind == TrackKind::Audio) {
                    t.muted = new_muted;
                }
            }
        }
        self.user_dragging_seek = dragging_any;
    }

    fn show_status_bar(&self, ui: &mut egui::Ui) {
        Frame::none()
            .fill(theme::BG_STATUS)
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::symmetric(14.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&self.status)
                            .size(12.5)
                            .color(theme::TEXT_PRIMARY),
                    );
                });
            });
    }

    /// File → Settings modal. Houses preferences that persist across
    /// runs in `~/.config/halo-clipper/config.json`: proxy quality,
    /// cache auto-clear, and a manual cache-clear button.
    fn show_settings_modal(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("settings_dim"),
        ))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 140));

        let mut close = false;
        let cache_bytes = proxy::cache_size_bytes();

        egui::Window::new("settings_modal")
            .title_bar(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .fixed_size([560.0, 470.0])
            .frame(
                Frame::window(&ctx.style())
                    .fill(theme::BG_MODAL)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(Margin::same(28.0)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Settings")
                        .size(24.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Saved to ~/.config/halo-clipper/config.json")
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(22.0);

                ui.label(
                    RichText::new("Scrub proxy quality")
                        .size(14.5)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.settings.proxy_quality,
                        ProxyQuality::Standard,
                        RichText::new("Standard").size(13.0),
                    )
                    .on_hover_text(
                        "Keyframe every 12 frames — small proxies, scrubs land \
                         within a fraction of a second of the pointer.",
                    );
                    ui.radio_value(
                        &mut self.settings.proxy_quality,
                        ProxyQuality::AllIntra,
                        RichText::new("All-intra (frame-exact)").size(13.0),
                    )
                    .on_hover_text(
                        "Every frame is a keyframe — scrubs land exactly where \
                         you point, but proxies are several times larger and \
                         slower to generate.",
                    );
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Applies to the next video you open. Existing proxies \
                         for the other profile stay cached.",
                    )
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(18.0);

                ui.label(
                    RichText::new("Scrub proxy cache")
                        .size(14.5)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);

                // Auto-clear toggle.
                let mut toggle = self.settings.auto_clear_cache_on_close;
                let resp = ui.add(egui::Checkbox::new(
                    &mut toggle,
                    RichText::new("Clear cache automatically when I close the app")
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY),
                ));
                if resp.changed() {
                    self.settings.auto_clear_cache_on_close = toggle;
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Off (default): proxies stay cached so re-opening the same \
                         clip is instant. On: free the disk space when you quit.",
                    )
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(18.0);

                // Manual clear row.
                ui.label(
                    RichText::new(format!("Cache size on disk: {}", format_bytes(cache_bytes)))
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY)
                        .monospace(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let has_cache = cache_bytes > 0;
                    if secondary_button(ui, "Clear cache now", has_cache).clicked() {
                        match proxy::clear_cache() {
                            Ok(freed) => {
                                self.status = format!(
                                    "Scrub-proxy cache cleared ({} freed).",
                                    format_bytes(freed),
                                );
                            }
                            Err(e) => {
                                log::warn!("clear_cache failed: {e}");
                                self.status = "Could not clear proxy cache — see log.".into();
                            }
                        }
                    }
                });

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    let cancel_btn = egui::Button::new(
                        RichText::new("Cancel")
                            .size(14.0)
                            .color(theme::TEXT_PRIMARY),
                    )
                    .rounding(Rounding::same(6.0))
                    .min_size(egui::vec2(110.0, 34.0));
                    if ui
                        .add(cancel_btn)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        // Cancel reverts in-memory changes by re-loading
                        // from disk; user expects "X out" to discard.
                        self.settings = Settings::load();
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "Save", true).clicked() {
                            if let Err(e) = self.settings.save() {
                                log::warn!("settings save failed: {e:#}");
                                self.status = "Could not save settings — see log.".into();
                            } else {
                                self.status = "Settings saved.".into();
                            }
                            close = true;
                        }
                    });
                });
            });

        if close {
            self.settings_open = false;
        }
    }

    /// Pre-export options modal. Two top-level modes — **Auto**
    /// (decides copy vs re-encode based on what was modified) and
    /// **Custom** (resolution / codec / bitrate / advanced unlock).
    /// "Begin Export" closes the modal and runs start_export().
    fn show_export_options_modal(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("export_opts_dim"),
        ))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 140));

        let mut begin = false;
        let mut cancel = false;
        let mut next_mode = self.export_mode;
        let modal_height = match self.export_mode {
            ExportMode::Custom if self.custom_encode.advanced_unlocked => 840.0,
            ExportMode::Custom => 680.0,
            ExportMode::Discord => 620.0,
            ExportMode::Auto => 430.0,
        };

        let _ = self.project.as_ref();

        egui::Window::new("export_options_modal")
            .title_bar(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .fixed_size([640.0, modal_height])
            .frame(
                Frame::window(&ctx.style())
                    .fill(theme::BG_MODAL)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(Margin::same(28.0)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Export Options")
                        .size(24.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Auto picks the simplest pipeline that preserves quality. Custom unlocks codec, resolution, bitrate.")
                        .size(13.0)
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(20.0);

                // Top-level Auto / Discord Nitro / Custom selector.
                // 84px tall rows with 14pt strong titles + 13pt body —
                // sized for "obvious to click, comfortable to read"
                // (Resolve's preset cards are similar dimensions).
                for (mode, title, desc) in [
                    (
                        ExportMode::Auto,
                        "Auto",
                        "Stream-copies when the In mark is keyframe-aligned and \
                         nothing was modified. Otherwise re-encodes at libx264 \
                         CRF 18 to preserve source quality.",
                    ),
                    (
                        ExportMode::Discord,
                        "Discord",
                        "Size-targeted encode that fits under a Discord upload \
                         cap — pick your tier (Free 10 MB / Nitro Basic 50 MB / \
                         Nitro 500 MB) or dial in any size.",
                    ),
                    (
                        ExportMode::Custom,
                        "Custom",
                        "Pick resolution, codec, quality-or-bitrate, and frame \
                         rate. Toggle Advanced for encoder preset and audio \
                         codec/bitrate.",
                    ),
                ] {
                    let selected = next_mode == mode;
                    let row = ui.allocate_response(
                        egui::vec2(ui.available_width(), 84.0),
                        egui::Sense::click(),
                    );

                    // Subtle selected state — let the border + radio dot
                    // carry the "this is picked" signal so the body text
                    // stays readable on near-normal background contrast.
                    let bg = if selected {
                        theme::ACCENT.linear_multiply(0.08)
                    } else if row.hovered() {
                        theme::BG_BUTTON_HOVER
                    } else {
                        theme::BG_BUTTON
                    };
                    let stroke = if selected {
                        Stroke::new(2.0_f32, theme::ACCENT)
                    } else if row.hovered() {
                        Stroke::new(1.0_f32, theme::ACCENT.linear_multiply(0.5))
                    } else {
                        Stroke::new(1.0_f32, theme::BORDER)
                    };
                    ui.painter()
                        .rect(row.rect, Rounding::same(8.0), bg, stroke);

                    // Radio indicator on the left edge.
                    let indicator_x = row.rect.left() + 22.0;
                    let indicator_y = row.rect.center().y;
                    let indicator_radius = 7.0;
                    ui.painter().circle_stroke(
                        egui::pos2(indicator_x, indicator_y),
                        indicator_radius,
                        Stroke::new(1.5_f32, if selected { theme::ACCENT } else { theme::BORDER }),
                    );
                    if selected {
                        ui.painter().circle_filled(
                            egui::pos2(indicator_x, indicator_y),
                            indicator_radius - 3.0,
                            theme::ACCENT,
                        );
                    }

                    // Paint text via the painter (not child.label) so the
                    // labels can't intercept the pointer and switch the
                    // cursor to an I-beam. The whole 640×84 card stays a
                    // single click target.
                    let text_left = row.rect.left() + 44.0;
                    let title_pos = egui::pos2(text_left, row.rect.top() + 14.0);
                    ui.painter().text(
                        title_pos,
                        egui::Align2::LEFT_TOP,
                        title,
                        egui::FontId::new(15.0, egui::FontFamily::Proportional),
                        theme::TEXT_PRIMARY,
                    );
                    // Description wraps to fit the row width.
                    let desc_pos = egui::pos2(text_left, row.rect.top() + 38.0);
                    let desc_wrap_width = row.rect.right() - 16.0 - text_left;
                    let desc_galley = ui.painter().layout(
                        desc.to_string(),
                        egui::FontId::new(12.5, egui::FontFamily::Proportional),
                        theme::TEXT_MUTED,
                        desc_wrap_width,
                    );
                    ui.painter().galley(desc_pos, desc_galley, theme::TEXT_MUTED);

                    if row.clicked() {
                        next_mode = mode;
                    }
                    row.on_hover_cursor(egui::CursorIcon::PointingHand);
                    ui.add_space(10.0);
                }

                // Per-mode sub-panel.
                match next_mode {
                    ExportMode::Discord => {
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(12.0);
                        self.show_discord_nitro_panel(ui);
                    }
                    ExportMode::Custom => {
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(12.0);
                        self.show_custom_encode_panel(ui);
                    }
                    ExportMode::Auto => {}
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    let cancel_btn = egui::Button::new(
                        RichText::new("Cancel")
                            .size(14.0)
                            .color(theme::TEXT_PRIMARY),
                    )
                    .rounding(Rounding::same(6.0))
                    .min_size(egui::vec2(110.0, 34.0));
                    if ui.add(cancel_btn)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        cancel = true;
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if primary_button(ui, "Begin Export", true).clicked() {
                                begin = true;
                            }
                        },
                    );
                });
            });

        self.export_mode = next_mode;
        if cancel {
            self.export_options_open = false;
        }
        if begin {
            self.export_options_open = false;
            self.start_export();
        }
    }

    /// Stats panel for Discord mode. Recomputes the target bitrate /
    /// resolution from the current cut duration each frame so the user
    /// sees what's about to encode without having to commit.
    fn show_discord_nitro_panel(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.project.as_ref() else {
            ui.label(RichText::new("Open a video first.").color(theme::TEXT_MUTED));
            return;
        };
        let duration_ms = self.cut_duration_ms_for_export(project);

        // ── Tier presets + target-size slider ──
        // One chip per Discord subscription tier; the slider covers
        // everything in between for people who want a specific size.
        ui.label(
            RichText::new("Target file size")
                .size(13.0)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (label, mb) in encode::DiscordNitro::TIERS {
                let active = self.discord_target_mb == *mb;
                if speed_preset_chip(ui, label, active).clicked() {
                    self.discord_target_mb = *mb;
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = 380.0;
            ui.add(
                egui::Slider::new(
                    &mut self.discord_target_mb,
                    encode::DiscordNitro::MIN_TARGET_MB..=encode::DiscordNitro::MAX_TARGET_MB,
                )
                .step_by(5.0)
                .suffix(" MB"),
            );
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Pick the tier matching where you'll upload, or drag the \
                 slider — the encoder picks resolution + bitrate to fit.",
            )
            .size(11.5)
            .color(theme::TEXT_MUTED),
        );
        ui.add_space(14.0);

        // ── Codec row: Auto, or pin a family ──
        // Auto is right almost always, but "almost" is why this row
        // exists: someone posting to a server full of phone clients may
        // want H.264 regardless of what the size math prefers, and
        // someone who knows their audience may want AV1 anyway.
        let av1_available =
            encode::DiscordNitro::best_av1(self.detected_gpu, &self.available_codecs).is_some();
        ui.label(
            RichText::new("Codec")
                .size(13.0)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for choice in encode::CodecChoice::ALL {
                let active = self.discord_codec_choice == *choice;
                let selectable = *choice != encode::CodecChoice::Av1 || av1_available;
                let resp = ui
                    .add_enabled_ui(selectable, |ui| {
                        speed_preset_chip(ui, choice.label(), active)
                    })
                    .inner;
                if resp.clicked() {
                    self.discord_codec_choice = *choice;
                }
                let tip = if selectable {
                    choice.hint()
                } else {
                    "No usable AV1 encoder on this machine"
                };
                resp.on_hover_text(tip);
            }
        });
        ui.add_space(14.0);

        // ── Live stats reflecting the slider value ──
        // Same NitroPlan::compute the export uses, so the panel can't
        // promise a codec or resolution the encode won't deliver.
        let target_bytes = encode::DiscordNitro::target_bytes_from_mb(self.discord_target_mb);
        let plan = encode::NitroPlan::compute(
            duration_ms,
            target_bytes,
            project.media.width,
            project.media.height,
            project.media.fps,
            self.discord_codec_choice,
            self.detected_gpu,
            &self.available_codecs,
        );
        let video_kbps = plan.video_kbps;
        let audio_kbps = plan.audio_kbps;
        // True when the MAX_BITRATE_KBPS global ceiling is doing more
        // work than the slider. Short clips can't usefully consume the
        // full target; we surface that so the user understands why a
        // 15-second clip at the 475 MB slider position still lands
        // smaller than expected — the cap, not the slider, is the
        // binding constraint.
        let cap_dominates = plan.cap_dominates;
        let resolution = plan.resolution;
        let dims = plan.dims;
        // Round to the nearest 5 MB. The 0.948 fudge in the estimate is
        // accurate to ~±5%, so "≈ 57 MB" implies more precision than we
        // actually have — "≈ 55 MB" reads as a ballpark, which it is.
        let est_mb_rounded = ((plan.est_mb() / 5.0).round() * 5.0).max(5.0) as i64;

        let rows: [(&str, String); 5] = [
            (
                "Cut duration",
                format!(
                    "{}:{:02}",
                    duration_ms / 60_000,
                    (duration_ms % 60_000) / 1000,
                ),
            ),
            (
                "Bitrate budget",
                format!(
                    "{:.1} Mbps video + {} kbps audio{}",
                    video_kbps as f64 / 1000.0,
                    audio_kbps,
                    if cap_dominates { "  (capped)" } else { "" },
                ),
            ),
            (
                "Resolution",
                format!("{} ({}×{})", resolution.label(), dims.0, dims.1),
            ),
            ("Codec", plan.codec.display().to_string()),
            (
                "Approx. output size",
                format!("≈ {est_mb_rounded} MB  ±10%"),
            ),
        ];

        egui::Grid::new("discord_nitro_stats")
            .num_columns(2)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                for (label, value) in rows {
                    ui.label(RichText::new(label).size(12.5).color(theme::TEXT_MUTED));
                    ui.label(
                        RichText::new(value)
                            .size(13.5)
                            .color(theme::TEXT_PRIMARY)
                            .monospace(),
                    );
                    ui.end_row();
                }
            });

        ui.add_space(10.0);
        // Pick the most useful explanatory note based on the current state.
        // Cap-warning takes priority — it explains the gap between slider
        // and actual output, which is the most common "huh?" moment.
        let note = if cap_dominates {
            format!(
                "Bitrate capped at {} Mbps — at {} s the encoder can't usefully \
                 consume the full {} MB target. Output will land near {} MB.",
                encode::DiscordNitro::MAX_BITRATE_KBPS / 1000,
                duration_ms / 1000,
                self.discord_target_mb,
                est_mb_rounded,
            )
        } else if video_kbps + audio_kbps < 500 {
            "Long clip at this target — bitrate is very low, picture may look soft.".into()
        } else {
            format!(
                "CBR encode aiming to land near {} MB (±10% in practice).",
                est_mb_rounded,
            )
        };
        let note_color = if cap_dominates || video_kbps + audio_kbps < 500 {
            theme::ACCENT_ERR.linear_multiply(0.85)
        } else {
            theme::TEXT_MUTED
        };
        ui.label(RichText::new(note).size(12.5).color(note_color));
        ui.add_space(6.0);
        // Why this codec — the AV1 switch changes encode time and
        // playback compatibility, so it shouldn't happen silently.
        ui.label(
            RichText::new(plan.codec_note())
                .size(12.5)
                .color(if plan.codec.is_av1() {
                    theme::ACCENT.linear_multiply(0.85)
                } else {
                    theme::TEXT_MUTED
                }),
        );
    }

    /// The body of the Custom export panel. Headline controls:
    /// resolution, frame rate, codec, rate control (constant quality vs
    /// target bitrate). The "Advanced" toggle reveals encoder preset and
    /// audio codec/bitrate.
    fn show_custom_encode_panel(&mut self, ui: &mut egui::Ui) {
        let src_fps = self.project.as_ref().map(|p| p.media.fps).unwrap_or(0.0);

        // Resolution + frame rate on one row.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Resolution").color(theme::TEXT_MUTED).small());
            let current = self.custom_encode.resolution;
            egui::ComboBox::from_id_salt("custom_resolution")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for preset in ResolutionPreset::ALL {
                        let resp = ui.selectable_label(*preset == current, preset.label());
                        if resp.clicked() && *preset != current {
                            self.custom_encode.resolution = *preset;
                            // Snap bitrate to the preset's default so the
                            // user doesn't have to retype every time.
                            self.custom_encode.video_bitrate_kbps = preset.default_bitrate_kbps();
                        }
                    }
                });

            ui.add_space(12.0);
            ui.label(RichText::new("Frame rate").color(theme::TEXT_MUTED).small());
            let fps_label = |limit: Option<u32>| match limit {
                None => {
                    if src_fps > 0.0 {
                        format!("Source ({src_fps:.0} fps)")
                    } else {
                        "Source".into()
                    }
                }
                Some(n) => format!("{n} fps"),
            };
            egui::ComboBox::from_id_salt("custom_fps")
                .selected_text(fps_label(self.custom_encode.fps_limit))
                .show_ui(ui, |ui| {
                    for option in [None, Some(24), Some(30), Some(60)] {
                        ui.selectable_value(
                            &mut self.custom_encode.fps_limit,
                            option,
                            fps_label(option),
                        );
                    }
                });
        });
        if self
            .custom_encode
            .fps_limit
            .is_some_and(|n| src_fps > 0.0 && n as f64 >= src_fps + 0.5)
        {
            ui.label(
                RichText::new(
                    "Chosen frame rate is above the source's — frames will be \
                     duplicated, not gained.",
                )
                .small()
                .color(theme::ACCENT_ERR.linear_multiply(0.85)),
            );
        }
        ui.add_space(6.0);

        // Recommendation banner — surfaces what the GPU probe came up
        // with so the user doesn't have to know which codec maps to
        // their hardware.
        let recommended = encode::recommend_codec(self.detected_gpu, &self.available_codecs);
        let banner = match self.detected_gpu {
            Some(g) if recommended != VideoCodec::X264 => format!(
                "Detected {} GPU → {} recommended for fastest encode.",
                g.display(),
                recommended.display(),
            ),
            Some(g) => format!(
                "Detected {} GPU but no matching HW encoder in ffmpeg — \
                 falling back to {} on the CPU.",
                g.display(),
                VideoCodec::X264.display(),
            ),
            None => format!(
                "No GPU detected — {} on the CPU.",
                VideoCodec::X264.display(),
            ),
        };
        ui.label(
            RichText::new(banner)
                .small()
                .color(theme::ACCENT.linear_multiply(0.85)),
        );
        ui.add_space(4.0);

        // Codec row. Always offer software encoders; HW encoders only
        // appear when ffmpeg actually advertised them at startup. The
        // recommended codec gets a "★ Recommended" suffix in the list.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Codec").color(theme::TEXT_MUTED).small());
            let mut all_codecs: Vec<VideoCodec> = vec![VideoCodec::X264, VideoCodec::X265];
            all_codecs.extend(self.available_codecs.iter().copied());
            let current = self.custom_encode.video_codec;
            let selected_text = if current == recommended {
                format!("{}  ★", current.display())
            } else {
                current.display().to_string()
            };
            egui::ComboBox::from_id_salt("custom_codec")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for codec in &all_codecs {
                        let label = if *codec == recommended {
                            format!("{}  ★ Recommended", codec.display())
                        } else {
                            codec.display().to_string()
                        };
                        let resp = ui.selectable_label(*codec == current, label);
                        if resp.clicked() && *codec != current {
                            self.custom_encode.set_codec(*codec);
                        }
                    }
                });
        });
        ui.add_space(8.0);

        // Rate control: constant quality (default) vs target bitrate.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Rate control")
                    .color(theme::TEXT_MUTED)
                    .small(),
            );
            ui.radio_value(
                &mut self.custom_encode.rate_control,
                RateControl::ConstantQuality,
                "Constant quality",
            )
            .on_hover_text("Same visual quality everywhere; file size varies with content.");
            ui.radio_value(
                &mut self.custom_encode.rate_control,
                RateControl::Bitrate,
                "Target bitrate",
            )
            .on_hover_text("Predictable file size; quality varies with content.");
        });
        ui.add_space(4.0);

        match self.custom_encode.rate_control {
            RateControl::ConstantQuality => {
                let codec = self.custom_encode.video_codec;
                let range = codec.quality_range();
                self.custom_encode.quality = self
                    .custom_encode
                    .quality
                    .clamp(*range.start(), *range.end());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Quality ({})", codec.quality_name()))
                            .color(theme::TEXT_MUTED)
                            .small(),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.custom_encode.quality, range)
                            .text("lower = better"),
                    );
                });
                ui.label(
                    RichText::new(format!(
                        "{} ≈ visually lossless. Each step up shrinks the file \
                         and softens fine detail.",
                        codec.default_quality(),
                    ))
                    .small()
                    .color(theme::TEXT_MUTED.linear_multiply(0.7)),
                );
            }
            RateControl::Bitrate => {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Bitrate (kbps)")
                            .color(theme::TEXT_MUTED)
                            .small(),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.custom_encode.video_bitrate_kbps, 500..=50_000)
                            .logarithmic(true),
                    );
                });
                // Rough size estimate so the slider means something.
                if let Some(project) = self.project.as_ref() {
                    let dur_ms = self.cut_duration_ms_for_export(project);
                    let total_kbps = self.custom_encode.video_bitrate_kbps as i64
                        + self.custom_encode.audio_bitrate_kbps as i64;
                    let est_mb = (total_kbps * dur_ms) as f64 / 8_000.0 / (1024.0 * 1024.0);
                    ui.label(
                        RichText::new(format!("≈ {est_mb:.0} MB at the current cut length."))
                            .small()
                            .color(theme::TEXT_MUTED.linear_multiply(0.7)),
                    );
                }
            }
        }
        ui.add_space(8.0);

        // Advanced unlock toggle.
        ui.checkbox(
            &mut self.custom_encode.advanced_unlocked,
            "Advanced — encoder preset, audio codec + bitrate",
        );

        if self.custom_encode.advanced_unlocked {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Preset").color(theme::TEXT_MUTED).small());
                let preset_options: &[&str] = match self.custom_encode.video_codec {
                    VideoCodec::X264 | VideoCodec::X265 => &[
                        "ultrafast",
                        "superfast",
                        "veryfast",
                        "faster",
                        "fast",
                        "medium",
                        "slow",
                        "slower",
                        "veryslow",
                    ],
                    // SVT-AV1 numbers its presets: 0 slowest, 13 fastest.
                    // Below 4 the encode is slower than most people will
                    // sit through, so the list starts at 4.
                    VideoCodec::SvtAv1 => &["4", "5", "6", "7", "8", "9", "10", "11", "12", "13"],
                    VideoCodec::NvencH264 | VideoCodec::NvencHevc | VideoCodec::NvencAv1 => {
                        &["p1", "p2", "p3", "p4", "p5", "p6", "p7"]
                    }
                    VideoCodec::QsvH264 | VideoCodec::QsvHevc => {
                        &["veryfast", "faster", "fast", "medium", "slow", "veryslow"]
                    }
                    VideoCodec::AmfH264 | VideoCodec::AmfHevc => &["speed", "balanced", "quality"],
                    // VAAPI has no preset knob at all; av1_qsv / av1_amf
                    // take a bare number with no named aliases, so both
                    // stay on the encoder's own default.
                    VideoCodec::VaapiH264
                    | VideoCodec::VaapiHevc
                    | VideoCodec::VaapiAv1
                    | VideoCodec::QsvAv1
                    | VideoCodec::AmfAv1 => &[""],
                };
                egui::ComboBox::from_id_salt("custom_preset")
                    .selected_text(if self.custom_encode.video_preset.is_empty() {
                        "(default)"
                    } else {
                        self.custom_encode.video_preset.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for opt in preset_options {
                            let label = if opt.is_empty() { "(default)" } else { opt };
                            ui.selectable_value(
                                &mut self.custom_encode.video_preset,
                                (*opt).into(),
                                label,
                            );
                        }
                    });
                if matches!(
                    self.custom_encode.video_codec,
                    VideoCodec::VaapiH264
                        | VideoCodec::VaapiHevc
                        | VideoCodec::VaapiAv1
                        | VideoCodec::QsvAv1
                        | VideoCodec::AmfAv1
                ) {
                    ui.label(
                        RichText::new("(no preset knob on this encoder)")
                            .small()
                            .color(theme::TEXT_MUTED.linear_multiply(0.7)),
                    );
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Audio codec")
                        .color(theme::TEXT_MUTED)
                        .small(),
                );
                let current_audio = self.custom_encode.audio_codec;
                egui::ComboBox::from_id_salt("custom_audio_codec")
                    .selected_text(current_audio.display())
                    .show_ui(ui, |ui| {
                        for codec in AudioCodec::ALL {
                            ui.selectable_value(
                                &mut self.custom_encode.audio_codec,
                                *codec,
                                codec.display(),
                            );
                        }
                    });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Audio bitrate (kbps)")
                        .color(theme::TEXT_MUTED)
                        .small(),
                );
                ui.add(
                    egui::Slider::new(&mut self.custom_encode.audio_bitrate_kbps, 64..=320)
                        .step_by(32.0),
                );
            });
        }

        if self.available_codecs.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("No hardware encoders or AV1 support detected in your ffmpeg build.")
                    .small()
                    .color(theme::TEXT_MUTED.linear_multiply(0.7)),
            );
        }
    }

    fn show_export_modal(&mut self, ctx: &egui::Context) {
        let progress = self.export.as_ref().unwrap().progress();

        // Dim the background.
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("modal_dim"),
        ))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 140));

        let mut close = false;
        let mut cancel = false;
        let mut open_folder = false;

        // Size adapts to phase — Running needs more room (stats grid),
        // Done/Cancelled are short, Failed scrolls so we cap at a max.
        let modal_height = match &progress.phase {
            ExportPhase::Running => 380.0,
            ExportPhase::Done => 260.0,
            ExportPhase::Failed(_) => 360.0,
            ExportPhase::Cancelled => 220.0,
        };

        egui::Window::new("export_modal")
            .title_bar(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .fixed_size([580.0, modal_height])
            .frame(
                Frame::window(&ctx.style())
                    .fill(theme::BG_MODAL)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(Margin::same(28.0)),
            )
            .show(ctx, |ui| {
                match &progress.phase {
                    ExportPhase::Running => {
                        // Header — title + destination filename.
                        ui.label(
                            RichText::new("Exporting Clip")
                                .size(22.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "→  {}",
                                progress
                                    .destination
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                            ))
                            .size(12.5)
                            .color(theme::TEXT_MUTED)
                            .monospace(),
                        );

                        ui.add_space(22.0);

                        // State copy + percentage. They sit ABOVE the
                        // bar so the bar itself stays clean.
                        let initializing =
                            progress.current_ms == 0 && progress.elapsed().as_secs() >= 2;
                        let pct = (progress.fraction() * 100.0).round() as i32;
                        let state_text = if initializing {
                            "Initializing encoder…".to_string()
                        } else {
                            format!("Encoding · {pct}%")
                        };
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&state_text)
                                    .size(13.5)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let speed_label = if initializing || progress.speed < 0.01 {
                                        String::new()
                                    } else {
                                        format!("{:.2}×", progress.speed)
                                    };
                                    if !speed_label.is_empty() {
                                        ui.label(
                                            RichText::new(speed_label)
                                                .size(13.5)
                                                .monospace()
                                                .color(theme::TEXT_MUTED),
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        paint_progress_bar(ui, progress.fraction(), initializing);

                        ui.add_space(20.0);

                        // Stats grid — 4 columns: label, value, label, value.
                        // Labels muted/small; values primary/monospace.
                        let elapsed = export::format_duration(progress.elapsed());
                        let eta = if initializing {
                            "warming up…".to_string()
                        } else {
                            progress
                                .eta()
                                .map(export::format_duration)
                                .unwrap_or_else(|| "calculating…".into())
                        };
                        let speed_v = if progress.speed < 0.01 {
                            "—".into()
                        } else {
                            format!("{:.2}× realtime", progress.speed)
                        };
                        let fps_v = if progress.fps < 0.1 {
                            "—".into()
                        } else {
                            format!("{:.0}", progress.fps)
                        };
                        let progress_v = format!(
                            "{} / {}",
                            media::format_timestamp(progress.current_ms),
                            media::format_timestamp(progress.total_ms)
                        );
                        let mode_v = progress.mode_label.to_string();

                        egui::Grid::new("export_stats_v2")
                            .num_columns(4)
                            .spacing([20.0, 12.0])
                            .min_col_width(70.0)
                            .show(ui, |ui| {
                                stat_grid_row(ui, "Elapsed", &elapsed, "ETA", &eta);
                                stat_grid_row(ui, "Speed", &speed_v, "Encode FPS", &fps_v);
                                stat_grid_row(ui, "Progress", &progress_v, "Mode", &mode_v);
                            });

                        ui.add_space(22.0);
                        ui.separator();
                        ui.add_space(14.0);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if secondary_button(ui, "Cancel", true).clicked() {
                                cancel = true;
                            }
                        });
                    }
                    ExportPhase::Done => {
                        ui.label(
                            RichText::new("✓  Export Complete")
                                .size(22.0)
                                .strong()
                                .color(theme::ACCENT_OK),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(progress.destination.display().to_string())
                                .size(12.5)
                                .monospace()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!(
                                "Took {}  ·  {}",
                                export::format_duration(progress.elapsed()),
                                progress.mode_label,
                            ))
                            .size(12.5)
                            .color(theme::TEXT_MUTED),
                        );

                        ui.add_space(24.0);
                        ui.separator();
                        ui.add_space(14.0);

                        ui.horizontal(|ui| {
                            if secondary_button(ui, "Open Folder", true).clicked() {
                                open_folder = true;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if primary_button(ui, "Done", true).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    }
                    ExportPhase::Failed(err) => {
                        ui.label(
                            RichText::new("✗  Export Failed")
                                .size(22.0)
                                .strong()
                                .color(theme::ACCENT_ERR),
                        );
                        ui.add_space(14.0);
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(err)
                                        .size(12.0)
                                        .monospace()
                                        .color(theme::TEXT_PRIMARY),
                                );
                            });
                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if secondary_button(ui, "Close", true).clicked() {
                                close = true;
                            }
                        });
                    }
                    ExportPhase::Cancelled => {
                        ui.label(
                            RichText::new("Export Cancelled")
                                .size(22.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("The output file may be incomplete.")
                                .size(13.0)
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if secondary_button(ui, "Close", true).clicked() {
                                close = true;
                            }
                        });
                    }
                }
            });

        if open_folder {
            if let Some(parent) = progress.destination.parent() {
                let _ = crate::platform::command("xdg-open").arg(parent).spawn();
            }
        }

        if cancel {
            if let Some(h) = &self.export {
                h.cancel();
            }
        }
        if close {
            // Snapshot status before dropping handle
            self.status = match progress.phase {
                ExportPhase::Done => format!("Exported to {}", progress.destination.display()),
                ExportPhase::Failed(_) => "Export failed.".into(),
                ExportPhase::Cancelled => "Export cancelled.".into(),
                ExportPhase::Running => self.status.clone(),
            };
            self.export = None;
        }
    }
}

// Paint callback invoked once per frame, inside egui's render pass.
// Runs on the main (UI) thread with a current GL context.
//
// Pipeline:
//   1. Snapshot GL state we might disturb.
//   2. (Re)create our offscreen FBO sized to the target rect.
//   3. Bind our FBO, ask mpv to render into it.
//   4. Bind the original draw FBO back, blit our FBO into the rect.
//   5. Restore GL state for egui's subsequent draws.
//
// Why the FBO indirection: mpv calls glViewport(0,0,w,h) internally before
// drawing, so any viewport we set is overwritten — mpv always paints into
// the bottom-left corner of whatever framebuffer is bound. To get the
// video into an arbitrary rect we render into a private FBO at (0,0,w,h)
// then blit-copy that FBO into the rect we want.
fn paint_mpv(
    player: &Arc<Player>,
    video_fbo: &Arc<Mutex<Option<VideoFbo>>>,
    info: egui::PaintCallbackInfo,
    painter: &egui_glow::Painter,
) {
    if let Err(e) = player.ensure_render_context() {
        log::error!("ensure_render_context: {e:#}");
        return;
    }

    let gl = painter.gl();
    let vp = info.viewport_in_pixels();
    let (vp_x, vp_y) = (vp.left_px, vp.from_bottom_px);
    let (vp_w, vp_h) = (vp.width_px, vp.height_px);
    if vp_w <= 0 || vp_h <= 0 {
        return;
    }

    let mut fbo_slot = video_fbo.lock().unwrap();
    VideoFbo::ensure_size(&mut fbo_slot, gl, vp_w, vp_h);
    let Some(my_fbo) = fbo_slot.as_ref() else {
        log::error!("video FBO allocation failed; skipping frame");
        return;
    };

    unsafe {
        // Snapshot
        let prev_draw_fbo = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        let prev_read_fbo = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
        let mut prev_viewport = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut prev_viewport);
        let prev_scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
        let mut prev_scissor = [0i32; 4];
        gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut prev_scissor);
        let prev_blend_enabled = gl.is_enabled(glow::BLEND);
        let prev_depth_enabled = gl.is_enabled(glow::DEPTH_TEST);
        let prev_cull_enabled = gl.is_enabled(glow::CULL_FACE);
        let prev_active_tex = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);

        gl.disable(glow::SCISSOR_TEST);

        // 1. Bind our FBO and let mpv render into it.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(my_fbo.fbo()));
        gl.viewport(0, 0, vp_w, vp_h);
        if let Err(e) = player.render(my_fbo.fbo_id_i32(), vp_w, vp_h, true) {
            log::error!("mpv render: {e:#}");
        }

        // 2. Blit our FBO into the rect on the original draw target.
        let dst_fbo = video_fbo::fbo_from_raw(prev_draw_fbo);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(my_fbo.fbo()));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, dst_fbo);
        gl.blit_framebuffer(
            0,
            0,
            vp_w,
            vp_h,
            vp_x,
            vp_y,
            vp_x + vp_w,
            vp_y + vp_h,
            glow::COLOR_BUFFER_BIT,
            glow::LINEAR,
        );

        // Restore for egui's next draws.
        gl.bind_framebuffer(
            glow::READ_FRAMEBUFFER,
            video_fbo::fbo_from_raw(prev_read_fbo),
        );
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, dst_fbo);
        gl.viewport(
            prev_viewport[0],
            prev_viewport[1],
            prev_viewport[2],
            prev_viewport[3],
        );
        gl.scissor(
            prev_scissor[0],
            prev_scissor[1],
            prev_scissor[2],
            prev_scissor[3],
        );
        if prev_scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        if prev_blend_enabled {
            gl.enable(glow::BLEND);
        } else {
            gl.disable(glow::BLEND);
        }
        if prev_depth_enabled {
            gl.enable(glow::DEPTH_TEST);
        } else {
            gl.disable(glow::DEPTH_TEST);
        }
        if prev_cull_enabled {
            gl.enable(glow::CULL_FACE);
        } else {
            gl.disable(glow::CULL_FACE);
        }
        gl.active_texture(prev_active_tex as u32);
    }
}

/// Build the audio half of mpv's lavfi-complex filter graph. Returns
/// None when every audio track in scope is muted — caller falls back to
/// mute=on.
///
/// Linked state (no audio clip detached): only A1 plays. The other
/// audio streams sit hidden under V1 and shouldn't all blast in the
/// preview at once.
///
/// Detached state: mix every non-muted audio track via amix, matching
/// the per-track mute glyphs the user can now see.
fn build_audio_chain(p: &Project) -> Option<String> {
    // A soundless source still gets one audio lane in the data model,
    // but referencing [aid1] on a file with no audio stream makes mpv
    // reject the whole filter graph.
    if p.media.audio_stream_count == 0 {
        return None;
    }
    let any_audio_detached = p.clips.iter().any(|c| {
        c.link.is_none()
            && p.tracks
                .iter()
                .any(|t| t.id == c.track_id && t.kind == TrackKind::Audio)
    });
    let stream_exists = |idx: &usize| *idx < p.media.audio_stream_count;
    // Effective gain = track gain × the gain of the clip currently under
    // the playhead on that track. Gain is a per-clip property, and mpv's
    // volume filter is a single scalar, so the preview follows whichever
    // segment is playing — the same edge-triggered trick `sync_clip_speed`
    // uses. The chain string changes when the playhead crosses into a
    // clip with a different gain, and `sync_audio_mix` pushes it then.
    let effective_gain = |t: &Track| -> f32 {
        let clip_gain = track::clip_on_track_at(&p.clips, t.id, p.playhead_ms)
            .map(|c| c.gain)
            .unwrap_or(1.0);
        (t.gain * clip_gain).max(0.0)
    };
    let unmuted: Vec<(usize, f32)> = if any_audio_detached {
        p.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.muted)
            .filter_map(|t| {
                t.source_audio_index
                    .filter(stream_exists)
                    .map(|idx| (idx, effective_gain(t)))
            })
            .collect()
    } else {
        p.tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio && !t.muted)
            .and_then(|t| {
                t.source_audio_index
                    .filter(stream_exists)
                    .map(|idx| (idx, effective_gain(t)))
            })
            .into_iter()
            .collect()
    };
    if unmuted.is_empty() {
        return None;
    }
    if unmuted.len() == 1 {
        let (idx, gain) = unmuted[0];
        return Some(format!("[aid{}]volume={:.3}[ao]", idx + 1, gain));
    }
    let mut filter = String::new();
    let mut labels: Vec<String> = Vec::with_capacity(unmuted.len());
    for (i, (idx, gain)) in unmuted.iter().enumerate() {
        let label = format!("[t{}]", i);
        filter.push_str(&format!("[aid{}]volume={:.3}{};", idx + 1, gain, label));
        labels.push(label);
    }
    filter.push_str(&format!(
        "{}amix=inputs={}[ao]",
        labels.concat(),
        labels.len(),
    ));
    Some(filter)
}

/// Find the nearest snap candidate for a clip being dragged.
///
/// Returns the `cut_start_ms` the primary clip should land at, or None if
/// no candidate is within `threshold_ms` of any edge. Considers:
/// project bounds, the playhead, and every other clip's `cut_start` /
/// `cut_end` (skipping the primary's own link group — those move with it).
#[allow(clippy::too_many_arguments)]
fn find_snap_target(
    intended_cut: i64,
    primary_dur: i64,
    primary_id: ClipId,
    primary_link: Option<LinkId>,
    clips: &[Clip],
    playhead: i64,
    canvas_length: i64,
    threshold_ms: i64,
) -> Option<i64> {
    let intended_end = intended_cut + primary_dur;
    let mut best: Option<(i64, i64)> = None;
    let mut consider = |target_cut: i64, distance: i64| {
        if distance > threshold_ms {
            return;
        }
        match best {
            Some((_, d)) if d <= distance => {}
            _ => best = Some((target_cut, distance)),
        }
    };

    // Project boundaries.
    consider(0, intended_cut.abs());
    consider(
        (canvas_length - primary_dur).max(0),
        (intended_end - canvas_length).abs(),
    );
    // Playhead — align either edge to it.
    consider(playhead, (intended_cut - playhead).abs());
    consider(playhead - primary_dur, (intended_end - playhead).abs());
    // Edges of every other clip (skip link partners — they move with us).
    for c in clips {
        if c.id == primary_id {
            continue;
        }
        if let Some(pl) = primary_link {
            if c.link == Some(pl) {
                continue;
            }
        }
        let cs = c.cut_start_ms;
        let ce = c.cut_end_ms();
        consider(cs, (intended_cut - cs).abs());
        consider(ce, (intended_cut - ce).abs());
        consider(cs - primary_dur, (intended_end - cs).abs());
        consider(ce - primary_dur, (intended_end - ce).abs());
    }

    best.map(|(t, _)| t)
}

/// Shared ordering for exported ranges and their segment captions.
#[cfg(test)]
fn sorted_track_clips(clips: &[Clip], track_id: TrackId) -> Vec<&Clip> {
    let mut out: Vec<&Clip> = clips.iter().filter(|c| c.track_id == track_id).collect();
    out.sort_by_key(|c| c.cut_start_ms);
    out
}

fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if b >= GB {
        format!("{:.2} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

fn show_simpleclip_info(ui: &mut egui::Ui, project: &Project) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("In").color(theme::ACCENT_IN).small().strong());
        ui.label(
            RichText::new(match project.in_ms {
                Some(ms) => media::format_timestamp(ms),
                None => "—".into(),
            })
            .monospace()
            .color(theme::TEXT_PRIMARY),
        );

        ui.add_space(10.0);
        ui.label(
            RichText::new("Out")
                .color(theme::ACCENT_OUT)
                .small()
                .strong(),
        );
        ui.label(
            RichText::new(match project.out_ms {
                Some(ms) => media::format_timestamp(ms),
                None => "—".into(),
            })
            .monospace()
            .color(theme::TEXT_PRIMARY),
        );

        if let (Some(i), Some(o)) = (project.in_ms, project.out_ms) {
            ui.add_space(10.0);
            let dur = (i.max(o) - i.min(o)).max(0);
            ui.label(RichText::new("Selection").color(theme::TEXT_MUTED).small());
            ui.label(
                RichText::new(media::format_timestamp(dur))
                    .monospace()
                    .strong()
                    .color(theme::ACCENT),
            );
        }
    });
}

/// Big monospace playhead / total readout that sits above the timeline.
/// Acts as the "primary timecode display" — what the user looks at to
/// see where they are. Style is borrowed from Resolve / Premiere: bold
/// current value, muted divisor, lots of horizontal breathing room.
fn show_playhead_readout(ui: &mut egui::Ui, playhead_ms: i64, total_ms: i64) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            RichText::new(media::format_timestamp(playhead_ms))
                .monospace()
                .size(22.0)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.label(
            RichText::new("/")
                .monospace()
                .size(20.0)
                .color(theme::TEXT_MUTED),
        );
        ui.label(
            RichText::new(media::format_timestamp(total_ms))
                .monospace()
                .size(20.0)
                .color(theme::TEXT_MUTED),
        );
    });
}

fn show_multitrack_info(ui: &mut egui::Ui, project: &Project, cut_duration: i64) {
    let video_track = project.first_video_track();
    let video_clip_count = video_track
        .map(|v| project.clips.iter().filter(|c| c.track_id == v).count())
        .unwrap_or(0);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Playhead").color(theme::TEXT_MUTED).small());
        ui.label(
            RichText::new(media::format_timestamp(project.playhead_ms))
                .monospace()
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.label(
            RichText::new(format!("/ {}", media::format_timestamp(cut_duration)))
                .monospace()
                .color(theme::TEXT_MUTED),
        );

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(RichText::new("V1 clips").color(theme::TEXT_MUTED).small());
        ui.label(
            RichText::new(format!("{video_clip_count}"))
                .monospace()
                .color(theme::TEXT_PRIMARY),
        );
    });
}

/// Primary call-to-action button. Uses the muted-teal `BTN_PRIMARY`
/// palette instead of the high-contrast `ACCENT` so it draws the eye
/// without glaring. One source of truth for every primary CTA
/// (Export Clip / Export Cut / Begin Export / Open Video).
fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    painted_button(
        ui,
        label,
        enabled,
        egui::vec2(150.0, 34.0),
        theme::BTN_PRIMARY,
        theme::BTN_PRIMARY_HOVER,
        theme::BTN_PRIMARY_TEXT,
        theme::TEXT_MUTED,
        14.0,
        true,
    )
}

/// Generic painted-button core used by primary/secondary/accent.
/// One source of truth so all clickable controls react identically to
/// hover/press — same rounding, same hand-cursor, same disabled fade.
///
/// The button auto-grows to fit `label` (min size = `min`), so longer
/// captions like "Detach Audio (D)" don't get cropped.
#[allow(clippy::too_many_arguments)]
fn painted_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    min: egui::Vec2,
    fill_normal: Color32,
    fill_hover: Color32,
    text_normal: Color32,
    text_disabled: Color32,
    font_size: f32,
    _strong: bool,
) -> egui::Response {
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Proportional);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font_id.clone(), text_normal);
    let pad_x = 18.0;
    let desired = egui::vec2((galley.size().x + pad_x * 2.0).max(min.x), min.y);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired, sense);

    let (fill, text_color) = if !enabled {
        (theme::BG_BUTTON_DISABLED, text_disabled)
    } else if response.hovered() {
        (fill_hover, text_normal)
    } else {
        (fill_normal, text_normal)
    };

    ui.painter().rect_filled(rect, Rounding::same(6.0), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font_id,
        text_color,
    );

    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Secondary action button — Set In/Set Out/Split/Delete/Pop Out, etc.
/// Same shape as primary_button but with the muted-grey palette so
/// primaries (Export, Begin Export) still draw the eye.
fn secondary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    painted_button(
        ui,
        label,
        enabled,
        egui::vec2(110.0, 32.0),
        theme::BG_BUTTON,
        theme::BG_BUTTON_HOVER,
        theme::TEXT_PRIMARY,
        theme::TEXT_MUTED,
        13.5,
        true,
    )
}

/// Like `secondary_button` but with a colored label — for the In/Out/
/// Split/Delete actions where the color carries meaning (green = Set In,
/// red = delete, etc). Stays on the muted grey fill so the row reads as
/// a uniform action strip, not a row of confetti.
fn accent_button(ui: &mut egui::Ui, label: &str, accent: Color32, enabled: bool) -> egui::Response {
    painted_button(
        ui,
        label,
        enabled,
        egui::vec2(120.0, 32.0),
        theme::BG_BUTTON,
        theme::BG_BUTTON_HOVER,
        accent,
        theme::TEXT_MUTED,
        13.5,
        true,
    )
}

/// Pill-shaped tab for the mode toggle. Selected → primary fill + light
/// text. Unselected → transparent fill + muted text with hover tint.
fn pill_button(ui: &mut egui::Ui, label: &str, selected: bool, enabled: bool) -> egui::Response {
    let font_id = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), font_id.clone(), theme::TEXT_PRIMARY);
    let pad_x = 22.0;
    let size = egui::vec2((galley.size().x + pad_x * 2.0).max(100.0), 30.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let hovered = enabled && response.hovered();
    let (fill, text_color) = if !enabled {
        (Color32::TRANSPARENT, theme::TEXT_MUTED)
    } else if selected {
        (theme::BTN_PRIMARY, theme::BTN_PRIMARY_TEXT)
    } else if hovered {
        (theme::BG_BUTTON_HOVER, theme::TEXT_PRIMARY)
    } else {
        (Color32::TRANSPARENT, theme::TEXT_MUTED)
    };

    ui.painter().rect_filled(rect, Rounding::same(15.0), fill);
    if !selected && enabled {
        ui.painter().rect_stroke(
            rect,
            Rounding::same(15.0),
            Stroke::new(1.0_f32, theme::BORDER),
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font_id,
        text_color,
    );

    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Slim progress bar used by the export modal — designed to fit the
/// app's palette and to gracefully handle the "no frames yet" case.
///
/// Two modes:
///
/// - **Determinate** (`indeterminate = false`): a static fill from 0
///   to `fraction × width`, no inline text. Percentage is shown
///   separately above the bar so the bar itself stays uncluttered.
///
/// - **Indeterminate** (`indeterminate = true`): an animated stripe
///   that loops across the bar. Used while ffmpeg is initializing and
///   hasn't reported any frames yet — better than showing a "0%" sliver
///   that looks broken. Schedules a continuous repaint so the
///   animation runs smoothly at refresh-rate.
fn paint_progress_bar(ui: &mut egui::Ui, fraction: f32, indeterminate: bool) {
    let height = 10.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let rounding = Rounding::same(height * 0.5);

    // Track background.
    ui.painter().rect_filled(rect, rounding, theme::BG_BUTTON);

    if indeterminate {
        // Stripe slides across the track at ~1 cycle / 1.6s.
        let t = ui.input(|i| i.time);
        let cycle = 1.6_f64;
        let phase = ((t % cycle) / cycle) as f32; // 0..1
        let stripe_w = rect.width() * 0.30;
        // Travel range so stripe enters from the left edge and exits the right.
        let travel = rect.width() + stripe_w;
        let stripe_left = rect.left() - stripe_w + travel * phase;
        let stripe_right = stripe_left + stripe_w;
        // Clip to the track so the rounded ends look clean.
        let clipped_left = stripe_left.max(rect.left());
        let clipped_right = stripe_right.min(rect.right());
        if clipped_right > clipped_left {
            let stripe_rect = egui::Rect::from_min_max(
                egui::pos2(clipped_left, rect.top()),
                egui::pos2(clipped_right, rect.bottom()),
            );
            ui.painter()
                .rect_filled(stripe_rect, rounding, theme::BTN_PRIMARY);
        }
        ui.ctx().request_repaint();
    } else {
        let fill_w = rect.width() * fraction.clamp(0.0, 1.0);
        if fill_w > 0.5 {
            let fill_rect = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, height));
            ui.painter()
                .rect_filled(fill_rect, rounding, theme::BTN_PRIMARY);
        }
    }
}

/// Small chip-style button used for the Speed preset row (0.5×/1×/etc.).
/// Active chip uses the primary palette so the user can see at a glance
/// which preset matches the current speed. Inactive chips are flat
/// buttons that brighten on hover.
fn speed_preset_chip(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let font_id = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), font_id.clone(), theme::TEXT_PRIMARY);
    let size = egui::vec2((galley.size().x + 18.0).max(44.0), 26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let hovered = response.hovered();
    let (fill, text_color) = if active {
        (theme::BTN_PRIMARY, theme::BTN_PRIMARY_TEXT)
    } else if hovered {
        (theme::BG_BUTTON_HOVER, theme::TEXT_PRIMARY)
    } else {
        (theme::BG_BUTTON, theme::TEXT_PRIMARY)
    };

    ui.painter().rect_filled(rect, Rounding::same(13.0), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font_id,
        text_color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Two label/value pairs rendered as one Grid row — label cells use
/// muted/small typography, value cells use primary/monospace. Stacks
/// flow as a clean 4-column block when called inside `egui::Grid::new`
/// with `num_columns(4)`. Used by the export modal stats panel.
fn stat_grid_row(ui: &mut egui::Ui, k1: &str, v1: &str, k2: &str, v2: &str) {
    ui.label(RichText::new(k1).size(12.0).color(theme::TEXT_MUTED));
    ui.label(
        RichText::new(v1)
            .size(13.5)
            .monospace()
            .color(theme::TEXT_PRIMARY),
    );
    ui.label(RichText::new(k2).size(12.0).color(theme::TEXT_MUTED));
    ui.label(
        RichText::new(v2)
            .size(13.5)
            .monospace()
            .color(theme::TEXT_PRIMARY),
    );
    ui.end_row();
}

#[cfg(test)]
mod caption_tests {
    use super::*;

    fn project() -> Project {
        let mut p = Project {
            source: PathBuf::from("test.mp4"),
            proxy: None,
            media: MediaInfo {
                duration_ms: 6_000,
                width: 320,
                height: 180,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: None,
                audio_stream_count: 0,
            },
            playhead_ms: 0,
            in_ms: None,
            out_ms: None,
            simple_audio_muted: false,
            tracks: Vec::new(),
            clips: Vec::new(),
            selected_clips: HashSet::new(),
            text_overlays: HashMap::new(),
            id_gen: IdGen::default(),
            mode: EditMode::MultiTrack,
        };
        p.ensure_tracks_initialized();
        p
    }

    #[test]
    fn split_captions_edit_independently_and_undo_restores_them() {
        let mut p = project();
        let left = p.clips[0].id;
        p.text_overlays.insert(
            left,
            TextOverlay {
                text: "Before".into(),
                ..Default::default()
            },
        );
        let before = UndoSnapshot::capture(&p);
        let right = p.split_segments(&[left], 2_000)[0];
        assert_eq!(p.text_overlays[&right].text, "Before");
        p.text_overlays.get_mut(&right).unwrap().text = "After".into();
        assert_eq!(p.text_overlays[&left].text, "Before");
        p.playhead_ms = 1_999;
        assert_eq!(p.active_caption().unwrap().1.text, "Before");
        p.playhead_ms = 2_000;
        assert_eq!(p.active_caption().unwrap().1.text, "After");
        let after = UndoSnapshot::capture(&p);
        before.apply(&mut p);
        assert!(!p.text_overlays.contains_key(&right));
        assert_eq!(p.clips[0].src_end_ms, 6_000);
        after.apply(&mut p);
        assert_eq!(p.active_caption().unwrap().1.text, "After");
    }

    #[test]
    fn captions_follow_moved_segments_and_disappear_in_gaps() {
        let mut p = project();
        let left = p.clips[0].id;
        let right = p.split_segments(&[left], 2_000)[0];
        p.text_overlays.insert(
            left,
            TextOverlay {
                text: "First".into(),
                ..Default::default()
            },
        );
        p.text_overlays.insert(
            right,
            TextOverlay {
                text: "Second".into(),
                ..Default::default()
            },
        );
        p.clips
            .iter_mut()
            .find(|c| c.id == left)
            .unwrap()
            .cut_start_ms = 7_000;
        p.clips
            .iter_mut()
            .find(|c| c.id == right)
            .unwrap()
            .cut_start_ms = 0;
        let captions = p.video_captions(p.first_video_track().unwrap());
        assert_eq!(captions[0].as_ref().unwrap().text, "Second");
        assert_eq!(captions[1].as_ref().unwrap().text, "First");
        p.playhead_ms = 5_000;
        assert!(p.active_caption().is_none());
        p.playhead_ms = 7_000;
        assert_eq!(p.active_caption().unwrap().1.text, "First");
        p.text_overlays.remove(&left);
        assert!(p.active_caption().is_none());
    }
}
