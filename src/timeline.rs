// Custom timeline widget. Click/drag to seek; In/Out marks are drawn as
// vertical lines with a tinted band between them. Major tick marks every
// 10 seconds, minor ticks every 1 second when the zoom permits.
//
// `scrubber()` is a slimmer media-player-style progress bar with a circular
// thumb for playback control. Same TimelineState so both stay in sync.

use crate::theme;
use crate::track::{Clip, ClipId, Track, TrackId, TrackKind};
use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::collections::HashSet;

pub struct TimelineState {
    pub duration_ms: i64,
    pub playhead_ms: i64,
    pub in_ms: Option<i64>,
    pub out_ms: Option<i64>,
    /// Simple Clip V1-mute state (audio on the source video). Drives the
    /// glyph on the V1 track header.
    pub audio_muted: bool,
}

#[derive(Default)]
pub struct TimelineOutput {
    pub seeked: bool,
    pub dragging: bool,
    /// Set when the user clicked the V1 mute glyph.
    pub mute_toggled: bool,
}

#[derive(Default)]
pub struct ScrubberOutput {
    pub seeked: bool,
    pub dragging: bool,
}

/// Simple Clip timeline. Same structure as a single-lane `show_multitrack`:
/// a `V1` header column, a recessed lane with the full source rendered as
/// a clip block, and In/Out flag handles on top. The block is always
/// drawn — Simple Clip exports a sub-range of the source, not a built-up
/// timeline, so the source is "there" the moment a file loads.
pub fn show(ui: &mut Ui, state: &mut TimelineState) -> (Response, TimelineOutput) {
    let height = LANE_H + LABEL_BAND_H + 14.0;
    let desired = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let mut out = TimelineOutput::default();

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, theme::BG_PREVIEW);

    let band_rect = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - LABEL_BAND_H),
        rect.right_bottom(),
    );
    painter.rect_filled(band_rect, 0.0, theme::BG_HEADER);

    if state.duration_ms <= 0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Open a video to begin",
            FontId::proportional(13.0),
            theme::TEXT_MUTED,
        );
        painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, theme::BORDER));
        return (response, out);
    }

    let lane_area_left = rect.left() + TRACK_HEADER_W;
    let lane_area_right = rect.right() - 4.0;
    let lane_area_width = (lane_area_right - lane_area_left).max(10.0);
    let lane_top = rect.top() + 6.0;
    let lane_bot = lane_top + LANE_H;

    // Header column: separator + mute glyph. Simple Clip drops the "V1"
    // label — the column is just there to anchor the mute toggle and
    // keep horizontal alignment consistent with Advanced mode.
    painter.line_segment(
        [
            egui::pos2(lane_area_left - 1.0, rect.top() + 4.0),
            egui::pos2(lane_area_left - 1.0, rect.bottom() - LABEL_BAND_H),
        ],
        Stroke::new(1.0_f32, theme::BORDER),
    );
    let header_center_y = (lane_top + lane_bot) * 0.5;
    let mute_size = 18.0;
    let mute_rect = Rect::from_min_size(
        egui::pos2(
            rect.left() + (TRACK_HEADER_W - mute_size) * 0.5,
            header_center_y - mute_size * 0.5,
        ),
        egui::vec2(mute_size, mute_size),
    );
    let mute_resp = ui.interact(mute_rect, egui::Id::new("simple_v1_mute"), Sense::click());
    let mute_resp = mute_resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if state.audio_muted {
            "Unmute audio"
        } else {
            "Mute audio"
        });
    crate::widgets::paint_icon_button(
        ui,
        mute_rect.expand(4.0),
        &mute_resp,
        if state.audio_muted {
            crate::widgets::Icon::SpeakerMuted
        } else {
            crate::widgets::Icon::Speaker
        },
        state.audio_muted,
        theme::ACCENT_ERR,
    );
    if mute_resp.clicked() {
        out.mute_toggled = true;
    }

    // Lane rail.
    let lane_rect = Rect::from_min_max(
        egui::pos2(lane_area_left, lane_top),
        egui::pos2(lane_area_right, lane_bot),
    );
    painter.rect_filled(lane_rect, 4.0, Color32::from_rgb(12, 14, 18));

    // Full-source clip block — always rendered (the source is always
    // there in Simple Clip mode). Uses a warm teal fill that reads
    // visually distinct from Advanced's cool slate-blue.
    let clip_fill = Color32::from_rgb(60, 92, 84);
    let block = Rect::from_min_max(
        egui::pos2(lane_rect.left(), lane_rect.top() + 1.0),
        egui::pos2(lane_rect.right(), lane_rect.bottom() - 1.0),
    );
    painter.rect_filled(block, 3.0, clip_fill);
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(lane_rect.left() + 3.0, block.top()),
            egui::pos2(lane_rect.right() - 3.0, block.top() + 1.0),
        ),
        0.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, 18),
    );

    // In/Out sub-range: accent outline over the selected portion so the
    // user sees what will actually export, without hiding the rest of the
    // source.
    let dur = state.duration_ms.max(1) as f32;
    if let (Some(in_ms), Some(out_ms)) = (state.in_ms, state.out_ms) {
        let lo = in_ms.min(out_ms);
        let hi = in_ms.max(out_ms);
        let x0 = (lane_area_left + (lo as f32 / dur) * lane_area_width).max(lane_area_left);
        let x1 = (lane_area_left + (hi as f32 / dur) * lane_area_width).min(lane_area_right);
        if x1 > x0 + 1.0 {
            let sel = Rect::from_min_max(
                egui::pos2(x0, lane_rect.top() + 1.0),
                egui::pos2(x1, lane_rect.bottom() - 1.0),
            );
            painter.rect_stroke(sel, 3.0, Stroke::new(2.0_f32, theme::ACCENT));
        }
    }

    // Ticks + In/Out flags use the lane's horizontal bounds.
    let lane_x_rect = Rect::from_min_max(
        egui::pos2(lane_area_left, rect.top()),
        egui::pos2(lane_area_right, rect.bottom()),
    );
    draw_ticks(&painter, lane_x_rect, LABEL_BAND_H, state.duration_ms);
    if let Some(in_ms) = state.in_ms {
        draw_handle(
            &painter,
            lane_x_rect,
            in_ms,
            state.duration_ms,
            theme::ACCENT_IN,
            "I",
        );
    }
    if let Some(out_ms) = state.out_ms {
        draw_handle(
            &painter,
            lane_x_rect,
            out_ms,
            state.duration_ms,
            theme::ACCENT_OUT,
            "O",
        );
    }

    // Playhead — same thickness + triangle as show_multitrack.
    let ph_frac = (state.playhead_ms as f32 / dur).clamp(0.0, 1.0);
    let ph_x = lane_area_left + ph_frac * lane_area_width;
    painter.line_segment(
        [
            egui::pos2(ph_x, rect.top() + 4.0),
            egui::pos2(ph_x, rect.bottom() - LABEL_BAND_H),
        ],
        Stroke::new(2.0_f32, theme::PLAYHEAD),
    );
    let triangle = vec![
        egui::pos2(ph_x - 7.0, rect.top()),
        egui::pos2(ph_x + 7.0, rect.top()),
        egui::pos2(ph_x, rect.top() + 10.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        triangle,
        theme::PLAYHEAD,
        Stroke::NONE,
    ));

    // Bare-lane click/drag scrubs (Simple Clip has no clips to drag).
    if (response.clicked() || response.dragged())
        && response
            .interact_pointer_pos()
            .is_some_and(|p| p.x >= lane_area_left)
    {
        if let Some(pos) = response.interact_pointer_pos() {
            let frac = ((pos.x - lane_area_left) / lane_area_width).clamp(0.0, 1.0);
            state.playhead_ms = (frac as f64 * state.duration_ms as f64) as i64;
            out.seeked = true;
        }
    }
    out.dragging = response.dragged();

    painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, theme::BORDER));
    (response, out)
}

