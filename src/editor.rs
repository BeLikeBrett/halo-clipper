use super::*;

#[derive(Clone, Debug)]
pub(super) enum EditorDrag {
    Asset(TrackId),
    Clip { id: ClipId, grab_ms: i64 },
    Track(TrackId),
}
const LANE_HEIGHT: f32 = 54.0;
const RULER_HEIGHT: f32 = 28.0;
/// Track-header column width (name, kind, monitor toggle).
const HEADER_W: f32 = 128.0;
/// Breathing room after the last second so the end handle isn't flush.
const RIGHT_PAD: f32 = 10.0;
/// Vertical inset of a clip inside its lane.
const CLIP_INSET: f32 = 4.0;
/// Height of the strip holding the horizontal scrollbar.
const SCROLLBAR_H: f32 = 12.0;

/// Slim horizontal scrollbar for a zoomed timeline, aligned with the lanes.
/// The playhead shows as a tick on the track so it can be found from any
/// view. Dragging the thumb pans 1:1; clicking the track glides there.
/// Returns a new view start while dragging.
fn timeline_scrollbar(
    ui: &mut egui::Ui,
    strip: egui::Rect,
    lanes: Lanes,
    view: TimelineZoom,
    duration: i64,
    playhead: i64,
) -> Option<i64> {
    let zoomed = view.zoom > 1.001;
    let shown = ui
        .ctx()
        .animate_bool_with_time(ui.id().with("scrollbar_shown"), zoomed, 0.18);
    if shown <= 0.0 {
        return None;
    }
    let track = egui::Rect::from_min_max(
        egui::pos2(lanes.left, strip.top()),
        egui::pos2(lanes.right(), strip.bottom()),
    );
    let span = view.span(duration);
    let scale = track.width() / duration.max(1) as f32;
    let thumb_w = (span as f32 * scale).max(28.0).min(track.width());
    // Thumb travel maps onto the scrollable range, so a min-width thumb
    // still reaches both ends.
    let travel = (track.width() - thumb_w).max(1.0);
    let max_start = (duration - span).max(1) as f32;
    let thumb_x = track.left() + view.start as f32 / max_start * travel;
    let thumb = egui::Rect::from_min_size(
        egui::pos2(thumb_x, track.top()),
        egui::vec2(thumb_w, track.height()),
    );
    let track_response = ui.interact(track, ui.id().with("scrollbar_track"), egui::Sense::click());
    let thumb_response = ui
        .interact(thumb, ui.id().with("scrollbar_thumb"), egui::Sense::drag())
        .on_hover_cursor(egui::CursorIcon::Grab);
    let hot = thumb_response.hovered() || thumb_response.dragged() || track_response.hovered();
    let grow = ui
        .ctx()
        .animate_bool_with_time(ui.id().with("scrollbar_hot"), hot, 0.12);
    let thickness = 4.0 + 3.0 * grow;
    let painter = ui.painter_at(strip);
    let bar =
        |r: egui::Rect| egui::Rect::from_center_size(r.center(), egui::vec2(r.width(), thickness));
    painter.rect_filled(
        bar(track),
        thickness * 0.5,
        Color32::from_white_alpha((10.0 * shown) as u8),
    );
    let alpha = (60.0 + 70.0 * grow + if thumb_response.dragged() { 30.0 } else { 0.0 }) * shown;
    painter.rect_filled(
        bar(thumb),
        thickness * 0.5,
        Color32::from_white_alpha(alpha as u8),
    );
    // Playhead tick over the track: inside the thumb when in view.
    let px = if (view.start..=view.start + span).contains(&playhead) {
        thumb_x + (playhead - view.start) as f32 / span.max(1) as f32 * thumb_w
    } else {
        track.left() + playhead as f32 * scale
    };
    painter.line_segment(
        [
            egui::pos2(px, track.center().y - thickness * 0.5 - 2.0),
            egui::pos2(px, track.center().y + thickness * 0.5 + 2.0),
        ],
        Stroke::new(1.5_f32, theme::PLAYHEAD.gamma_multiply(shown)),
    );
    if thumb_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let dx = thumb_response.drag_delta().x;
        if dx != 0.0 {
            ui.data_mut(|d| d.remove::<i64>(ui.id().with("timeline_zoom").with("glide")));
            return Some(view.start + (dx / travel * max_start).round() as i64);
        }
    } else if track_response.clicked() {
        if let Some(pos) = track_response.interact_pointer_pos() {
            // Center the view on the clicked point, gliding there.
            let at = ((pos.x - track.left()) / track.width() * duration as f32) as i64;
            let target = (at - span / 2).clamp(0, (duration - span).max(0));
            ui.data_mut(|d| d.insert_temp(ui.id().with("timeline_zoom").with("glide"), target));
            ui.ctx().request_repaint();
        }
    }
    None
}

/// Horizontal time mapping shared by the ruler and the lanes: the visible
/// window `[start, start + span)` spread across the lane width, plus the
/// top of the first lane for row hit-testing.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lanes {
    pub(super) left: f32,
    pub(super) width: f32,
    top: f32,
    start: i64,
    pub(super) span: i64,
}
impl Lanes {
    /// Whole timeline fitted to the width.
    #[cfg(test)]
    fn new(rect: egui::Rect, duration: i64) -> Self {
        Self::window(rect, 0, duration)
    }
    pub(super) fn window(rect: egui::Rect, start: i64, span: i64) -> Self {
        Self {
            left: rect.left() + HEADER_W,
            width: (rect.width() - HEADER_W - RIGHT_PAD).max(1.0),
            top: rect.top(),
            start,
            span: span.max(1),
        }
    }
    pub(super) fn right(&self) -> f32 {
        self.left + self.width
    }
    pub(super) fn x_of(&self, time: i64) -> f32 {
        self.left + (time - self.start) as f32 / self.span as f32 * self.width
    }
    pub(super) fn time_at(&self, x: f32) -> i64 {
        (self.start as f32 + (x - self.left) / self.width * self.span as f32)
            .round()
            .max(0.0) as i64
    }
    /// Multiples of `step` that fall inside (or just around) the window.
    fn ticks(&self, step: i64) -> impl Iterator<Item = i64> {
        let step = step.max(1);
        let first = self.start / step;
        let last = (self.start + self.span) / step + 1;
        (first..=last).map(move |n| n * step)
    }
}

/// Horizontal zoom of the Advanced timeline. `zoom == 1` fits everything
/// (the default); Ctrl+wheel zooms around the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TimelineZoom {
    zoom: f32,
    start: i64,
}
impl Default for TimelineZoom {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            start: 0,
        }
    }
}
impl TimelineZoom {
    /// Closest useful view: ~0.4 s across the whole lane width.
    const MIN_SPAN_MS: f32 = 400.0;

    fn span(&self, duration: i64) -> i64 {
        (duration as f32 / self.zoom).round().max(1.0) as i64
    }
    fn clamp(&mut self, duration: i64) {
        let max_zoom = (duration as f32 / Self::MIN_SPAN_MS).max(1.0);
        self.zoom = self.zoom.clamp(1.0, max_zoom);
        self.start = self.start.clamp(0, (duration - self.span(duration)).max(0));
    }
    /// Zooms by `factor`, keeping `anchor` at the same fraction of the width.
    fn zoom_at(&mut self, factor: f32, anchor: i64, fraction: f32, duration: i64) {
        self.zoom *= factor;
        self.clamp(duration);
        self.start = anchor - (fraction * self.span(duration) as f32).round() as i64;
        self.clamp(duration);
    }
    fn pan(&mut self, delta_ms: i64, duration: i64) {
        self.start += delta_ms;
        self.clamp(duration);
    }
    /// Pages the view so a moving playhead never runs off-screen.
    fn follow(&mut self, playhead: i64, duration: i64) {
        let span = self.span(duration);
        if playhead < self.start || playhead > self.start + span {
            self.start = playhead - span / 10;
            self.clamp(duration);
        }
    }
}

/// Last frame's lane geometry, for dropping files from outside the app.
#[derive(Clone, Debug)]
pub(super) struct TimelineDropZone {
    area: egui::Rect,
    lanes: Lanes,
    rows_top: f32,
    /// Index in `Project::tracks` of each visible row.
    real: Vec<usize>,
    /// Video lanes among the visible rows (by row), for dropping onto one.
    video: Vec<Option<TrackId>>,
    visual_rows: usize,
    total: usize,
    snaps: Vec<i64>,
}
impl TimelineDropZone {
    /// Drop time (snapped to 0, the playhead and clip ends within 12 px)
    /// and where: the middle of a video lane drops onto it, elsewhere a new
    /// lane opens at the nearest boundary above the audio rows.
    fn spot_and_row(&self, pos: egui::Pos2) -> Option<(i64, FileLane)> {
        if !self.area.contains(pos) {
            return None;
        }
        let raw = self.lanes.time_at(pos.x.max(self.lanes.left));
        let reach = (12.0 / self.lanes.width * self.lanes.span as f32) as i64;
        let start = self
            .snaps
            .iter()
            .copied()
            .filter(|s| (s - raw).abs() <= reach)
            .min_by_key(|s| (s - raw).abs())
            .unwrap_or(raw);
        let rel = ((pos.y - self.rows_top) / LANE_HEIGHT).max(0.0);
        let row = rel.floor() as usize;
        if (0.25..0.75).contains(&(rel - row as f32)) {
            if let Some(Some(lane)) = self.video.get(row) {
                return Some((start, FileLane::Onto(*lane, row)));
            }
        }
        let slot = (rel + 0.5).floor().min(self.visual_rows as f32) as usize;
        Some((start, FileLane::Insert(slot)))
    }
    /// Drop time, the lane to drop onto (if it has room), and the
    /// `Project::tracks` index for a new lane.
    pub(super) fn spot(&self, pos: egui::Pos2) -> Option<(i64, Option<TrackId>, usize)> {
        let (start, lane) = self.spot_and_row(pos)?;
        Some(match lane {
            FileLane::Onto(id, row) => (
                start,
                Some(id),
                self.real.get(row).copied().unwrap_or(self.total),
            ),
            FileLane::Insert(slot) => (
                start,
                None,
                self.real.get(slot).copied().unwrap_or(self.total),
            ),
        })
    }
}

/// Where dropped files go: onto a video lane (and its row), or a new lane
/// at this row boundary.
#[derive(Clone, Copy)]
enum FileLane {
    Onto(TrackId, usize),
    Insert(usize),
}

#[derive(Clone)]
struct Placement {
    start: i64,
    length: i64,
    slot: usize,
    target: Option<TrackId>,
    snap: Option<i64>,
    kind: TrackKind,
    label: String,
}
/// What a vertical drag needs to know about the item being moved.
pub(super) struct LaneAsk<'a> {
    /// Kind of lane the item lives on.
    pub kind: TrackKind,
    /// Lane it came from (`None` for new media).
    pub own: Option<TrackId>,
    /// The item is the only thing on `own`: a new lane right next to it
    /// would change nothing.
    pub alone: bool,
    /// The item fits on this lane at its current time.
    pub fits: &'a dyn Fn(TrackId) -> bool,
}

