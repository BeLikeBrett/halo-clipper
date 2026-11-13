//! Multi-track timeline data model.
//!
//! A project in `EditMode::MultiTrack` is a list of tracks plus a flat
//! list of clips. Each clip belongs to one track and references a
//! half-open source range. Optional `LinkId`s group clips that were
//! derived from the same source moment (e.g. the V+A pair created when
//! you load a video) so the user can cut them together by default and
//! detach when they want independent edits.
//!
//! Clips have a `cut_start_ms` — their position on the timeline — so
//! gaps between clips are first-class (a kept video clip with a deleted
//! audio clip below it leaves a hole the export pipeline must fill with
//! silence).

use crate::media::MediaInfo;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ClipId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct LinkId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TrackId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Debug)]
pub struct LayerSource {
    pub path: PathBuf,
    pub media: MediaInfo,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            scale: 1.0,
            x: 0.5,
            y: 0.5,
            opacity: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    /// Track-level audio gain, 1.0 = unity. Audio-only; ignored on video
    /// tracks. Set from the Edit panel, applied in both the preview mix
    /// and the export filter graph.
    pub gain: f32,
    /// For audio tracks: which input audio stream (`[0:a:N]`) this track
    /// pulls from. Video tracks: `None`. Lets us probe-once / map-many
    /// when the source has multiple audio streams (game audio + mic, etc.).
    pub source_audio_index: Option<usize>,
    /// None uses the project source. Imported layers retain their own original.
    pub source: Option<LayerSource>,
}

#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub id: ClipId,
    pub track_id: TrackId,
    /// Position on the cut timeline (where this clip begins playing).
    pub cut_start_ms: i64,
    /// Half-open source range.
    pub src_start_ms: i64,
    pub src_end_ms: i64,
    pub link: Option<LinkId>,
    /// Per-clip audio gain. 1.0 = unity. Ignored for video clips.
    pub gain: f32,
    /// Playback speed multiplier. 1.0 = normal, 0.5 = half, 2.0 = double.
    /// Preview mirrors this onto mpv's `speed` property while playback
    /// is inside the clip; export bakes it via setpts/atempo. Timeline
    /// geometry is unchanged — speed is treated as a post-process effect
    /// for now, so a 2× clip still occupies its full source width on
    /// the timeline.
    pub speed: f32,
    pub transform: Transform,
}

impl Clip {
    pub fn src_duration_ms(&self) -> i64 {
        (self.src_end_ms - self.src_start_ms).max(0)
    }
    pub fn duration_ms(&self) -> i64 {
        (self.src_duration_ms() as f64 / self.speed.clamp(0.25, 4.0) as f64).round() as i64
    }
    pub fn cut_end_ms(&self) -> i64 {
        self.cut_start_ms + self.duration_ms()
    }
    pub fn contains_cut(&self, t: i64) -> bool {
        t >= self.cut_start_ms && t < self.cut_end_ms()
    }
    /// Map cut-time inside this clip to its source-time equivalent.
    pub fn cut_to_src(&self, cut_ms: i64) -> Option<i64> {
        self.contains_cut(cut_ms).then_some(
            self.src_start_ms
                + ((cut_ms - self.cut_start_ms) as f64 * self.speed.clamp(0.25, 4.0) as f64).round()
                    as i64,
        )
    }
}

/// Per-segment text overlay (drawn into the video by ffmpeg's drawtext).
/// Position is normalized 0..=1 of the source frame so the same overlay
/// reads identically at any resolution; rendered centered on (x, y).
#[derive(Clone, Debug, PartialEq)]
pub struct TextOverlay {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub font_size_px: u32,
    pub color: [u8; 3],
    /// Fontconfig font name. Values like "Sans", "Sans Bold", "Serif",
    /// "Mono" are resolved through libfontconfig at render time.
    pub font: String,
    /// Black outline thickness in source pixels. 0 = off. Maps to
    /// drawtext's `borderw`; the preview approximates it with offset
    /// passes of the same glyphs.
    pub outline_px: u32,
    /// Soft drop shadow (fixed 2px offset at export scale). Maps to
    /// drawtext's `shadowx/shadowy/shadowcolor`.
    pub shadow: bool,
    /// Text opacity, 0.1..=1.0. Applied to fill, outline, and shadow.
    pub opacity: f32,
}

impl Default for TextOverlay {
    fn default() -> Self {
        Self {
            text: String::new(),
            x: 0.5,
            y: 0.1,
            font_size_px: 36,
            color: [255, 255, 255],
            font: "Sans".into(),
            outline_px: 0,
            shadow: false,
            opacity: 1.0,
        }
    }
}