pub struct MultiTrackTimelineState<'a> {
    pub tracks: &'a [Track],
    pub clips: &'a [Clip],
    pub playhead_cut_ms: i64,
    pub cut_duration_ms: i64,
    pub selected_clips: &'a HashSet<ClipId>,
}

#[derive(Default)]
pub struct MultiTrackOutput {
    pub seeked: bool,
    /// True only while the user is actively dragging the PLAYHEAD handle.
    /// Clip drags don't count — those don't move the playhead, so mpv
    /// position mirroring should keep running during them.
    pub dragging: bool,
    /// Set when a clip was clicked. `additive` is true if the click was
    /// made with Ctrl/Cmd held — caller should toggle that clip in the
    /// selection rather than replace it.
    pub clicked_clip: Option<ClickedClip>,
    /// Set when a track-header mute glyph was clicked.
    pub mute_toggled: Option<TrackId>,
    /// Set if the user clicked bare lane background without a modifier
    /// (so caller can clear the selection).
    pub clicked_empty: bool,
    /// Set on the frame a clip drag begins. The f32 is cut-ms per px,
    /// used by the snap thresholds in the app layer.
    pub clip_drag_started: Option<(ClipId, f32)>,
    /// Set every frame of a drag with the (clip, delta_ms since last frame).
    pub clip_drag_delta: Option<(ClipId, i64)>,
    /// Set on the frame the drag ends — caller should clear its session state.
    pub clip_drag_ended: bool,
    /// Set when the V1 (video) header mute glyph is clicked in the
    /// linked-audio collapsed state. Caller should toggle every audio
    /// track's muted state together so the "implied single track" stays
    /// in sync.
    pub linked_audio_mute_toggled: bool,
}