/// The lane a dragged item lands on. `y` is relative to the lanes' top in
/// the layout as drawn, with last frame's insertion `gap` open. Reading the
/// drawn layout (not the closed one) keeps an open gap under the pointer
/// stable instead of flickering as the lanes slide.
pub(super) fn resolve_lane(
    y: f32,
    gap: Option<usize>,
    tracks: &[Track],
    ask: &LaneAsk,
) -> textui::LaneTarget {
    use textui::LaneTarget::{Lane, New};
    let h = LANE_HEIGHT;
    let visual = tracks.iter().filter(|t| t.kind.is_visual()).count();
    let (lo, hi) = if ask.kind.is_visual() {
        (0, visual)
    } else {
        (visual, tracks.len())
    };
    let gap = gap.filter(|g| (lo..=hi).contains(g));
    if let Some(g) = gap {
        let top = g as f32 * h;
        if (top..top + h).contains(&y) {
            return New(g);
        }
    }
    let y = match gap {
        Some(g) if y >= (g + 1) as f32 * h => y - h,
        _ => y,
    }
    .max(0.0);
    let row = (y / h).floor() as usize;
    let within = y / h - row as f32;
    let fits = |id: TrackId| (ask.fits)(id);
    let decided = match tracks.get(row) {
        None => New(hi),
        // Its own lane keeps it while it fits: horizontal moves with a
        // little vertical wobble never open a lane.
        Some(t) if Some(t.id) == ask.own && fits(t.id) => Lane(t.id),
        Some(t) if t.kind == ask.kind && (0.25..0.75).contains(&within) => {
            if fits(t.id) {
                Lane(t.id)
            } else if ask.kind.is_visual() {
                // Occupied: a new layer on top of it.
                New(row)
            } else {
                New(row + 1)
            }
        }
        Some(_) => New(if within < 0.5 { row } else { row + 1 }),
    };
    match decided {
        New(slot) => {
            let slot = slot.clamp(lo, hi);
            let own_row = ask
                .own
                .and_then(|own| tracks.iter().position(|t| t.id == own));
            match (ask.alone, own_row, ask.own) {
                (true, Some(r), Some(own)) if slot == r || slot == r + 1 => Lane(own),
                _ => New(slot),
            }
        }
        lane => lane,
    }
}

/// The lane a drop would create: a soft accent row with a label in the
/// header column. Drawn under the lanes, which slide aside to reveal it.
fn paint_new_lane(
    painter: &egui::Painter,
    rect: egui::Rect,
    left: f32,
    slot: usize,
    kind: TrackKind,
) {
    let top = rect.top() + slot as f32 * LANE_HEIGHT;
    let row = egui::Rect::from_min_size(
        egui::pos2(rect.left(), top),
        egui::vec2(rect.width(), LANE_HEIGHT),
    );
    painter.rect_filled(row, 0.0, theme::BG_PANEL);
    let well = egui::Rect::from_min_max(
        egui::pos2(left + 4.0, row.top() + 4.0),
        egui::pos2(row.right() - 4.0, row.bottom() - 4.0),
    );
    painter.rect_filled(well, 6.0, theme::ACCENT.gamma_multiply(0.08));
    painter.rect_stroke(
        well,
        6.0,
        Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.45)),
    );
    let name = match kind {
        TrackKind::Video => "New video layer",
        TrackKind::Audio => "New audio layer",
        TrackKind::Text => "New text layer",
    };
    painter.text(
        egui::pos2(rect.left() + 14.0, row.center().y),
        Align2::LEFT_CENTER,
        name,
        theme::medium(11.5),
        theme::ACCENT,
    );
}

/// Eases a drawn position toward `target` (frame-rate independent).
fn glide(ctx: &egui::Context, id: egui::Id, target: f32) -> f32 {
    let dt = ctx.input(|i| i.stable_dt).min(0.05);
    let value = ctx.data_mut(|d| {
        let y = d.get_temp_mut_or_insert_with(id, || target);
        *y += (target - *y) * (1.0 - (-dt * 24.0).exp());
        if (target - *y).abs() < 0.5 {
            *y = target;
        }
        *y
    });
    if value != target {
        ctx.request_repaint();
    }
    value
}
fn ruler_step(duration: i64, width: f32) -> i64 {
    let desired = duration as f32 / (width / 100.0).max(1.0);
    let power = 10.0_f32.powf(desired.max(100.0).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|n| (n * power) as i64)
        .find(|n| *n as f32 >= desired)
        .unwrap_or(1000)
        .max(100)
}
fn ruler_label(t: i64, step: i64) -> String {
    if step < 1000 {
        format!("{}:{:02}.{}", t / 60000, (t / 1000) % 60, (t % 1000) / 100)
    } else {
        format!("{}:{:02}", t / 60000, (t / 1000) % 60)
    }
}
/// Compact clip length for the clip body: "4.2s", "1:05".
fn short_duration(ms: i64) -> String {
    if ms < 60_000 {
        format!("{:.1}s", ms as f32 / 1000.0)
    } else {
        format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)
    }
}
#[allow(clippy::too_many_arguments)]
fn placement(
    p: &Project,
    tracks: &[Track],
    clips: &[Clip],
    payload: &EditorDrag,
    pos: egui::Pos2,
    lanes: Lanes,
    gap: Option<usize>,
    ctx: &egui::Context,
) -> Option<Placement> {
    let (length, grab, kind, exclude, label) = match payload {
        EditorDrag::Asset(id) => {
            let a = p.assets.iter().find(|a| a.id == *id)?;
            (
                a.source.media.duration_ms,
                0,
                if a.source.media.width > 0 {
                    TrackKind::Video
                } else {
                    TrackKind::Audio
                },
                None,
                a.label(),
            )
        }
        EditorDrag::Clip { id, grab_ms } => {
            let c = clips.iter().find(|c| c.id == *id)?;
            let kind = tracks.iter().find(|t| t.id == c.track_id)?.kind;
            (
                c.duration_ms(),
                *grab_ms,
                kind,
                Some(*id),
                p.clip_source(c)
                    .0
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        }
        EditorDrag::Track(_) => return None,
    };
    let mut start = (lanes.time_at(pos.x) - grab).max(0);
    let mut snap = None;
    if !ctx.input(|i| i.modifiers.shift) {
        let threshold = (12.0 / lanes.width * lanes.span as f32).ceil() as i64;
        let mut candidates = vec![
            (0, 0),
            (p.playhead_ms, p.playhead_ms),
            (p.playhead_ms - length, p.playhead_ms),
        ];
        for c in clips.iter().filter(|c| Some(c.id) != exclude) {
            for edge in [c.cut_start_ms, c.cut_end_ms()] {
                candidates.extend([(edge, edge), (edge - length, edge)]);
            }
        }
        if let Some((value, guide)) = candidates
            .into_iter()
            .filter(|(t, _)| *t >= 0 && (start - *t).abs() <= threshold)
            .min_by_key(|(t, _)| (start - *t).abs())
        {
            start = value;
            snap = Some(guide);
        }
    }
    if p.clips.is_empty() {
        if let EditorDrag::Asset(id) = payload {
            let anchor_id = egui::Id::new("first_timeline_placement");
            let bypass = ctx.input(|i| i.modifiers.shift);
            let locked = ctx.data_mut(|data| {
                let anchor = data.get_temp_mut_or_insert_with(anchor_id, || (*id, pos.x, true));
                if anchor.0 != *id {
                    *anchor = (*id, pos.x, true);
                }
                if (pos.x - anchor.1).abs() > 28.0 || bypass {
                    anchor.2 = false;
                }
                anchor.2
            });
            if locked {
                start = 0;
                snap = Some(0);
            }
        }
    }
    let own = exclude.and_then(|id| clips.iter().find(|c| c.id == id).map(|c| c.track_id));
    let fits = |lane: TrackId| {
        !clips.iter().any(|c| {
            c.track_id == lane
                && Some(c.id) != exclude
                && start < c.cut_end_ms()
                && start + length > c.cut_start_ms
        })
    };
    let ask = LaneAsk {
        kind,
        own,
        alone: own.is_some_and(|lane| {
            clips
                .iter()
                .all(|c| c.track_id != lane || Some(c.id) == exclude)
        }),
        fits: &fits,
    };
    let (slot, target) = match resolve_lane(pos.y - lanes.top, gap, tracks, &ask) {
        textui::LaneTarget::Lane(id) => (
            tracks.iter().position(|t| t.id == id).unwrap_or(0),
            Some(id),
        ),
        textui::LaneTarget::New(slot) => (slot, None),
    };
    Some(Placement {
        start,
        length,
        slot,
        target,
        snap,
        kind,
        label,
    })
}

/// Everything `paint_clip` needs to know about one bar.
struct ClipLook<'a> {
    label: &'a str,
    kind: TrackKind,
    thumb: Option<egui::TextureId>,
    selected: bool,
    /// Track disabled for export ("hidden").
    disabled: bool,
    /// Audio silenced in the preview monitor only.
    quiet: bool,
    /// `Some(monitor_muted)` for a V+A compound: draws the attached-audio strip.
    linked_audio: Option<bool>,
    duration_ms: i64,
    speed: f32,
    /// Abuts a neighbour on that side (a split or butt edit).
    join_left: bool,
    join_right: bool,
}