/// A font option exposed in the Edit panel.
///
/// Why three fields:
/// - `display` is the label the user picks from the dropdown and is what
///   we store in `TextOverlay::font` (stable, human-readable).
/// - `egui_family` is the font-family egui paints the preview overlay
///   with — preview rendering happens in our UI, not in mpv, so smooth
///   drag/resize is just a matter of egui repainting.
/// - `fontconfig` is the pattern ffmpeg's drawtext receives at export
///   time. "Sans Bold" isn't a fontconfig family on its own; the proper
///   form is the family plus a `:style=` qualifier.
pub struct FontChoice {
    pub display: &'static str,
    pub egui_family: &'static str,
    #[cfg_attr(windows, allow(dead_code))]
    pub fontconfig: &'static str,
}

pub const FONT_CHOICES: &[FontChoice] = &[
    FontChoice {
        display: "Sans",
        egui_family: "DejaVu Sans",
        fontconfig: "Sans",
    },
    FontChoice {
        display: "Sans Bold",
        egui_family: "DejaVu Sans Bold",
        fontconfig: "Sans:style=Bold",
    },
    FontChoice {
        display: "Sans Italic",
        egui_family: "DejaVu Sans Oblique",
        fontconfig: "Sans:style=Italic",
    },
    FontChoice {
        display: "Serif",
        egui_family: "DejaVu Serif",
        fontconfig: "Serif",
    },
    FontChoice {
        display: "Serif Bold",
        egui_family: "DejaVu Serif Bold",
        fontconfig: "Serif:style=Bold",
    },
    FontChoice {
        display: "Mono",
        egui_family: "DejaVu Sans Mono",
        fontconfig: "Monospace",
    },
];

pub fn font_choice(display: &str) -> &'static FontChoice {
    FONT_CHOICES
        .iter()
        .find(|c| c.display == display)
        .unwrap_or(&FONT_CHOICES[0])
}

/// Monotonic ID generator. Atomic so helpers can borrow `&IdGen` while
/// the caller holds `&mut` on the clip list (disjoint borrows).
#[derive(Debug, Default)]
pub struct IdGen {
    clip: AtomicU32,
    link: AtomicU32,
    track: AtomicU32,
}

impl IdGen {
    pub fn next_clip(&self) -> ClipId {
        ClipId(self.clip.fetch_add(1, Ordering::Relaxed))
    }
    pub fn next_link(&self) -> LinkId {
        LinkId(self.link.fetch_add(1, Ordering::Relaxed))
    }
    pub fn next_track(&self) -> TrackId {
        TrackId(self.track.fetch_add(1, Ordering::Relaxed))
    }
}

/// Longest cut_end across the entire project.
pub fn project_cut_duration(clips: &[Clip]) -> i64 {
    clips.iter().map(Clip::cut_end_ms).max().unwrap_or(0)
}

/// Find the clip on `track_id` that contains `cut_ms`, if any.
pub fn clip_on_track_at(clips: &[Clip], track_id: TrackId, cut_ms: i64) -> Option<&Clip> {
    clips
        .iter()
        .find(|c| c.track_id == track_id && c.contains_cut(cut_ms))
}

pub fn clip_by_id(clips: &[Clip], id: ClipId) -> Option<&Clip> {
    clips.iter().find(|c| c.id == id)
}

/// Split a single clip at `cut_ms`. Both halves keep the original link.
/// Returns the new (right-half) clip's ID on success.
pub fn split_clip(
    clips: &mut Vec<Clip>,
    target: ClipId,
    cut_ms: i64,
    id_gen: &IdGen,
) -> Option<ClipId> {
    let idx = clips.iter().position(|c| c.id == target)?;
    let c = clips[idx];
    let split_src = c.cut_to_src(cut_ms)?;
    if split_src <= c.src_start_ms || split_src >= c.src_end_ms {
        return None;
    }
    clips[idx].src_end_ms = split_src;
    let right = Clip {
        id: id_gen.next_clip(),
        track_id: c.track_id,
        cut_start_ms: cut_ms,
        src_start_ms: split_src,
        src_end_ms: c.src_end_ms,
        link: c.link,
        gain: c.gain,
        speed: c.speed,
        transform: c.transform,
    };
    let right_id = right.id;
    clips.insert(idx + 1, right);
    Some(right_id)
}