#[derive(Clone, Copy)]
pub struct ClickedClip {
    pub id: ClipId,
    pub additive: bool,
}

const TRACK_HEADER_W: f32 = 92.0;
const LANE_H: f32 = 50.0;
const LANE_GAP: f32 = 6.0;
const LABEL_BAND_H: f32 = 22.0;

/// Multi-track timeline. Each track gets its own lane; clips sit on the
/// lane as flat slate-colored blocks. The selected clip gets a thicker
/// accent outline. A single playhead spans all lanes.
pub fn show_multitrack(
    ui: &mut Ui,
    state: &mut MultiTrackTimelineState,
) -> (Response, MultiTrackOutput) {
    // Audio lanes stay hidden while their clips are still linked to the
    // video — the user sees a single combined V+A row. The lane appears
    // as soon as ANY clip on that track has been detached (link cleared).
    let visible_tracks: Vec<&Track> = state.tracks.iter().collect();

    let num_tracks = visible_tracks.len().max(1) as f32;
    let height = LANE_H * num_tracks + LANE_GAP * (num_tracks - 1.0).max(0.0) + LABEL_BAND_H + 8.0;
    let desired = Vec2::new(ui.available_width(), height);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let mut out = MultiTrackOutput::default();

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, theme::BG_PREVIEW);

    // Time-label band at the bottom.
    let band_rect = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - LABEL_BAND_H),
        rect.right_bottom(),
    );
    painter.rect_filled(band_rect, 0.0, theme::BG_HEADER);

    if state.cut_duration_ms <= 0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Nothing on the timeline yet",
            FontId::proportional(13.0),
            theme::TEXT_MUTED,
        );
        painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, theme::BORDER));
        return (response, out);
    }

    let lane_area_left = rect.left() + TRACK_HEADER_W;
    let lane_area_right = rect.right() - 4.0;
    let lane_area_width = (lane_area_right - lane_area_left).max(10.0);

    // Header column separator.
    painter.line_segment(
        [
            egui::pos2(lane_area_left - 1.0, rect.top() + 4.0),
            egui::pos2(lane_area_left - 1.0, rect.bottom() - LABEL_BAND_H),
        ],
        Stroke::new(1.0_f32, theme::BORDER),
    );

    let clip_fill_v = Color32::from_rgb(54, 68, 92);
    let clip_fill_a = Color32::from_rgb(58, 74, 70);
    let lane_bg = Color32::from_rgb(12, 14, 18);

    for (i, track) in visible_tracks.iter().enumerate() {
        let lane_top = rect.top() + 6.0 + i as f32 * (LANE_H + LANE_GAP);
        let lane_bot = lane_top + LANE_H;

        // Header: track name + mute glyph.
        let header_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 6.0, lane_top),
            egui::pos2(lane_area_left - 4.0, lane_bot),
        );
        let name_color = if track.muted {
            theme::TEXT_MUTED
        } else {
            theme::TEXT_PRIMARY
        };
        painter.text(
            egui::pos2(header_rect.left() + 6.0, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &track.name,
            FontId::proportional(13.0),
            name_color,
        );
        // Mute glyph.
        // - Audio tracks: per-track mute.
        // - Video tracks: only the V row in the collapsed-audio state.
        //   That click toggles every audio track at once.
        let show_mute_glyph = match track.kind {
            TrackKind::Audio => true,
            TrackKind::Video => true,
        };
        if show_mute_glyph {
            let muted_display = match track.kind {
                TrackKind::Audio => track.muted,
                TrackKind::Video => track.muted,
            };
            let mute_size = 18.0;
            let mute_rect = Rect::from_min_size(
                egui::pos2(
                    header_rect.right() - mute_size,
                    header_rect.center().y - mute_size * 0.5,
                ),
                egui::vec2(mute_size, mute_size),
            );
            let mute_resp =
                ui.interact(mute_rect, egui::Id::new(("mute", track.id)), Sense::click());
            let glyph_color = if muted_display {
                theme::ACCENT_ERR
            } else if mute_resp.hovered() {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            };
            let mute_text = if track.kind == TrackKind::Video {
                if muted_display {
                    "○"
                } else {
                    "◉"
                }
            } else if muted_display {
                "🔇"
            } else {
                "🔊"
            };
            painter.text(
                mute_rect.center(),
                egui::Align2::CENTER_CENTER,
                mute_text,
                FontId::proportional(13.0),
                glyph_color,
            );
            if mute_resp.clicked() {
                match track.kind {
                    TrackKind::Audio => out.mute_toggled = Some(track.id),
                    TrackKind::Video => out.mute_toggled = Some(track.id),
                }
            }
        }

        // Lane background rail.
        let lane_rect = Rect::from_min_max(
            egui::pos2(lane_area_left, lane_top),
            egui::pos2(lane_area_right, lane_bot),
        );
        painter.rect_filled(lane_rect, 4.0, lane_bg);

        // Clips on this lane. Sort first so we can detect adjacency for
        // the split-gap below.
        let track_fill = match track.kind {
            TrackKind::Video => clip_fill_v,
            TrackKind::Audio => clip_fill_a,
        };
        let mut t_clips: Vec<&Clip> = state
            .clips
            .iter()
            .filter(|c| c.track_id == track.id)
            .collect();
        t_clips.sort_by_key(|c| c.cut_start_ms);

        // Gap between adjacent clips on the SAME track — the only visual
        // cue for a split. Big enough that you can spot it at a glance,
        // small enough that it doesn't eat clip width on dense cuts.
        const CLIP_GAP: f32 = 14.0;

        for (idx, clip) in t_clips.iter().enumerate() {
            let cut_w = state.cut_duration_ms.max(1) as f32;
            let mut x0 = lane_area_left + (clip.cut_start_ms as f32 / cut_w) * lane_area_width;
            let mut x1 = lane_area_left + (clip.cut_end_ms() as f32 / cut_w) * lane_area_width;
            // Pull the edge in by half a gap on any side that has an
            // adjacent clip (so the gap visually splits the two).
            if idx > 0 && t_clips[idx - 1].cut_end_ms() == clip.cut_start_ms {
                x0 += CLIP_GAP * 0.5;
            }
            if idx + 1 < t_clips.len() && t_clips[idx + 1].cut_start_ms == clip.cut_end_ms() {
                x1 -= CLIP_GAP * 0.5;
            }
            let x0 = x0.max(lane_area_left);
            let x1 = x1.min(lane_area_right);
            if x1 <= x0 + 1.0 {
                continue;
            }

            let block = Rect::from_min_max(
                egui::pos2(x0, lane_top + 2.0),
                egui::pos2(x1, lane_bot - 2.0),
            );
            let dim = track.muted;
            let fill = if dim {
                track_fill.linear_multiply(0.55)
            } else {
                track_fill
            };
            painter.rect_filled(block, 3.0, fill);
            // Subtle top highlight.
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(x0 + 3.0, block.top()),
                    egui::pos2(x1 - 3.0, block.top() + 1.0),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 18),
            );
            // Selection outline.
            if state.selected_clips.contains(&clip.id) {
                painter.rect_stroke(block, 3.0, Stroke::new(2.0_f32, theme::ACCENT));
            }

            // Click + drag hit-test. Click selects (with Ctrl/Cmd =
            // additive). Drag moves the clip along the timeline; caller
            // applies the delta to the clip + its linked partners.
            let clip_resp = ui.interact(
                block,
                egui::Id::new(("clip", clip.id)),
                Sense::click_and_drag(),
            );
            if clip_resp.clicked() {
                let additive = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                out.clicked_clip = Some(ClickedClip {
                    id: clip.id,
                    additive,
                });
            }
            if clip_resp.drag_started() {
                let cut_per_px = state.cut_duration_ms.max(1) as f32 / lane_area_width;
                out.clip_drag_started = Some((clip.id, cut_per_px));
            }
            if clip_resp.dragged() {
                let delta_px = clip_resp.drag_delta().x;
                if delta_px != 0.0 {
                    let cut_w = state.cut_duration_ms.max(1) as f32;
                    let delta_ms = (delta_px / lane_area_width * cut_w) as i64;
                    if delta_ms != 0 {
                        out.clip_drag_delta = Some((clip.id, delta_ms));
                    }
                }
                if clip_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            } else if clip_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if clip_resp.drag_stopped() {
                out.clip_drag_ended = true;
            }
        }
    }

    // Time ticks (in the label band).
    let time_rect = Rect::from_min_max(egui::pos2(lane_area_left, rect.top()), rect.right_bottom());
    draw_ticks(&painter, time_rect, LABEL_BAND_H, state.cut_duration_ms);

    // Playhead — thicker white line + bigger triangle head. The visible
    // line is 2.5px but a wider invisible hit area below makes it easy
    // to grab.
    let cut_w = state.cut_duration_ms.max(1) as f32;
    let ph_frac = (state.playhead_cut_ms as f32 / cut_w).clamp(0.0, 1.0);
    let ph_x = lane_area_left + ph_frac * lane_area_width;
    let lane_area_top = rect.top() + 4.0;
    let lane_area_bot = rect.bottom() - LABEL_BAND_H;
    painter.line_segment(
        [
            egui::pos2(ph_x, lane_area_top),
            egui::pos2(ph_x, lane_area_bot),
        ],
        Stroke::new(2.5_f32, Color32::WHITE),
    );
    let triangle = vec![
        egui::pos2(ph_x - 8.0, rect.top()),
        egui::pos2(ph_x + 8.0, rect.top()),
        egui::pos2(ph_x, rect.top() + 11.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        triangle,
        Color32::WHITE,
        Stroke::NONE,
    ));

    // Wide invisible hit area for scrubbing. 18px gives a forgiving grab
    // even at small zoom levels — clip blocks underneath still get their
    // clicks first because clip rects are foreground (allocated later
    // would be foreground; ui.interact ordering: later wins).
    const PH_HIT_W: f32 = 18.0;
    let ph_hit = Rect::from_min_max(
        egui::pos2(ph_x - PH_HIT_W * 0.5, rect.top()),
        egui::pos2(ph_x + PH_HIT_W * 0.5, lane_area_bot),
    );
    let ph_resp = ui.interact(ph_hit, egui::Id::new("playhead_grab"), Sense::drag());
    if ph_resp.hovered() || ph_resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if ph_resp.dragged() {
        if let Some(pos) = ph_resp.interact_pointer_pos() {
            let frac = ((pos.x - lane_area_left) / lane_area_width).clamp(0.0, 1.0);
            state.playhead_cut_ms = (frac as f64 * state.cut_duration_ms as f64) as i64;
            out.seeked = true;
            out.dragging = true;
        }
    }

    // Bare-lane CLICK still seeks (and clears selection if no modifier).
    // Drag no longer scrubs — that's the playhead handle's job now.
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if pos.x >= lane_area_left {
                let frac = ((pos.x - lane_area_left) / lane_area_width).clamp(0.0, 1.0);
                state.playhead_cut_ms = (frac as f64 * state.cut_duration_ms as f64) as i64;
                out.seeked = true;
                let modifier =
                    ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift);
                if !modifier {
                    out.clicked_empty = true;
                }
            }
        }
    }

    painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, theme::BORDER));
    (response, out)
}