/// Draws one clip bar. Joined sides get a tight radius and a hairline gap
/// so two halves of a split read as separate pieces meeting at a seam,
/// never as one bar overlapping another.
fn paint_clip(painter: &egui::Painter, block: egui::Rect, look: &ClipLook) {
    let (base, edge) = match look.kind {
        TrackKind::Video => (theme::CLIP_VIDEO, theme::CLIP_VIDEO_EDGE),
        TrackKind::Audio => (theme::CLIP_AUDIO, theme::CLIP_AUDIO_EDGE),
        TrackKind::Text => (theme::CLIP_TEXT, theme::CLIP_TEXT_EDGE),
    };
    let dim = |c: Color32| {
        if look.disabled {
            c.gamma_multiply(0.4)
        } else {
            c
        }
    };
    let base = if look.quiet {
        base.gamma_multiply(0.55)
    } else {
        base
    };
    let fill = dim(if look.selected {
        base.linear_multiply(1.18)
    } else {
        base
    });
    let r = |joined: bool| if joined { 1.5 } else { 5.0 };
    let rounding = egui::Rounding {
        nw: r(look.join_left),
        sw: r(look.join_left),
        ne: r(look.join_right),
        se: r(look.join_right),
    };
    painter.rect_filled(block, rounding, fill);
    // One-pixel top highlight: reads as a lit edge, not a second band.
    painter.line_segment(
        [
            egui::pos2(block.left() + rounding.nw.max(2.0), block.top() + 1.5),
            egui::pos2(block.right() - rounding.ne.max(2.0), block.top() + 1.5),
        ],
        Stroke::new(
            1.0_f32,
            Color32::from_white_alpha(if look.disabled { 6 } else { 22 }),
        ),
    );

    let clip = painter.with_clip_rect(block.intersect(painter.clip_rect()));
    // Content sticks to the visible part of a clip scrolled half off-screen.
    let visible_left = block.left().max(painter.clip_rect().left());
    let mut body = block.shrink2(egui::vec2(8.0, 5.0));
    body.min.x = visible_left + 8.0;
    if let Some(muted) = look.linked_audio {
        let strip = egui::Rect::from_min_max(
            egui::pos2(block.left() + 1.0, block.bottom() - 11.0),
            egui::pos2(block.right() - 1.0, block.bottom() - 1.0),
        );
        let strip_fill = if muted {
            theme::CLIP_AUDIO.gamma_multiply(0.35)
        } else {
            theme::CLIP_AUDIO
        };
        clip.rect_filled(
            strip,
            egui::Rounding {
                nw: 0.0,
                ne: 0.0,
                sw: rounding.sw,
                se: rounding.se,
            },
            dim(strip_fill),
        );
        clip.line_segment(
            [strip.left_top(), strip.right_top()],
            Stroke::new(1.0_f32, dim(theme::CLIP_AUDIO_EDGE.gamma_multiply(0.7))),
        );
        body.max.y = strip.top() - 3.0;
    }

    if let Some(texture) = look.thumb.filter(|_| block.width() > 120.0) {
        let h = body.height() + 4.0;
        let thumb = egui::Rect::from_min_size(
            egui::pos2(visible_left + 4.0, body.center().y - h * 0.5),
            egui::vec2(h * 16.0 / 9.0, h),
        );
        clip.image(
            texture,
            thumb,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            if look.disabled {
                Color32::from_gray(90)
            } else {
                Color32::WHITE
            },
        );
        clip.rect_stroke(
            thumb,
            2.0,
            Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
        );
        body.min.x = thumb.right() + 9.0;
    }

    if block.width() > 34.0 {
        let text = clip.with_clip_rect(
            body.expand2(egui::vec2(0.0, 3.0))
                .intersect(clip.clip_rect()),
        );
        let two_lines = body.height() >= 30.0;
        let name_y = if two_lines {
            body.top() + 7.0
        } else {
            body.center().y
        };
        text.text(
            egui::pos2(body.left(), name_y),
            Align2::LEFT_CENTER,
            look.label,
            theme::medium(12.0),
            dim(theme::TEXT_PRIMARY),
        );
        let mut meta = short_duration(look.duration_ms);
        if (look.speed - 1.0).abs() > 0.001 {
            meta = format!("{meta}  ·  {}×", (look.speed * 100.0).round() / 100.0);
        }
        let meta_color = dim(theme::TEXT_SECONDARY.gamma_multiply(0.85));
        if two_lines {
            text.text(
                egui::pos2(body.left(), body.bottom() - 6.0),
                Align2::LEFT_CENTER,
                meta,
                theme::mono(10.0),
                meta_color,
            );
        } else if body.width() > 190.0 {
            text.text(
                egui::pos2(body.right(), body.center().y),
                Align2::RIGHT_CENTER,
                meta,
                theme::mono(10.0),
                meta_color,
            );
        }
    }

    // Inner outline so a selection never spills over a neighbour.
    let (width, color) = if look.selected {
        (1.5, theme::ACCENT)
    } else {
        (1.0, dim(edge.gamma_multiply(0.75)))
    };
    painter.rect_stroke(
        block.shrink(width * 0.5),
        rounding,
        Stroke::new(width, color),
    );
}

/// Edit-point marker on a seam between two abutting clips: a pair of
/// notches pointing at the cut, like the ones on a film splice.
fn paint_seam(painter: &egui::Painter, x: f32, top: f32, bottom: f32) {
    let w = 5.5;
    let h = 7.0;
    let color = theme::TEXT_PRIMARY.gamma_multiply(0.9);
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - w, top),
            egui::pos2(x + w, top),
            egui::pos2(x, top + h),
        ],
        color,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - w, bottom),
            egui::pos2(x, bottom - h),
            egui::pos2(x + w, bottom),
        ],
        color,
        Stroke::NONE,
    ));
}

/// Playhead head: a rounded tab with a pointed tip, big enough to grab
/// like a handle rather than aim at like a hairline.
fn paint_playhead_head(painter: &egui::Painter, x: f32, top: f32, bottom: f32, hot: bool) {
    let half = if hot { 7.5 } else { 6.5 };
    let tip = 6.0;
    let body = egui::Rect::from_min_max(
        egui::pos2(x - half, top),
        egui::pos2(x + half, bottom - tip),
    );
    painter.rect_filled(
        body,
        egui::Rounding {
            nw: 3.0,
            ne: 3.0,
            sw: 0.0,
            se: 0.0,
        },
        theme::PLAYHEAD,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            body.left_bottom(),
            body.right_bottom(),
            egui::pos2(x, bottom),
        ],
        theme::PLAYHEAD,
        Stroke::NONE,
    ));
    // Grip lines: the visual cue that this is a handle.
    for dy in [-2.5, 0.5, 3.5] {
        let y = body.center().y + dy;
        painter.line_segment(
            [egui::pos2(x - 3.0, y), egui::pos2(x + 3.0, y)],
            Stroke::new(1.0_f32, Color32::from_black_alpha(90)),
        );
    }
}