/// Split the specified `target_ids` at `cut_ms`. The LEFT halves keep
/// their original link; the RIGHT halves of any originally linked group
/// share a *fresh* link with each other. Clips whose source-range doesn't
/// actually contain `cut_ms` are silently skipped.
///
/// This is what makes split + delete behave intuitively: splitting a
/// V/A linked pair produces two new linked pairs (left pair and right
/// pair), and deleting one piece only removes its pair — not every piece
/// that traces back to the original link.
pub fn split_clips(
    clips: &mut Vec<Clip>,
    target_ids: &[ClipId],
    cut_ms: i64,
    id_gen: &IdGen,
) -> Vec<ClipId> {
    let targets: Vec<(ClipId, Option<LinkId>)> = target_ids
        .iter()
        .filter_map(|id| {
            let c = clip_by_id(clips, *id)?;
            if !c.contains_cut(cut_ms) {
                return None;
            }
            Some((*id, c.link))
        })
        .collect();

    let mut remap: std::collections::HashMap<LinkId, LinkId> = std::collections::HashMap::new();
    let mut new_ids = Vec::with_capacity(targets.len());

    for (id, orig_link) in targets {
        let Some(new_id) = split_clip(clips, id, cut_ms, id_gen) else {
            continue;
        };
        if let Some(orig) = orig_link {
            let new_link = *remap.entry(orig).or_insert_with(|| id_gen.next_link());
            if let Some(right) = clips.iter_mut().find(|c| c.id == new_id) {
                right.link = Some(new_link);
            }
        }
        new_ids.push(new_id);
    }
    new_ids
}

/// Convenience: split every clip currently intersecting `cut_ms`. Kept
/// in the public API for unit tests + potential future "split all"
/// shortcut; the live app routes through `split_clips` after expanding
/// selection to the full link group.
#[allow(dead_code)]
pub fn split_all_at(clips: &mut Vec<Clip>, cut_ms: i64, id_gen: &IdGen) -> Vec<ClipId> {
    let ids: Vec<ClipId> = clips
        .iter()
        .filter(|c| c.contains_cut(cut_ms))
        .map(|c| c.id)
        .collect();
    split_clips(clips, &ids, cut_ms, id_gen)
}

/// Delete `target` and every clip that shares its link. If `target` is
/// unlinked, only it is removed.
pub fn delete_with_links(clips: &mut Vec<Clip>, target: ClipId) {
    let link = clip_by_id(clips, target).and_then(|c| c.link);
    match link {
        Some(l) => clips.retain(|c| c.link != Some(l)),
        None => clips.retain(|c| c.id != target),
    }
}