/// Slim media-player-style scrubber. Click or drag the handle to seek.
pub fn scrubber(ui: &mut Ui, state: &mut TimelineState) -> ScrubberOutput {
    let desired = Vec2::new(ui.available_width(), 28.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let mut out = ScrubberOutput::default();

    let painter = ui.painter_at(rect);
    let track_h = 6.0;
    // Inset the track horizontally by the handle radius so the handle
    // can sit at frac=0 (or 1) without poking outside the widget bounds.
    let handle_clearance = 12.0;
    let track_left = rect.left() + handle_clearance;
    let track_right = rect.right() - handle_clearance;
    let track = Rect::from_min_max(
        egui::pos2(track_left, rect.center().y - track_h * 0.5),
        egui::pos2(track_right, rect.center().y + track_h * 0.5),
    );
    let radius = track_h * 0.5;
    painter.rect_filled(track, radius, theme::BG_PREVIEW);

    if state.duration_ms <= 0 {
        return out;
    }
    let frac = (state.playhead_ms as f32 / state.duration_ms as f32).clamp(0.0, 1.0);
    let track_w = track.width();

    // Filled progress portion
    let progress = Rect::from_min_size(track.min, Vec2::new(track_w * frac, track.height()));
    painter.rect_filled(progress, radius, theme::ACCENT);

    // Draggable handle
    let handle_x = track.left() + frac * track_w;
    let handle_pos = egui::pos2(handle_x, rect.center().y);
    let handle_r = if response.hovered() || response.dragged() {
        10.0
    } else {
        8.0
    };
    painter.circle_filled(handle_pos, handle_r, theme::ACCENT);
    painter.circle_stroke(handle_pos, handle_r, Stroke::new(1.5_f32, Color32::WHITE));

    if response.clicked() || response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let f = ((pos.x - track.left()) / track_w).clamp(0.0, 1.0);
            state.playhead_ms = (f as f64 * state.duration_ms as f64) as i64;
            out.seeked = true;
        }
    }
    out.dragging = response.dragged();
    out
}