impl ClipperApp {
    pub(super) fn show_quick_properties(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &self.project else { return };
        let Some(c) = p
            .selected_clips
            .iter()
            .min()
            .and_then(|id| track::clip_by_id(&p.clips, *id))
            .copied()
        else {
            return;
        };
        let action = if p.can_attach_ids(&p.selected_clips) {
            Some(("Attach Audio", ClipAction::AttachSelected))
        } else if p.can_detach_ids(&p.selected_clips) {
            Some(("Detach Audio", ClipAction::DetachSelected))
        } else {
            None
        };
        if let Some((label, action)) = action {
            if ui.button(label).on_hover_text("Collapse or expose audio without changing its edits. D detaches; Shift+D attaches.").clicked() { self.apply_clip_action(action); }
            ui.add_space(8.0);
        }
        if !self.project.as_ref().is_some_and(|p| p.is_video(c.id)) {
            return;
        }
        let width = ui.available_width() - 2.0;
        ui.set_max_width(width);
        let mut transform = c.transform;
        let previous = transform;
        textui::section(ui, "Frame");
        if crate::widgets::fit_button(ui, !self.frame_is_fitted(c.id), Some(width)).clicked() {
            self.fit_to_screen(c.id);
            return;
        }
        ui.add_space(8.0);
        let mut zoom = transform.scale * 100.0;
        let zoom_changed = ui
            .horizontal(|ui| {
                ui.label(
                    RichText::new("Zoom")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_MUTED),
                );
                ui.spacing_mut().slider_width = (width - 140.0).max(60.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Slider::new(&mut zoom, 3.0..=800.0)
                            .logarithmic(true)
                            .fixed_decimals(0)
                            .suffix("%"),
                    )
                })
                .inner
            })
            .inner;
        if zoom_changed.changed() {
            transform.scale = zoom / 100.0;
        }
        let mut opacity = transform.opacity * 100.0;
        if textui::labeled_slider(ui, "Opacity", &mut opacity, 0.0..=100.0, "%", width) {
            transform.opacity = opacity / 100.0;
        }
        if transform != previous {
            // One undo step per slider drag or click.
            if !self.clip_animation_editing {
                self.push_undo();
            }
            if let Some(clip) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|clip| clip.id == c.id))
            {
                clip.transform = transform;
            }
            self.clip_animation_editing = ui.input(|i| i.pointer.any_down());
        }

        textui::divider(ui);
        let mut animation = c.animation;
        let mut preview = None;
        textui::section(ui, "Animation");
        for (label, effect, ms) in [
            ("In", &mut animation.intro, &mut animation.in_ms),
            ("Out", &mut animation.outro, &mut animation.out_ms),
        ] {
            ui.push_id(label, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if label == "In" {
                            "Start of clip"
                        } else {
                            "End of clip"
                        })
                        .font(theme::medium(12.0))
                        .color(theme::TEXT_SECONDARY),
                    );
                    if *effect != crate::animation::Effect::None {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::widgets::preview_button(ui)
                                .on_hover_text(format!(
                                    "Play the {} animation",
                                    label.to_lowercase()
                                ))
                                .clicked()
                            {
                                preview = Some(if label == "In" {
                                    c.cut_start_ms
                                } else {
                                    c.cut_end_ms() - (*ms as i64).min(c.duration_ms() / 2) - 100
                                });
                            }
                        });
                    }
                });
                ui.add_space(6.0);
                textui::chip_grid(ui, 3, crate::animation::Effect::ALL.len(), |ui, i, cell| {
                    let option = crate::animation::Effect::ALL[i];
                    if textui::chip(ui, option.label(), *effect == option, cell).clicked() {
                        *effect = option;
                    }
                });
                if *effect != crate::animation::Effect::None {
                    textui::duration_control(ui, "Length", ms);
                }
            });
            ui.add_space(10.0);
        }
        if [animation.intro, animation.outro].iter().any(|e| {
            matches!(
                e,
                crate::animation::Effect::SnapZoom
                    | crate::animation::Effect::PullBack
                    | crate::animation::Effect::Whip
                    | crate::animation::Effect::Flash
            )
        }) {
            let mut amount = animation.amount * 100.0;
            if textui::labeled_slider(ui, "Strength", &mut amount, 0.0..=100.0, "%", width) {
                animation.amount = amount / 100.0;
            }
        }
        if animation != c.animation {
            if !self.clip_animation_editing {
                self.push_undo();
            }
            if let Some(clip) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|clip| clip.id == c.id))
            {
                clip.animation = animation;
            }
            self.clip_animation_editing = ui.input(|i| i.pointer.any_down());
        } else if !ui.input(|i| i.pointer.any_down()) {
            self.clip_animation_editing = false;
        }
        if let Some(at) = preview {
            let at = at.max(c.cut_start_ms);
            self.seek_to(at);
            if !self.live.playing {
                self.live.toggle(at);
            }
        }
    }

    pub(super) fn show_media_bin(&mut self, ui: &mut egui::Ui) {
        // Native file drags may not report a pointer position (Windows
        // OLE / some Linux compositors). Make the Media destination visible
        // for the whole drag and accept the drop anywhere in the window.
        // Media is the destination unless the pointer is over the timeline.
        let to_media = self.file_hover
            && !self
                .file_hover_pos
                .zip(self.timeline_drop.as_ref())
                .is_some_and(|(pos, zone)| zone.spot(pos).is_some());
        let fade = ui
            .ctx()
            .animate_bool_with_time(ui.id().with("file_drop_media"), to_media, 0.12);
        if fade > 0.0 {
            let rect = ui.max_rect().shrink(3.0);
            // Foreground, so the bin's own rows don't paint over it.
            let layer = egui::LayerId::new(egui::Order::Foreground, ui.id().with("drop_layer"));
            crate::widgets::paint_drop_zone(
                &ui.ctx()
                    .layer_painter(layer)
                    .with_clip_rect(rect.expand(1.0)),
                rect,
                fade,
                Some("Drop to add to Media"),
            );
        }
        let advanced = self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Media").strong().size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Import").clicked() {
                    if advanced {
                        self.import_layer();
                    } else {
                        self.open_file();
                    }
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        let assets = self
            .project
            .as_ref()
            .map(|p| p.assets.clone())
            .unwrap_or_default();
        let mut remove = None;
        let mut focus = None;
        let active_simple = self.project.as_ref().and_then(|p| p.simple.focus);
        if assets.is_empty() && advanced {
            ui.add_space(18.0);
            ui.label(
                RichText::new("Import video or audio, then drag it onto the timeline.")
                    .color(theme::TEXT_MUTED)
                    .size(13.0),
            );
        }
        egui::ScrollArea::vertical()
            .id_salt("media_bin")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for asset in &assets {
                    let name = asset
                        .source
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let video = asset.source.media.width > 0;
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 62.0),
                        egui::Sense::click_and_drag(),
                    );
                    let hover = response.hovered() || response.dragged();
                    ui.painter().rect_filled(
                        rect,
                        5.0,
                        if !advanced && active_simple == Some(asset.id) {
                            theme::BG_BUTTON_ACTIVE
                        } else if hover {
                            theme::BG_BUTTON_HOVER
                        } else {
                            theme::BG_STATUS
                        },
                    );
                    let icon = egui::Rect::from_min_size(
                        rect.min + egui::vec2(6.0, 10.0),
                        egui::vec2(64.0, 40.0),
                    );
                    ui.painter().rect_filled(icon, 2.0, theme::BG_PREVIEW);
                    if video {
                        if let Some(image) = self.thumbnails.get(ui.ctx(), &asset.source.path) {
                            ui.painter().image(
                                image.id(),
                                icon,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            ui.painter().text(
                                icon.center(),
                                Align2::CENTER_CENTER,
                                "…",
                                egui::FontId::proportional(14.0),
                                theme::TEXT_MUTED,
                            );
                        }
                    } else {
                        ui.painter().text(
                            icon.center(),
                            Align2::CENTER_CENTER,
                            "♫",
                            egui::FontId::proportional(20.0),
                            theme::TEXT_MUTED,
                        );
                    }
                    let textrect = egui::Rect::from_min_max(
                        rect.min + egui::vec2(78.0, 9.0),
                        rect.max - egui::vec2(23.0, 8.0),
                    );
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(textrect));
                    let mut title = egui::text::LayoutJob::default();
                    title.append(
                        &format!("{} · ", asset.import_number),
                        0.0,
                        egui::TextFormat {
                            font_id: theme::mono(10.5),
                            color: theme::TEXT_MUTED,
                            ..Default::default()
                        },
                    );
                    title.append(
                        name.as_ref(),
                        0.0,
                        egui::TextFormat {
                            font_id: theme::regular(12.5),
                            color: theme::TEXT_PRIMARY,
                            ..Default::default()
                        },
                    );
                    child
                        .add(egui::Label::new(title).truncate())
                        .on_hover_text(asset.label());
                    let metadata = if video {
                        format!(
                            "{} × {}  ·  {}",
                            asset.source.media.width,
                            asset.source.media.height,
                            media::format_timestamp(asset.source.media.duration_ms)
                        )
                    } else {
                        media::format_timestamp(asset.source.media.duration_ms)
                    };
                    child.add(
                        egui::Label::new(
                            RichText::new(metadata).size(10.5).color(theme::TEXT_MUTED),
                        )
                        .truncate(),
                    );
                    if !advanced && video && response.clicked() {
                        focus = Some(asset.id);
                    }
                    if advanced {
                        response.dnd_set_drag_payload(EditorDrag::Asset(asset.id));
                        response.on_hover_cursor(egui::CursorIcon::Grab);
                    }
                    let close = egui::Rect::from_min_size(
                        egui::pos2(rect.right() - 23.0, rect.top() + 4.0),
                        egui::vec2(20.0, 22.0),
                    );
                    let close_response = ui
                        .interact(
                            close,
                            ui.id().with(("remove_media", asset.id)),
                            egui::Sense::click(),
                        )
                        .on_hover_text("Remove media and its timeline clips");
                    if hover || close_response.hovered() {
                        ui.painter().text(
                            close.center(),
                            Align2::CENTER_CENTER,
                            "×",
                            egui::FontId::proportional(16.0),
                            theme::TEXT_MUTED,
                        );
                    }
                    if close_response.clicked() {
                        remove = Some(asset.id);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(id) = focus {
            self.select_simple_media(id);
            ui.ctx().request_repaint();
        }
        if let Some(id) = remove {
            if advanced {
                self.push_undo();
            }
            if let Some(p) = &mut self.project {
                p.remove_asset(id);
            }
            self.live.clear();
            if !advanced
                && self
                    .project
                    .as_ref()
                    .is_some_and(|p| p.simple.focus.is_none())
            {
                if let Some(id) = self.project.as_ref().and_then(|p| {
                    p.assets
                        .iter()
                        .find(|a| a.source.media.width > 0)
                        .map(|a| a.id)
                }) {
                    self.select_simple_media(id);
                } else {
                    self.undo_stack.clear();
                    self.redo_stack.clear();
                    let _ = self.player.set_paused(true);
                }
            }
            ui.ctx().request_repaint();
            self.status = "Media removed.".into();
            if self.project.as_ref().is_some_and(|p| p.assets.is_empty()) {
                let _ = self.player.set_paused(true);
                if let Some(h) = self.proxy_handle.take() {
                    h.cancel();
                }
            }
        }
        if let Some(drag) = egui::DragAndDrop::payload::<EditorDrag>(ui.ctx()) {
            let name = match drag.as_ref() {
                EditorDrag::Asset(id) => self
                    .project
                    .as_ref()
                    .and_then(|p| p.assets.iter().find(|a| a.id == *id))
                    .map(|a| a.label()),
                _ => None,
            };
            if let (Some(name), Some(pointer)) = (
                name,
                ui.ctx()
                    .pointer_hover_pos()
                    .filter(|pos| ui.ctx().available_rect().contains(*pos)),
            ) {
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("media_drag"),
                ));
                let text = egui::RichText::new(name).size(12.0);
                let galley = painter.layout_no_wrap(
                    text.text().to_string(),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_PRIMARY,
                );
                let rect = egui::Rect::from_min_size(
                    pointer + egui::vec2(14.0, 14.0),
                    galley.size() + egui::vec2(20.0, 14.0),
                );
                painter.rect_filled(rect, 4.0, theme::BG_BUTTON);
                painter.galley(
                    rect.min + egui::vec2(10.0, 7.0),
                    galley,
                    theme::TEXT_PRIMARY,
                );
            }
        }
    }
    /// Transport row above the Advanced timeline: go-to-start, play,
    /// timecode, monitor volume and the Export call to action.
    fn show_transport(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project.as_ref() else {
            return;
        };
        let playhead = p.playhead_ms;
        let end = p.content_end_ms();
        let can_export = !p.clips.is_empty() && self.export.is_none();
        let mut toggle = false;
        let mut export = false;
        let mut to_start = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if crate::widgets::icon_button(
                ui,
                crate::widgets::Icon::SkipStart,
                30.0,
                false,
                theme::ACCENT,
            )
            .on_hover_text("Go to start  ·  Home")
            .clicked()
            {
                to_start = true;
            }
            if crate::widgets::play_button(ui, self.live.playing)
                .on_hover_text("Play / pause  ·  Space")
                .clicked()
            {
                toggle = true;
            }
            ui.add_space(8.0);
            ui.label(crate::widgets::timecode_job(
                &media::format_timestamp(playhead),
                16.0,
                theme::TEXT_PRIMARY,
            ));
            ui.label(
                RichText::new(format!("/ {}", media::format_timestamp(end)))
                    .font(theme::mono(11.5))
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, "Export…", can_export).clicked() {
                    export = true;
                }
                ui.add_space(10.0);
                ui.spacing_mut().slider_width = 84.0;
                let response = ui
                    .add(egui::Slider::new(&mut self.volume_ui, 0.0..=130.0).show_value(false))
                    .on_hover_text(format!("Preview volume {:.0}%", self.volume_ui));
                if response.changed() {
                    let _ = self.player.set_volume(self.volume_ui);
                }
                let muted = self.player.state().muted;
                if crate::widgets::icon_button(
                    ui,
                    if muted {
                        crate::widgets::Icon::SpeakerMuted
                    } else {
                        crate::widgets::Icon::Speaker
                    },
                    28.0,
                    muted,
                    theme::ACCENT_ERR,
                )
                .on_hover_text("Mute preview  ·  M")
                .clicked()
                {
                    let _ = self.player.toggle_mute();
                }
            });
        });
        if toggle {
            self.toggle_play();
        }
        if to_start {
            if let Some(p) = &mut self.project {
                p.playhead_ms = 0;
            }
            self.live.reanchor(0);
            ui.ctx().request_repaint();
        }
        if export {
            self.export_options_open = true;
        }
    }

    pub(super) fn show_editor_timeline(&mut self, ui: &mut egui::Ui) {
        self.show_transport(ui);
        ui.add_space(8.0);
        let Some(p) = self.project.as_ref() else {
            return;
        };
        let (tracks, clips, compound_labels) = p.timeline_view();
        let selected: HashSet<_> = p
            .selected_clips
            .iter()
            .flat_map(|id| p.edit_ids(*id))
            .collect();
        let real_indices: HashMap<_, _> = p
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();
        let duration = p.canvas_length_ms().max(1000);
        let playhead = p.playhead_ms;
        let file_name = |c: &Clip| {
            let source = p.clip_source(c).0;
            p.assets
                .iter()
                .find(|a| a.source.path == source)
                .map(|a| a.label())
                .unwrap_or_else(|| {
                    source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
        };
        // Compound bars show their audio as a strip, so the " · V+A"
        // suffix the view model adds is redundant on screen.
        let compounds: HashSet<ClipId> = compound_labels
            .iter()
            .filter(|(_, label)| label.ends_with("V+A"))
            .map(|(id, _)| *id)
            .collect();
        let mut labels: HashMap<_, _> = clips.iter().map(|c| (c.id, file_name(c))).collect();
        labels.extend(
            compound_labels
                .into_iter()
                .filter(|(id, _)| !compounds.contains(id)),
        );
        // Audio tracks each row monitors: its own (audio) or the ones
        // folded into its compound bars (video).
        let monitored: HashMap<TrackId, Vec<TrackId>> = tracks
            .iter()
            .map(|t| {
                let ids = match t.kind {
                    TrackKind::Audio => vec![t.id],
                    TrackKind::Video => p.attached_audio_tracks(t.id).into_iter().collect(),
                    TrackKind::Text => Vec::new(),
                };
                (t.id, ids)
            })
            .collect();
        let monitor_muted = |t: TrackId| {
            monitored.get(&t).is_some_and(|ids| {
                !ids.is_empty() && ids.iter().all(|id| self.live.monitor_muted.contains(id))
            })
        };
        let payload = egui::DragAndDrop::payload::<EditorDrag>(ui.ctx());
        let images: HashMap<_, _> = clips
            .iter()
            .filter(|c| p.is_video(c.id))
            .filter_map(|c| {
                self.thumbnails
                    .get(ui.ctx(), p.clip_source(c).0)
                    .map(|texture| (c.id, texture.id()))
            })
            .collect();
        if payload.is_none() {
            ui.ctx().data_mut(|data| {
                data.remove::<(TrackId, f32, bool)>(egui::Id::new("first_timeline_placement"))
            });
        }
        let mut seek = None;
        let mut scrubbing = false;
        let mut select = None;
        let mut clear = false;
        let mut mute = None;
        let mut monitor_toggle: Option<(Vec<TrackId>, bool)> = None;
        let mut drop = None;
        let mut context_action = None;

        // Shared surface behind ruler + lanes, filled once its extent is known.
        let surface_bg = ui.painter().add(egui::Shape::Noop);
        let (ruler_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), RULER_HEIGHT),
            egui::Sense::hover(),
        );
        let footer_h = 26.0;
        // Exact budget: scroll + scrollbar + spacing + footer must not exceed
        // what the panel offers, or the resizable panel grows every frame.
        let height =
            (ui.available_height() - footer_h - SCROLLBAR_H - 2.0 * ui.spacing().item_spacing.y)
                .max(LANE_HEIGHT);

        // ── Zoom / pan ── Ctrl+wheel zooms around the pointer; Shift+wheel or
        // a sideways swipe pans. The view lives in egui memory: it is a
        // viewing preference, not part of the edit.
        let view_id = ui.id().with("timeline_zoom");
        let mut view: TimelineZoom = ui.data(|d| d.get_temp(view_id)).unwrap_or_default();
        view.clamp(duration);
        let current = Lanes::window(ruler_rect, view.start, view.span(duration));
        let timeline_area = egui::Rect::from_min_max(
            egui::pos2(current.left, ruler_rect.top()),
            egui::pos2(ruler_rect.right(), ruler_rect.bottom() + height),
        );
        if let Some(pointer) = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|pos| timeline_area.contains(*pos))
        {
            let (zoom, pan) = ui.input_mut(|i| {
                let pan = i.smooth_scroll_delta.x;
                i.smooth_scroll_delta.x = 0.0;
                (i.zoom_delta(), pan)
            });
            if (zoom - 1.0).abs() > f32::EPSILON {
                let fraction = ((pointer.x - current.left) / current.width).clamp(0.0, 1.0);
                view.zoom_at(zoom, current.time_at(pointer.x), fraction, duration);
            }
            if pan != 0.0 {
                let ms = -pan / current.width * view.span(duration) as f32;
                view.pan(ms.round() as i64, duration);
            }
        }
        // Glide toward a scrollbar click target.
        let glide_id = view_id.with("glide");
        if let Some(target) = ui.data(|d| d.get_temp::<i64>(glide_id)) {
            let step = ((target - view.start) as f32 * 0.3).round() as i64;
            if step.abs() <= 1 {
                view.start = target;
                ui.data_mut(|d| d.remove::<i64>(glide_id));
            } else {
                view.start += step;
                ui.ctx().request_repaint();
            }
            view.clamp(duration);
        }
        // Keep the playhead in view whenever it moves (playback, keys,
        // clicks), but let the user scroll away from a parked playhead.
        let last_id = view_id.with("last_playhead");
        let moved = ui.data(|d| d.get_temp::<i64>(last_id)) != Some(playhead);
        ui.data_mut(|d| d.insert_temp(last_id, playhead));
        if self.live.playing || moved {
            view.follow(playhead, duration);
        }
        ui.data_mut(|d| d.insert_temp(view_id, view));
        let span = view.span(duration);
        let ruler_lanes = Lanes::window(ruler_rect, view.start, span);
        let step = ruler_step(span, ruler_lanes.width);
        let band_snaps: Vec<i64> = [0, playhead]
            .into_iter()
            .chain(clips.iter().flat_map(|c| [c.cut_start_ms, c.cut_end_ms()]))
            .collect();
        let mut band_actions = Vec::new();
        let band_drag = &mut self.text_ui.band_drag;
        let lane_bg = theme::BG_PANEL;
        let empty_bg = theme::BG_HEADER;
        let header_bg = Color32::from_rgb(23, 24, 29);
        let scroll = egui::ScrollArea::vertical()
            .id_salt("timeline_lanes")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let lane_h = LANE_HEIGHT;
                let rows = tracks.len();
                let text_drag = band_drag.as_ref().and_then(|d| d.moving());
                let dragging = payload.is_some() || text_drag.is_some();
                let reserve = usize::from(dragging && !tracks.is_empty());
                let content_h = (rows + reserve).max(1) as f32 * lane_h;
                let total_h = content_h.max(height);
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), total_h),
                    egui::Sense::hover(),
                );
                let lanes = Lanes::window(rect, view.start, span);
                let left = lanes.left;
                let painter = ui.painter_at(rect);
                // Everything time-positioned is clipped to the lane area so
                // zoomed clips never slide under the track headers.
                let lane_area = egui::Rect::from_min_max(egui::pos2(left, rect.top()), rect.max);
                let lane_painter = painter.with_clip_rect(lane_area.intersect(painter.clip_rect()));
                painter.rect_filled(rect, 0.0, empty_bg);
                painter.rect_filled(
                    egui::Rect::from_min_max(rect.min, egui::pos2(left, rect.bottom())),
                    0.0,
                    header_bg,
                );
                // Background: click empty timeline to seek + deselect.
                let background = ui.interact(
                    egui::Rect::from_min_max(egui::pos2(left, rect.top()), rect.max),
                    ui.id().with("timeline_background"),
                    egui::Sense::click(),
                );
                if background.clicked() {
                    seek = background
                        .interact_pointer_pos()
                        .map(|pos| lanes.time_at(pos.x).min(duration));
                    clear = true;
                }
                let hover = ui
                    .ctx()
                    .pointer_hover_pos()
                    .filter(|pos| rect.intersect(ui.clip_rect()).contains(*pos));
                // Last frame's insertion gap, so the lane decision reads
                // the layout as drawn.
                let gap_id = ui.id().with("drag_gap");
                let last_gap = ui.data(|d| d.get_temp::<Option<usize>>(gap_id)).flatten();
                let placement = payload.as_ref().and_then(|drag| {
                    hover.and_then(|pos| {
                        placement(p, &tracks, &clips, drag, pos, lanes, last_gap, ui.ctx())
                    })
                });
                // A text bar being moved: same lane rules as clips.
                let text_target = text_drag.and_then(|id| {
                    let item = p.texts.iter().find(|t| t.id == id)?;
                    let pos = ui.ctx().pointer_interact_pos()?;
                    let fits = |lane: TrackId| {
                        !p.texts.iter().any(|t| {
                            t.track == lane
                                && t.id != id
                                && item.start_ms < t.end_ms
                                && item.end_ms > t.start_ms
                        })
                    };
                    let ask = LaneAsk {
                        kind: TrackKind::Text,
                        own: Some(item.track),
                        alone: p.texts.iter().all(|t| t.track != item.track || t.id == id),
                        fits: &fits,
                    };
                    Some(resolve_lane(pos.y - rect.top(), last_gap, &tracks, &ask))
                });
                if let (Some(d), Some(target)) = (band_drag.as_mut(), text_target) {
                    d.set_target(target);
                }
                let gap = match text_target {
                    Some(textui::LaneTarget::New(slot)) => Some(slot),
                    Some(_) => None,
                    None => placement
                        .as_ref()
                        .filter(|drop| drop.target.is_none())
                        .map(|drop| drop.slot),
                };
                if dragging {
                    ui.data_mut(|d| d.insert_temp(gap_id, gap));
                } else {
                    ui.data_mut(|d| {
                        d.remove::<Option<usize>>(gap_id);
                        d.remove::<f32>(gap_id.with("ghost"));
                    });
                }
                // The opening lane, under the rows that slide away from it.
                if let Some(slot) = gap {
                    let kind = text_target
                        .map(|_| TrackKind::Text)
                        .or(placement.as_ref().map(|place| place.kind))
                        .unwrap_or(TrackKind::Video);
                    paint_new_lane(&painter, rect, left, slot, kind);
                }
                // The clip being moved leaves its old spot the moment a
                // drop position exists; only the ghost is drawn.
                let moving = match (payload.as_deref(), &placement) {
                    (Some(EditorDrag::Clip { id, .. }), Some(_)) => Some(*id),
                    _ => None,
                };
                for (index, t) in tracks.iter().enumerate() {
                    let target_shift = if gap.is_some_and(|slot| index >= slot) {
                        lane_h
                    } else {
                        0.0
                    };
                    let shift = ui.ctx().animate_value_with_time(
                        ui.id().with(("lane_insertion", t.id)),
                        target_shift,
                        0.14,
                    );
                    let top = rect.top() + index as f32 * lane_h + shift;
                    let row = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), top),
                        egui::pos2(rect.right(), top + lane_h),
                    );
                    let lane = egui::Rect::from_min_max(egui::pos2(left, top), row.max);
                    painter.rect_filled(lane, 0.0, lane_bg);
                    for t in lanes.ticks(step) {
                        let x = lanes.x_of(t);
                        lane_painter.line_segment(
                            [egui::pos2(x, top), egui::pos2(x, top + lane_h)],
                            Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
                        );
                    }
                    painter.line_segment(
                        [row.left_bottom(), row.right_bottom()],
                        Stroke::new(1.0_f32, theme::BG_HEADER),
                    );

                    // ── Header ──
                    let title = egui::Rect::from_min_max(row.min, egui::pos2(left, row.bottom()));
                    painter.rect_filled(title, 0.0, header_bg);
                    painter.line_segment(
                        [title.right_top(), title.right_bottom()],
                        Stroke::new(1.0_f32, theme::BORDER),
                    );
                    let kind_color = match t.kind {
                        TrackKind::Video => theme::CLIP_VIDEO_EDGE,
                        TrackKind::Audio => theme::CLIP_AUDIO_EDGE,
                        TrackKind::Text => theme::CLIP_TEXT_EDGE,
                    };
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            title.left_top() + egui::vec2(0.0, 8.0),
                            egui::vec2(3.0, lane_h - 16.0),
                        ),
                        egui::Rounding {
                            ne: 2.0,
                            se: 2.0,
                            nw: 0.0,
                            sw: 0.0,
                        },
                        if t.muted {
                            kind_color.gamma_multiply(0.35)
                        } else {
                            kind_color
                        },
                    );
                    let title_response = ui
                        .interact(
                            title,
                            ui.id().with(("track_drag", t.id)),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text("Drag to reorder · right-click for track options");
                    title_response.dnd_set_drag_payload(EditorDrag::Track(t.id));
                    let has_monitor = monitored.get(&t.id).is_some_and(|ids| !ids.is_empty());
                    let sub = match (t.muted, t.kind, has_monitor) {
                        (true, ..) => "Disabled",
                        (false, TrackKind::Audio, _) => "Audio",
                        (false, TrackKind::Video, true) => "Video + audio",
                        (false, TrackKind::Video, false) => "Video",
                        (false, TrackKind::Text, _) => "Text",
                    };
                    painter.text(
                        egui::pos2(title.left() + 14.0, title.center().y - 7.0),
                        Align2::LEFT_CENTER,
                        &t.name,
                        theme::semibold(12.5),
                        if t.muted {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT_PRIMARY
                        },
                    );
                    painter.text(
                        egui::pos2(title.left() + 14.0, title.center().y + 9.0),
                        Align2::LEFT_CENTER,
                        sub,
                        theme::regular(10.5),
                        theme::TEXT_MUTED,
                    );
                    title_response.context_menu(|ui| {
                        ui.set_width(220.0);
                        if crate::widgets::menu_item(
                            ui,
                            if t.muted {
                                "Enable track"
                            } else {
                                "Disable track (preview + export)"
                            },
                            None,
                        )
                        .clicked()
                        {
                            mute = Some(t.id);
                            ui.close_menu();
                        }
                    });
                    if has_monitor {
                        let muted = monitor_muted(t.id);
                        let button = egui::Rect::from_center_size(
                            egui::pos2(title.right() - 20.0, title.center().y),
                            egui::vec2(26.0, 26.0),
                        );
                        let response = ui
                            .interact(
                                button,
                                ui.id().with(("monitor", t.id)),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(if muted {
                                "Unmute in preview"
                            } else {
                                "Mute in preview — the export keeps this audio"
                            });
                        crate::widgets::paint_icon_button(
                            ui,
                            button,
                            &response,
                            if muted {
                                crate::widgets::Icon::SpeakerMuted
                            } else {
                                crate::widgets::Icon::Speaker
                            },
                            muted,
                            theme::ACCENT_ERR,
                        );
                        if response.clicked() {
                            monitor_toggle = monitored.get(&t.id).map(|ids| (ids.clone(), !muted));
                        }
                    }

                    if t.kind == TrackKind::Text {
                        band_actions.extend(textui::show_text_lane(
                            ui,
                            &lane_painter,
                            lane.intersect(lane_area),
                            t.id,
                            CLIP_INSET,
                            lanes,
                            &p.texts,
                            p.selected_text,
                            &band_snaps,
                            t.muted,
                            band_drag,
                        ));
                        continue;
                    }

                    // ── Clips ──
                    let mut row_clips: Vec<&Clip> =
                        clips.iter().filter(|c| c.track_id == t.id).collect();
                    row_clips.sort_by_key(|c| c.cut_start_ms);
                    let joined = |a: &Clip, b: &Clip| {
                        a.cut_end_ms() == b.cut_start_ms
                            && Some(a.id) != moving
                            && Some(b.id) != moving
                    };
                    let row_muted = monitor_muted(t.id);
                    for (i, c) in row_clips.iter().enumerate() {
                        if Some(c.id) == moving {
                            continue;
                        }
                        let join_left = i > 0 && joined(row_clips[i - 1], c);
                        let join_right = row_clips.get(i + 1).is_some_and(|n| joined(c, n));
                        let x0 = lanes.x_of(c.cut_start_ms) + if join_left { 1.0 } else { 0.0 };
                        let x1 = lanes.x_of(c.cut_end_ms()) - if join_right { 1.0 } else { 0.0 };
                        let block = egui::Rect::from_min_max(
                            egui::pos2(x0, top + CLIP_INSET),
                            egui::pos2(x1.max(x0 + 4.0), top + lane_h - CLIP_INSET),
                        );
                        let hit = block.intersect(lane_area);
                        if !hit.is_positive() {
                            continue;
                        }
                        paint_clip(
                            &lane_painter,
                            block,
                            &ClipLook {
                                label: labels.get(&c.id).map(String::as_str).unwrap_or("Clip"),
                                kind: t.kind,
                                thumb: images.get(&c.id).copied(),
                                selected: selected.contains(&c.id),
                                disabled: t.muted,
                                quiet: t.kind == TrackKind::Audio && row_muted,
                                linked_audio: compounds.contains(&c.id).then_some(row_muted),
                                duration_ms: c.duration_ms(),
                                speed: c.speed,
                                join_left,
                                join_right,
                            },
                        );
                        let response = ui
                            .interact(
                                hit,
                                ui.id().with(("timeline_clip", c.id)),
                                egui::Sense::click_and_drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::Grab);
                        if response.clicked() {
                            select = Some(c.id);
                        }
                        let grab = ui
                            .input(|i| i.pointer.press_origin())
                            .map(|pos| lanes.time_at(pos.x) - c.cut_start_ms)
                            .unwrap_or(0);
                        response.dnd_set_drag_payload(EditorDrag::Clip {
                            id: c.id,
                            grab_ms: grab,
                        });
                        response.context_menu(|ui| {
                            ui.set_width(220.0);
                            let context_selection = if selected.contains(&c.id) {
                                p.selected_clips.clone()
                            } else {
                                [c.id].into_iter().collect()
                            };
                            let mut chosen = None;
                            if crate::widgets::menu_item(ui, "Split at playhead", Some("S"))
                                .clicked()
                            {
                                chosen = Some(ClipAction::Split);
                            }
                            if p.can_detach_ids(&context_selection)
                                && crate::widgets::menu_item(ui, "Detach audio", Some("D"))
                                    .clicked()
                            {
                                chosen = Some(ClipAction::DetachSelected);
                            }
                            if p.can_attach_ids(&context_selection)
                                && crate::widgets::menu_item(ui, "Attach audio", Some("Shift+D"))
                                    .clicked()
                            {
                                chosen = Some(ClipAction::AttachSelected);
                            }
                            if crate::widgets::menu_item(ui, "Close gap before", Some("G"))
                                .clicked()
                            {
                                chosen = Some(ClipAction::CloseGaps);
                            }
                            crate::widgets::menu_separator(ui);
                            if crate::widgets::menu_item(ui, "Delete", Some("Del")).clicked() {
                                chosen = Some(ClipAction::DeleteSelected);
                            }
                            if let Some(action) = chosen {
                                if !selected.contains(&c.id) {
                                    select = Some(c.id);
                                }
                                context_action = Some(action);
                                ui.close_menu();
                            }
                        });
                    }
                    // Edit points on top of both neighbours.
                    for pair in row_clips.windows(2).filter(|w| joined(w[0], w[1])) {
                        paint_seam(
                            &lane_painter,
                            lanes.x_of(pair[1].cut_start_ms),
                            top + CLIP_INSET,
                            top + lane_h - CLIP_INSET,
                        );
                    }
                }
                if tracks.is_empty() {
                    painter.text(
                        egui::pos2(left + 16.0, rect.top() + lane_h * 0.5),
                        Align2::LEFT_CENTER,
                        "Drop video or audio here",
                        theme::regular(12.5),
                        theme::TEXT_MUTED,
                    );
                }

                // ── Drag feedback ──
                if let (Some(payload), Some(pos)) = (payload.as_ref(), hover) {
                    if let Some(place) = placement {
                        let row_index = match place.target {
                            Some(target) => tracks.iter().position(|t| t.id == target).unwrap_or(0),
                            None => place.slot,
                        };
                        let y = glide(
                            ui.ctx(),
                            gap_id.with("ghost"),
                            rect.top() + row_index as f32 * lane_h,
                        );
                        let x0 = lanes.x_of(place.start);
                        let ghost = egui::Rect::from_min_max(
                            egui::pos2(x0, y + CLIP_INSET),
                            egui::pos2(
                                lanes.x_of(place.start + place.length).max(x0 + 12.0),
                                y + lane_h - CLIP_INSET,
                            ),
                        );
                        let (thumb, linked, label) = match payload.as_ref() {
                            EditorDrag::Clip { id, .. } => (
                                images.get(id).copied(),
                                compounds.contains(id).then_some(false),
                                labels.get(id).cloned().unwrap_or(place.label.clone()),
                            ),
                            EditorDrag::Asset(id) => (
                                p.assets
                                    .iter()
                                    .find(|a| a.id == *id && a.source.media.width > 0)
                                    .and_then(|a| self.thumbnails.get(ui.ctx(), &a.source.path))
                                    .map(|t| t.id()),
                                p.assets
                                    .iter()
                                    .find(|a| a.id == *id)
                                    .filter(|a| {
                                        a.source.media.width > 0
                                            && a.source.media.audio_stream_count > 0
                                    })
                                    .map(|_| false),
                                place.label.clone(),
                            ),
                            EditorDrag::Track(_) => (None, None, place.label.clone()),
                        };
                        paint_clip(
                            &lane_painter,
                            ghost,
                            &ClipLook {
                                label: &label,
                                kind: place.kind,
                                thumb,
                                selected: true,
                                disabled: false,
                                quiet: false,
                                linked_audio: linked,
                                duration_ms: place.length,
                                speed: 1.0,
                                join_left: false,
                                join_right: false,
                            },
                        );
                        if let Some(time) = place.snap {
                            let x = lanes.x_of(time);
                            lane_painter.line_segment(
                                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                                Stroke::new(1.0_f32, theme::ACCENT),
                            );
                        }
                        if ui.input(|i| i.pointer.any_released()) {
                            let index = tracks
                                .get(place.slot)
                                .and_then(|t| real_indices.get(&t.id))
                                .copied()
                                .unwrap_or(p.tracks.len());
                            drop =
                                Some((payload.as_ref().clone(), place.start, place.target, index));
                        }
                    } else if let EditorDrag::Track(id) = payload.as_ref() {
                        let slot =
                            (((pos.y - rect.top()) / lane_h) + 0.5).floor().max(0.0) as usize;
                        let y = rect.top() + slot.min(tracks.len()) as f32 * lane_h;
                        painter.line_segment(
                            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                            Stroke::new(2.0_f32, theme::ACCENT),
                        );
                        if ui.input(|i| i.pointer.any_released()) {
                            let index = tracks
                                .get(slot)
                                .and_then(|t| real_indices.get(&t.id))
                                .copied()
                                .unwrap_or(p.tracks.len());
                            drop = Some((EditorDrag::Track(*id), 0, None, index));
                        }
                    }
                }

                // A text bar being moved glides to the lane it will land on.
                if let (Some(id), Some(target)) = (text_drag, text_target) {
                    let row = match target {
                        textui::LaneTarget::Lane(lane) => {
                            tracks.iter().position(|t| t.id == lane).unwrap_or(0)
                        }
                        textui::LaneTarget::New(slot) => slot,
                    };
                    let y = glide(
                        ui.ctx(),
                        gap_id.with("ghost"),
                        rect.top() + row as f32 * lane_h,
                    );
                    if let Some(item) = p.texts.iter().find(|t| t.id == id) {
                        let x0 = lanes.x_of(item.start_ms);
                        let bar = egui::Rect::from_min_max(
                            egui::pos2(x0, y + CLIP_INSET),
                            egui::pos2(
                                lanes.x_of(item.end_ms).max(x0 + 4.0),
                                y + lane_h - CLIP_INSET,
                            ),
                        );
                        textui::paint_lifted_text_bar(&lane_painter, bar, item);
                    }
                }

                // ── Playhead line (grabbable a few px either side) ──
                let viewport = ui.clip_rect().intersect(rect);
                let x = lanes.x_of(playhead);
                if (left..=lanes.right() + 1.0).contains(&x) {
                    let line_hit = egui::Rect::from_min_max(
                        egui::pos2(x - 5.0, viewport.top()),
                        egui::pos2(x + 5.0, viewport.bottom()),
                    );
                    let line =
                        ui.interact(line_hit, ui.id().with("playhead_line"), egui::Sense::drag());
                    let hot = line.hovered() || line.dragged();
                    if hot {
                        ui.ctx().set_cursor_icon(if line.dragged() {
                            egui::CursorIcon::Grabbing
                        } else {
                            egui::CursorIcon::Grab
                        });
                    }
                    if line.dragged() {
                        seek = line
                            .interact_pointer_pos()
                            .map(|pos| lanes.time_at(pos.x).min(duration));
                        scrubbing = true;
                    }
                    lane_painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        Stroke::new(if hot { 2.0_f32 } else { 1.5 }, theme::PLAYHEAD),
                    );
                }
                (viewport, rect.top())
            });

        // ── Ruler (pinned above the scrolling lanes) ──
        let (lanes_view, rows_top) = scroll.inner;
        // External file drags: remember the lanes for the drop, and show
        // where the files would land.
        let zone = TimelineDropZone {
            area: lanes_view,
            lanes: Lanes::window(lanes_view, view.start, span),
            rows_top,
            real: tracks
                .iter()
                .filter_map(|t| real_indices.get(&t.id).copied())
                .collect(),
            video: tracks
                .iter()
                .map(|t| (t.kind == TrackKind::Video && !t.muted).then_some(t.id))
                .collect(),
            visual_rows: tracks.iter().filter(|t| t.kind.is_visual()).count(),
            total: p.tracks.len(),
            snaps: [0, playhead]
                .into_iter()
                .chain(clips.iter().map(|c| c.cut_end_ms()))
                .collect(),
        };
        let file_spot = self
            .file_hover
            .then_some(self.file_hover_pos)
            .flatten()
            .and_then(|pos| zone.spot_and_row(pos));
        let drop_fade = ui.ctx().animate_bool_with_time(
            ui.id().with("file_drop_timeline"),
            file_spot.is_some(),
            0.12,
        );
        if let Some((start, lane)) = file_spot {
            let painter = ui.painter_at(lanes_view);
            crate::widgets::paint_drop_zone(&painter, lanes_view.shrink(1.0), drop_fade, None);
            let x = zone.lanes.x_of(start);
            let y = match lane {
                FileLane::Onto(_, row) => {
                    // The lane it lands on, softly lit.
                    let top = rows_top + row as f32 * LANE_HEIGHT;
                    let well = egui::Rect::from_min_max(
                        egui::pos2(zone.lanes.left + 2.0, top + 3.0),
                        egui::pos2(lanes_view.right() - 3.0, top + LANE_HEIGHT - 3.0),
                    );
                    painter.rect_filled(well, 6.0, theme::ACCENT.gamma_multiply(0.10 * drop_fade));
                    painter.rect_stroke(
                        well,
                        6.0,
                        Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.55 * drop_fade)),
                    );
                    top + LANE_HEIGHT * 0.5
                }
                FileLane::Insert(slot) => {
                    let y = rows_top + slot as f32 * LANE_HEIGHT;
                    painter.line_segment(
                        [
                            egui::pos2(zone.lanes.left, y),
                            egui::pos2(lanes_view.right(), y),
                        ],
                        Stroke::new(2.0_f32, theme::ACCENT.gamma_multiply(drop_fade)),
                    );
                    y
                }
            };
            painter.line_segment(
                [
                    egui::pos2(x, lanes_view.top()),
                    egui::pos2(x, lanes_view.bottom()),
                ],
                Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.8 * drop_fade)),
            );
            let label = format!("Add at {}", media::format_timestamp(start));
            let galley = painter.layout_no_wrap(label, theme::medium(12.0), theme::TEXT_PRIMARY);
            let chip = egui::Rect::from_min_size(
                egui::pos2(
                    x + 8.0,
                    (y - galley.size().y * 0.5 - 6.0).max(lanes_view.top() + 4.0),
                ),
                galley.size() + egui::vec2(16.0, 10.0),
            );
            painter.rect_filled(chip, 6.0, theme::BG_MODAL.gamma_multiply(drop_fade));
            painter.rect_stroke(
                chip,
                6.0,
                Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.6 * drop_fade)),
            );
            painter.galley(chip.min + egui::vec2(8.0, 5.0), galley, theme::TEXT_PRIMARY);
        }
        self.timeline_drop = Some(zone);
        let (bar_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), SCROLLBAR_H),
            egui::Sense::hover(),
        );
        if let Some(start) = timeline_scrollbar(ui, bar_rect, ruler_lanes, view, duration, playhead)
        {
            view.start = start;
            view.clamp(duration);
            ui.data_mut(|d| d.insert_temp(view_id, view));
            ui.ctx().request_repaint();
        }
        let surface = ruler_rect.union(lanes_view);
        ui.painter()
            .set(surface_bg, egui::Shape::rect_filled(surface, 6.0, empty_bg));
        let painter = ui.painter_at(ruler_rect);
        painter.rect_filled(
            ruler_rect,
            egui::Rounding {
                nw: 6.0,
                ne: 6.0,
                sw: 0.0,
                se: 0.0,
            },
            theme::BG_HEADER,
        );
        painter.line_segment(
            [ruler_rect.left_bottom(), ruler_rect.right_bottom()],
            Stroke::new(1.0_f32, theme::BORDER),
        );
        let header = egui::Rect::from_min_max(
            ruler_rect.min,
            egui::pos2(ruler_lanes.left, ruler_rect.bottom()),
        );
        painter.line_segment(
            [header.right_top(), header.right_bottom()],
            Stroke::new(1.0_f32, theme::BORDER),
        );
        let ticks = painter.with_clip_rect(egui::Rect::from_min_max(
            egui::pos2(ruler_lanes.left, ruler_rect.top()),
            ruler_rect.max,
        ));
        let minor = (step / 5).max(1);
        for t in ruler_lanes
            .ticks(minor)
            .filter(|t| (0..=duration).contains(t))
        {
            let x = ruler_lanes.x_of(t);
            let major = t % step == 0;
            ticks.line_segment(
                [
                    egui::pos2(x, ruler_rect.bottom() - if major { 9.0 } else { 4.0 }),
                    egui::pos2(x, ruler_rect.bottom()),
                ],
                Stroke::new(
                    1.0_f32,
                    if major {
                        theme::TEXT_FAINT
                    } else {
                        theme::BORDER
                    },
                ),
            );
            if major {
                ticks.text(
                    egui::pos2(x + 4.0, ruler_rect.top() + 10.0),
                    Align2::LEFT_CENTER,
                    ruler_label(t, step),
                    theme::mono(10.0),
                    theme::TEXT_MUTED,
                );
            }
        }
        // The whole ruler scrubs; the head is a fat target on top of it.
        let ruler_hit = egui::Rect::from_min_max(
            egui::pos2(ruler_lanes.left - 8.0, ruler_rect.top()),
            ruler_rect.max,
        );
        let ruler = ui.interact(
            ruler_hit,
            ui.id().with("timeline_ruler"),
            egui::Sense::click_and_drag(),
        );
        let in_view = |x: f32| (ruler_lanes.left - 1.0..=ruler_lanes.right() + 1.0).contains(&x);
        let ph_x = ruler_lanes.x_of(playhead);
        // Off-screen (zoomed away) the head has no hit area at all.
        let head_hit = egui::Rect::from_center_size(
            egui::pos2(ph_x, ruler_rect.center().y),
            if in_view(ph_x) {
                egui::vec2(28.0, RULER_HEIGHT + 8.0)
            } else {
                egui::Vec2::ZERO
            },
        );
        let head = ui.interact(head_hit, ui.id().with("playhead_head"), egui::Sense::drag());
        let dragging_head = head.dragged() || ruler.dragged() || scrubbing;
        if head.hovered() || ruler.hovered() || dragging_head {
            ui.ctx().set_cursor_icon(if dragging_head {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        for response in [&ruler, &head] {
            if response.clicked() || response.dragged() {
                seek = response
                    .interact_pointer_pos()
                    .map(|pos| ruler_lanes.time_at(pos.x).min(duration));
                scrubbing |= response.dragged();
            }
        }
        let shown = seek.unwrap_or(playhead);
        if scrubbing && seek.is_some() {
            // Scrubbing past either edge of a zoomed view drags it along.
            view.follow(shown, duration);
            ui.data_mut(|d| d.insert_temp(view_id, view));
        }
        let ph_x = ruler_lanes.x_of(shown);
        // Live readout in the header cell — the time stays visible however
        // far the lanes are scrolled.
        painter.text(
            egui::pos2(header.left() + 14.0, header.center().y),
            Align2::LEFT_CENTER,
            media::format_timestamp(shown),
            theme::mono(11.0),
            if dragging_head {
                theme::PLAYHEAD
            } else {
                theme::TEXT_SECONDARY
            },
        );
        if in_view(ph_x) {
            ui.painter_at(ruler_rect.expand2(egui::vec2(0.0, 1.0)))
                .line_segment(
                    [
                        egui::pos2(ph_x, ruler_rect.bottom() - 6.0),
                        egui::pos2(ph_x, ruler_rect.bottom() + 1.0),
                    ],
                    Stroke::new(1.5_f32, theme::PLAYHEAD),
                );
            paint_playhead_head(
                &ui.painter_at(ruler_rect.expand2(egui::vec2(10.0, 0.0))),
                ph_x,
                ruler_rect.top() + 3.0,
                ruler_rect.bottom() - 1.0,
                head.hovered() || dragging_head,
            );
        }
        self.user_dragging_seek = scrubbing;

        if clear {
            if let Some(p) = &mut self.project {
                p.selected_clips.clear();
                p.selected_text = None;
            }
        }
        if let Some(id) = select {
            ui.ctx().request_repaint();
            if let Some(p) = &mut self.project {
                p.selected_text = None;
                let additive = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                if !additive {
                    p.selected_clips.clear();
                }
                if additive && p.selected_clips.contains(&id) {
                    p.selected_clips.remove(&id);
                } else {
                    p.selected_clips.insert(id);
                }
            }
        }
        if let Some(ms) = seek {
            ui.ctx().request_repaint();
            if let Some(p) = &mut self.project {
                p.playhead_ms = ms;
            }
            self.live.reanchor(ms);
        }
        if let Some((ids, muted)) = monitor_toggle {
            for id in ids {
                if muted {
                    self.live.monitor_muted.insert(id);
                } else {
                    self.live.monitor_muted.remove(&id);
                }
            }
            ui.ctx().request_repaint();
        }
        if let Some(id) = mute {
            self.push_undo();
            if let Some(t) = self
                .project
                .as_mut()
                .and_then(|p| p.tracks.iter_mut().find(|t| t.id == id))
            {
                t.muted = !t.muted;
            }
        }
        if let Some((payload, start, target, index)) = drop {
            self.push_undo();
            if let Some(p) = &mut self.project {
                match payload {
                    EditorDrag::Asset(id) => {
                        let first = p.clips.is_empty();
                        p.place_asset(id, start, target, index);
                        if first {
                            p.playhead_ms = start;
                        }
                    }
                    EditorDrag::Clip { id, .. } => p.move_segment(id, start, target, index),
                    EditorDrag::Track(id) => p.reorder_track(id, index),
                }
            }
            ui.ctx().request_repaint();
            egui::DragAndDrop::clear_payload(ui.ctx());
            self.live.pause();
        }
        if let Some(action) = context_action {
            ui.ctx().request_repaint();
            self.apply_clip_action(action);
            if let Some(p) = &mut self.project {
                p.prune_tracks();
            }
        }
        if !band_actions.is_empty() {
            ui.ctx().request_repaint();
            self.apply_band_actions(band_actions, &tracks);
        }
        if self.show_shortcut_footer(ui, footer_h, view.zoom) {
            ui.data_mut(|d| d.insert_temp(view_id, TimelineZoom::default()));
            ui.ctx().request_repaint();
        }
        // The panel remembers the height its content used; claiming the
        // whole rect keeps a short frame from shrinking it for good.
        ui.expand_to_include_rect(ui.max_rect());
    }

    /// Key-cap legend under the timeline, plus the zoom readout. Returns
    /// true when "Fit" was clicked.
    fn show_shortcut_footer(&self, ui: &mut egui::Ui, height: f32, zoom: f32) -> bool {
        let (footer, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let painter = ui.painter().with_clip_rect(footer);
        let mut x = footer.left() + 2.0;
        for (key, action) in [
            ("Space", "Play"),
            ("S", "Split"),
            ("T", "Text"),
            ("Del", "Delete"),
            ("G", "Close gap"),
            ("D", "Detach"),
            ("Shift D", "Attach"),
            ("Ctrl Z", "Undo"),
            ("Ctrl Scroll", "Zoom"),
        ] {
            let key_text =
                painter.layout_no_wrap(key.into(), theme::mono(10.0), theme::TEXT_SECONDARY);
            let key_rect = egui::Rect::from_min_size(
                egui::pos2(x, footer.center().y - 9.0),
                egui::vec2(key_text.size().x + 10.0, 18.0),
            );
            painter.rect_filled(key_rect, 4.0, theme::BG_STATUS);
            painter.rect_stroke(key_rect, 4.0, Stroke::new(1.0_f32, theme::BORDER));
            // Key-cap lip: a darker bottom edge reads as a physical key.
            painter.line_segment(
                [
                    key_rect.left_bottom() + egui::vec2(3.0, -0.5),
                    key_rect.right_bottom() + egui::vec2(-3.0, -0.5),
                ],
                Stroke::new(1.0_f32, theme::BG_PREVIEW),
            );
            painter.galley(
                egui::pos2(
                    key_rect.left() + 5.0,
                    key_rect.center().y - key_text.size().y * 0.5,
                ),
                key_text,
                theme::TEXT_SECONDARY,
            );
            let label =
                painter.layout_no_wrap(action.into(), theme::regular(11.0), theme::TEXT_MUTED);
            let label_w = label.size().x;
            painter.galley(
                egui::pos2(
                    key_rect.right() + 6.0,
                    footer.center().y - label.size().y * 0.5,
                ),
                label,
                theme::TEXT_MUTED,
            );
            x = key_rect.right() + 6.0 + label_w + 18.0;
        }
        if zoom <= 1.001 {
            return false;
        }
        // Zoomed in: show the factor and a one-click way back to Fit.
        let fit = egui::Rect::from_min_size(
            egui::pos2(footer.right() - 46.0, footer.center().y - 10.0),
            egui::vec2(46.0, 20.0),
        );
        let response = ui
            .interact(
                fit,
                ui.id().with("timeline_fit"),
                egui::Sense {
                    click: true,
                    drag: false,
                    focusable: false,
                },
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Show the whole timeline");
        painter.rect_filled(
            fit,
            5.0,
            if response.hovered() {
                theme::BG_BUTTON_HOVER
            } else {
                theme::BG_BUTTON
            },
        );
        painter.text(
            fit.center(),
            Align2::CENTER_CENTER,
            "Fit",
            theme::medium(11.5),
            theme::TEXT_PRIMARY,
        );
        painter.text(
            fit.left_center() - egui::vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            format!("{:.1}×", zoom),
            theme::mono(10.5),
            theme::TEXT_MUTED,
        );
        response.clicked()
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    fn video(p: &mut Project) -> TrackId {
        p.import_asset(track::LayerSource {
            path: "drag.mp4".into(),
            media: MediaInfo {
                duration_ms: 5000,
                width: 1920,
                height: 1080,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: None,
                audio_stream_count: 1,
            },
        })
    }
    #[test]
    fn quick_first_drop_starts_at_zero_until_a_deliberate_sideways_move() {
        let mut p = Project::empty();
        let asset = video(&mut p);
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 200.0));
        let drag = EditorDrag::Asset(asset);
        let hover = |x| {
            placement(
                &p,
                &[],
                &[],
                &drag,
                egui::pos2(x, 20.0),
                Lanes::new(rect, p.canvas_length_ms()),
                None,
                &ctx,
            )
            .unwrap()
            .start
        };
        assert_eq!(hover(300.0), 0);
        assert_eq!(hover(315.0), 0);
        assert!(hover(350.0) > 2000);
        assert!(hover(315.0) > 0); // remains deliberately unlocked after moving back
    }
    #[test]
    fn additional_media_always_opens_a_distinct_lane_above_or_below() {
        let mut p = Project::empty();
        let asset = video(&mut p);
        p.place_asset(asset, 0, None, 0);
        let (tracks, clips, _) = p.timeline_view();
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 240.0));
        let drag = EditorDrag::Asset(asset);
        let above = placement(
            &p,
            &tracks,
            &clips,
            &drag,
            egui::pos2(HEADER_W, 10.0),
            Lanes::new(rect, p.canvas_length_ms()),
            None,
            &ctx,
        )
        .unwrap();
        let below = placement(
            &p,
            &tracks,
            &clips,
            &drag,
            egui::pos2(HEADER_W, LANE_HEIGHT - 8.0),
            Lanes::new(rect, p.canvas_length_ms()),
            None,
            &ctx,
        )
        .unwrap();
        assert_eq!((above.slot, above.target), (0, None));
        assert_eq!((below.slot, below.target), (1, None));
    }
    #[test]
    fn zoom_keeps_the_time_under_the_pointer_and_stays_in_bounds() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1138.0, 100.0));
        let duration = 20_000;
        let mut view = TimelineZoom::default();
        let fit = Lanes::window(rect, view.start, view.span(duration));
        let pointer = fit.left + fit.width * 0.25;
        let before = fit.time_at(pointer);
        view.zoom_at(4.0, before, 0.25, duration);
        let zoomed = Lanes::window(rect, view.start, view.span(duration));
        assert_eq!(view.span(duration), 5_000);
        assert!((zoomed.time_at(pointer) - before).abs() <= 5);
        // Zooming out past Fit snaps back to the whole timeline.
        view.zoom_at(0.01, 0, 0.0, duration);
        assert_eq!((view.zoom, view.start), (1.0, 0));
        // Panning never leaves the timeline.
        view.zoom_at(10.0, 19_000, 1.0, duration);
        view.pan(60_000, duration);
        assert_eq!(view.start + view.span(duration), duration);
        view.pan(-60_000, duration);
        assert_eq!(view.start, 0);
    }
    #[test]
    fn playback_pages_a_zoomed_view_to_keep_the_playhead_visible() {
        let duration = 60_000;
        let mut view = TimelineZoom {
            zoom: 6.0,
            start: 0,
        };
        view.follow(15_000, duration);
        assert!(view.start <= 15_000 && 15_000 <= view.start + view.span(duration));
    }
    #[test]
    fn ruler_ticks_never_repeat_whole_second_labels() {
        assert_eq!(ruler_step(13388, 1800.0), 1000);
        assert!(ruler_step(2000, 1200.0) < 1000); // fractional labels at close scale
    }
}