/// Break the link on `target` and every clip that shares it.
/// Returns true if anything changed.
pub fn detach_links(clips: &mut [Clip], target: ClipId) -> bool {
    let Some(link) = clip_by_id(clips, target).and_then(|c| c.link) else {
        return false;
    };
    let mut changed = false;
    for c in clips.iter_mut() {
        if c.link == Some(link) {
            c.link = None;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_clip(
        id: u32,
        track: u32,
        cut_start: i64,
        src_start: i64,
        src_end: i64,
        link: Option<u32>,
    ) -> Clip {
        Clip {
            id: ClipId(id),
            track_id: TrackId(track),
            cut_start_ms: cut_start,
            src_start_ms: src_start,
            src_end_ms: src_end,
            link: link.map(LinkId),
            gain: 1.0,
            speed: 1.0,
            transform: Transform::default(),
        }
    }

    #[test]
    fn cut_end_and_duration() {
        let c = mk_clip(0, 0, 100, 200, 500, None);
        assert_eq!(c.src_duration_ms(), 300);
        assert_eq!(c.cut_end_ms(), 400);
        assert!(c.contains_cut(100));
        assert!(c.contains_cut(399));
        assert!(!c.contains_cut(400));
        assert!(!c.contains_cut(99));
    }

    #[test]
    fn cut_to_src_only_inside() {
        let c = mk_clip(0, 0, 100, 200, 500, None);
        assert_eq!(c.cut_to_src(100), Some(200));
        assert_eq!(c.cut_to_src(150), Some(250));
        assert_eq!(c.cut_to_src(399), Some(499));
        assert_eq!(c.cut_to_src(400), None);
        assert_eq!(c.cut_to_src(99), None);
    }

    #[test]
    fn project_cut_duration_is_max_cut_end() {
        let clips = vec![
            mk_clip(0, 0, 0, 0, 100, None),
            mk_clip(1, 0, 100, 200, 300, None),
            mk_clip(2, 1, 0, 0, 250, None),
        ];
        assert_eq!(project_cut_duration(&clips), 250);
    }

    #[test]
    fn split_clip_preserves_link_on_both_halves() {
        let ids = IdGen::default();
        let mut clips = vec![mk_clip(0, 0, 0, 100, 400, Some(7))];
        let new_id = split_clip(&mut clips, ClipId(0), 200, &ids).expect("should split");
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].src_end_ms, 300);
        assert_eq!(clips[1].id, new_id);
        assert_eq!(clips[1].src_start_ms, 300);
        assert_eq!(clips[1].cut_start_ms, 200);
        assert_eq!(clips[0].link, clips[1].link);
        assert_eq!(clips[0].link, Some(LinkId(7)));
    }

    #[test]
    fn split_at_clip_boundary_is_noop() {
        let ids = IdGen::default();
        let mut clips = vec![mk_clip(0, 0, 0, 100, 400, None)];
        // cut_ms = 0 is the very start; cut_ms = 300 is past the end.
        assert!(split_clip(&mut clips, ClipId(0), 0, &ids).is_none());
        assert!(split_clip(&mut clips, ClipId(0), 300, &ids).is_none());
        assert_eq!(clips.len(), 1);
    }

    // IdGen starts allocating from 0, so the initial clips in these
    // tests use 100+ IDs and link 99 to avoid colliding with the IDs
    // split_all_at will mint for the right-halves.
    #[test]
    fn split_all_at_splits_each_track_and_remaps_right_links() {
        let ids = IdGen::default();
        let mut clips = vec![
            mk_clip(100, 0, 0, 0, 400, Some(99)),
            mk_clip(101, 1, 0, 0, 400, Some(99)),
        ];
        let new_ids = split_all_at(&mut clips, 200, &ids);
        assert_eq!(new_ids.len(), 2);
        assert_eq!(clips.len(), 4);

        let lefts: Vec<&Clip> = clips.iter().filter(|c| c.cut_start_ms == 0).collect();
        assert_eq!(lefts.len(), 2);
        assert_eq!(lefts[0].link, Some(LinkId(99)));
        assert_eq!(lefts[1].link, Some(LinkId(99)));

        let rights: Vec<&Clip> = clips.iter().filter(|c| c.cut_start_ms == 200).collect();
        assert_eq!(rights.len(), 2);
        assert!(rights[0].link.is_some());
        assert_eq!(rights[0].link, rights[1].link);
        assert_ne!(rights[0].link, Some(LinkId(99)));
    }

    #[test]
    fn split_all_at_keeps_unlinked_clips_unlinked() {
        let ids = IdGen::default();
        let mut clips = vec![
            mk_clip(100, 0, 0, 0, 400, None),
            mk_clip(101, 1, 0, 0, 400, None),
        ];
        let _ = split_all_at(&mut clips, 200, &ids);
        assert!(clips.iter().all(|c| c.link.is_none()));
    }

    #[test]
    fn split_then_delete_removes_only_one_pair() {
        // Regression: a linked V/A pair, split at the middle, then deleting
        // one right-half should remove only the right pair — not all four.
        let ids = IdGen::default();
        let mut clips = vec![
            mk_clip(100, 0, 0, 0, 400, Some(99)),
            mk_clip(101, 1, 0, 0, 400, Some(99)),
        ];
        let new_ids = split_all_at(&mut clips, 200, &ids);
        assert_eq!(clips.len(), 4);
        delete_with_links(&mut clips, new_ids[0]);
        assert_eq!(clips.len(), 2);
        assert!(clips.iter().all(|c| c.cut_start_ms == 0));
    }

    #[test]
    fn delete_with_links_removes_partners() {
        let mut clips = vec![
            mk_clip(0, 0, 0, 0, 100, Some(1)),
            mk_clip(1, 1, 0, 0, 100, Some(1)),
            mk_clip(2, 1, 100, 100, 200, None),
        ];
        delete_with_links(&mut clips, ClipId(0));
        // Both linked clips (ids 0, 1) are gone; unlinked clip 2 remains.
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, ClipId(2));
    }

    #[test]
    fn delete_unlinked_only_removes_target() {
        let mut clips = vec![
            mk_clip(0, 0, 0, 0, 100, None),
            mk_clip(1, 1, 0, 0, 100, None),
        ];
        delete_with_links(&mut clips, ClipId(0));
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, ClipId(1));
    }

    #[test]
    fn detach_links_clears_whole_group() {
        let mut clips = vec![
            mk_clip(0, 0, 0, 0, 100, Some(1)),
            mk_clip(1, 1, 0, 0, 100, Some(1)),
            mk_clip(2, 1, 100, 100, 200, Some(2)),
        ];
        assert!(detach_links(&mut clips, ClipId(0)));
        assert!(clips[0].link.is_none());
        assert!(clips[1].link.is_none());
        // Clip 2 is in a different link group, untouched.
        assert_eq!(clips[2].link, Some(LinkId(2)));
    }

    #[test]
    fn detach_on_unlinked_is_noop() {
        let mut clips = vec![mk_clip(0, 0, 0, 0, 100, None)];
        assert!(!detach_links(&mut clips, ClipId(0)));
    }
}