fn x_of(rect: Rect, ms: i64, duration_ms: i64) -> f32 {
    let frac = (ms as f64 / duration_ms.max(1) as f64).clamp(0.0, 1.0);
    rect.left() + frac as f32 * rect.width()
}

fn draw_ticks(painter: &egui::Painter, rect: Rect, label_band_h: f32, duration_ms: i64) {
    let duration_s = duration_ms as f64 / 1000.0;
    let pixels_per_second = rect.width() as f64 / duration_s.max(0.001);

    // Choose major tick interval so labels are roughly 90–120 px apart.
    let major_interval_s = pick_interval(pixels_per_second, 100.0);
    let minor_interval_s = (major_interval_s / 5.0).max(1.0);

    let label_top = rect.bottom() - label_band_h;
    let upper_bottom = label_top;
    let minor_color = theme::BORDER;
    let major_color = theme::TEXT_MUTED;

    // Minor ticks: small marks at the top of the label band
    if pixels_per_second * minor_interval_s > 6.0 {
        let mut t = 0.0_f64;
        while t <= duration_s {
            let x = rect.left() + (t / duration_s) as f32 * rect.width();
            painter.line_segment(
                [egui::pos2(x, label_top), egui::pos2(x, label_top + 4.0)],
                Stroke::new(1.0_f32, minor_color),
            );
            t += minor_interval_s;
        }
    }

    // Major ticks: vertical line across the upper area + bold label
    let mut t = 0.0_f64;
    while t <= duration_s {
        let x = rect.left() + (t / duration_s) as f32 * rect.width();
        // Long vertical tick line in upper area
        painter.line_segment(
            [
                egui::pos2(x, upper_bottom),
                egui::pos2(x, upper_bottom - 10.0),
            ],
            Stroke::new(1.5_f32, major_color),
        );
        // Tick inside label band
        painter.line_segment(
            [egui::pos2(x, label_top), egui::pos2(x, label_top + 8.0)],
            Stroke::new(1.5_f32, major_color),
        );
        // Label, larger and more readable
        painter.text(
            egui::pos2(x + 5.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format_short_time(t as i64),
            FontId::monospace(13.0),
            theme::TEXT_PRIMARY,
        );
        t += major_interval_s;
    }
}

fn pick_interval(pixels_per_second: f64, target_px: f64) -> f64 {
    let target_seconds = target_px / pixels_per_second;
    // Snap to a "nice" interval
    for &candidate in &[
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
    ] {
        if candidate >= target_seconds {
            return candidate;
        }
    }
    3600.0
}

fn format_short_time(seconds: i64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn draw_handle(
    painter: &egui::Painter,
    rect: Rect,
    ms: i64,
    duration_ms: i64,
    color: Color32,
    label: &str,
) {
    const LABEL_BAND_H: f32 = 22.0;
    const FLAG_W: f32 = 14.0;
    const FLAG_H: f32 = 14.0;

    let upper_bot = rect.bottom() - LABEL_BAND_H;
    let x = x_of(rect, ms, duration_ms);
    // Thin stem within the upper area only.
    painter.line_segment(
        [egui::pos2(x, rect.top() + 4.0), egui::pos2(x, upper_bot)],
        Stroke::new(1.5_f32, color),
    );
    // Clamp the flag rect so it stays fully visible even when the mark
    // is at the very start (x = rect.left()) or end of the timeline.
    let flag_left = (x - FLAG_W * 0.5).clamp(rect.left() + 1.0, rect.right() - FLAG_W - 1.0);
    let flag = Rect::from_min_size(
        egui::pos2(flag_left, rect.top() + 10.0),
        egui::vec2(FLAG_W, FLAG_H),
    );
    painter.rect_filled(flag, 2.5, color);
    painter.text(
        flag.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::monospace(10.0),
        Color32::BLACK,
    );
}