#[cfg(test)]
mod lane_drag_tests {
    use super::*;
    use textui::LaneTarget::{Lane, New};

    /// T1, V1, V2, A1 (top to bottom), as the timeline shows them.
    fn lanes() -> Vec<Track> {
        let mut p = Project::empty();
        let v2 = p.new_track(TrackKind::Video, usize::MAX);
        p.new_track(TrackKind::Audio, usize::MAX);
        p.new_track(TrackKind::Video, 0);
        p.new_track(TrackKind::Text, 0);
        let mut view = p.tracks.clone();
        view.sort_by_key(|t| {
            (
                !t.kind.is_visual(),
                t.id != v2 && t.kind == TrackKind::Video,
            )
        });
        view.sort_by_key(|t| match t.kind {
            TrackKind::Text => 0,
            TrackKind::Video if t.id == v2 => 2,
            TrackKind::Video => 1,
            TrackKind::Audio => 3,
        });
        view
    }
    const H: f32 = LANE_HEIGHT;
    fn mid(row: usize) -> f32 {
        (row as f32 + 0.5) * H
    }

    #[test]
    fn middle_of_a_free_lane_of_its_kind_targets_it_edges_insert() {
        let t = lanes();
        let fits = |_: TrackId| true;
        let ask = LaneAsk {
            kind: TrackKind::Video,
            own: Some(t[2].id),
            alone: false,
            fits: &fits,
        };
        assert_eq!(resolve_lane(mid(1), None, &t, &ask), Lane(t[1].id));
        assert_eq!(resolve_lane(H + 3.0, None, &t, &ask), New(1));
        assert_eq!(resolve_lane(2.0 * H - 3.0, None, &t, &ask), New(2));
        // Never among the audio lanes, never above them for audio.
        assert_eq!(resolve_lane(mid(3), None, &t, &ask), New(3));
    }

    #[test]
    fn an_occupied_lane_opens_a_layer_above_and_its_own_lane_holds() {
        let t = lanes();
        let (v1, v2) = (t[1].id, t[2].id);
        let fits = move |id: TrackId| id != v1;
        let ask = LaneAsk {
            kind: TrackKind::Video,
            own: Some(v2),
            alone: false,
            fits: &fits,
        };
        assert_eq!(resolve_lane(mid(1), None, &t, &ask), New(1));
        // A little vertical wobble on its own lane never opens a lane.
        assert_eq!(resolve_lane(2.0 * H + 2.0, None, &t, &ask), Lane(v2));
    }

    #[test]
    fn an_open_gap_stays_while_the_pointer_is_in_it() {
        let t = lanes();
        let fits = |_: TrackId| true;
        let ask = LaneAsk {
            kind: TrackKind::Text,
            own: Some(t[0].id),
            alone: false,
            fits: &fits,
        };
        // Gap open at row 1 (between T1 and V1): anywhere in it keeps it.
        assert_eq!(resolve_lane(H + 1.0, Some(1), &t, &ask), New(1));
        assert_eq!(resolve_lane(2.0 * H - 1.0, Some(1), &t, &ask), New(1));
        // Below it, rows read one lower: drawn row 2 is V1.
        assert_eq!(resolve_lane(mid(2), Some(1), &t, &ask), New(2));
    }

    #[test]
    fn a_new_lane_right_next_to_its_only_occupant_changes_nothing() {
        let t = lanes();
        let fits = |_: TrackId| true;
        let ask = LaneAsk {
            kind: TrackKind::Text,
            own: Some(t[0].id),
            alone: true,
            fits: &fits,
        };
        assert_eq!(resolve_lane(H + 3.0, None, &t, &ask), Lane(t[0].id));
        assert_eq!(resolve_lane(2.0 * H - 3.0, None, &t, &ask), New(2));
    }
}
