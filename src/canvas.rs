use super::*;
use crate::framing::Background;
use crate::track::Transform;
use crate::{picker, widgets};

pub(super) fn layer_rect(canvas: egui::Rect, width: u32, height: u32, t: Transform) -> egui::Rect {
    let ratios = (
        canvas.width() / width.max(1) as f32,
        canvas.height() / height.max(1) as f32,
    );
    let fit = match t.fit {
        crate::framing::Fit::Fit => ratios.0.min(ratios.1),
        crate::framing::Fit::Fill => ratios.0.max(ratios.1),
    };
    let size = egui::vec2(width as f32 * fit * t.scale, height as f32 * fit * t.scale);
    egui::Rect::from_center_size(
        egui::pos2(
            canvas.left() + canvas.width() * t.x,
            canvas.top() + canvas.height() * t.y,
        ),
        size,
    )
}
pub(super) struct CanvasUi {
    pub(super) background_open: bool,
    aspect_open: bool,
    picker: picker::PickerState,
    color_editing: bool,
    wheel_clip: Option<ClipId>,
    wheel_time: f64,
    wheel_blocked: bool,
    outline_clip: Option<ClipId>,
    outline_until: f64,
    /// Snap guides for the drag in progress (empty when not snapped).
    pub(super) guides: Vec<Guide>,
}
impl Default for CanvasUi {
    fn default() -> Self {
        Self {
            background_open: false,
            aspect_open: false,
            picker: picker::PickerState::new([24, 26, 31]),
            color_editing: false,
            wheel_clip: None,
            wheel_time: -1.0,
            wheel_blocked: false,
            outline_clip: None,
            outline_until: 0.0,
            guides: Vec::new(),
        }
    }
}
impl CanvasUi {
    pub(super) fn stop_wheel(&mut self) {
        self.wheel_blocked = true;
        self.wheel_clip = None;
    }
}
fn zoom_at(
    canvas: egui::Rect,
    width: u32,
    height: u32,
    mut transform: Transform,
    pointer: egui::Pos2,
    scroll: f32,
) -> Transform {
    let rect = layer_rect(canvas, width, height, transform);
    let scale = (transform.scale * (scroll * 0.0035).exp()).clamp(0.03, 8.0);
    let center = pointer + (rect.center() - pointer) * (scale / transform.scale);
    transform.scale = scale;
    transform.x = (center.x - canvas.left()) / canvas.width();
    transform.y = (center.y - canvas.top()) / canvas.height();
    transform
}
/// Screen-pixel radius inside which an edge locks to a guide. Firm
/// enough to square a layer up without aiming; dragging further breaks
/// free, and Alt turns snapping off entirely.
const SNAP_PX: f32 = 10.0;

/// Lines a moving layer can lock to: the canvas edges and center, plus
/// the edges and centers of the other visible layers.
#[derive(Debug, Default)]
pub(super) struct SnapTargets {
    xs: Vec<f32>,
    ys: Vec<f32>,
}
impl SnapTargets {
    pub(super) fn new(canvas: egui::Rect, others: impl IntoIterator<Item = egui::Rect>) -> Self {
        let mut t = Self {
            xs: vec![canvas.left(), canvas.center().x, canvas.right()],
            ys: vec![canvas.top(), canvas.center().y, canvas.bottom()],
        };
        for r in others {
            t.xs.extend([r.left(), r.center().x, r.right()]);
            t.ys.extend([r.top(), r.center().y, r.bottom()]);
        }
        t
    }
}

/// A guide drawn while snapped: vertical (`x`) or horizontal (`y`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Guide {
    X(f32),
    Y(f32),
}

/// Smallest correction that puts one of `edges` on one of `targets`,
/// with the target hit, if any lies within `SNAP_PX`.
fn nearest(edges: &[f32], targets: &[f32]) -> Option<(f32, f32)> {
    edges
        .iter()
        .flat_map(|e| targets.iter().map(move |t| (t - e, *t)))
        .filter(|(d, _)| d.abs() <= SNAP_PX)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
}

/// Snaps a moved layer: each axis independently aligns its nearest edge
/// or center. Returns the corrected rect plus the guides to draw.
pub(super) fn snap_move(rect: egui::Rect, targets: &SnapTargets) -> (egui::Rect, Vec<Guide>) {
    let mut guides = Vec::new();
    let mut offset = egui::Vec2::ZERO;
    if let Some((dx, x)) = nearest(&[rect.left(), rect.center().x, rect.right()], &targets.xs) {
        offset.x = dx;
        guides.push(Guide::X(x));
    }
    if let Some((dy, y)) = nearest(&[rect.top(), rect.center().y, rect.bottom()], &targets.ys) {
        offset.y = dy;
        guides.push(Guide::Y(y));
    }
    (rect.translate(offset), guides)
}

/// Snaps a corner resize. The opposite corner (`anchor`) is fixed and the
/// aspect ratio is locked, so only one axis can lock: whichever free edge
/// is closer to a guide decides the scale.
fn snap_resize(
    anchor: egui::Pos2,
    corner: (f32, f32),
    size: egui::Vec2,
    targets: &SnapTargets,
) -> Option<(f32, Guide)> {
    let (sx, sy) = corner;
    let free = anchor + egui::vec2(sx * size.x, sy * size.y);
    let x = nearest(&[free.x], &targets.xs)
        .map(|(d, t)| (d.abs(), (t - anchor.x).abs() / size.x.max(1.0), Guide::X(t)));
    let y = nearest(&[free.y], &targets.ys)
        .map(|(d, t)| (d.abs(), (t - anchor.y).abs() / size.y.max(1.0), Guide::Y(t)));
    [x, y]
        .into_iter()
        .flatten()
        .filter(|(_, factor, _)| *factor > 0.02)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, factor, guide)| (factor, guide))
}

/// Reset framing that suits the canvas: a source whose shape is close to
/// the canvas fills it (no hairline bars); a clearly different shape —
/// landscape gameplay in a 9:16 canvas — fits whole and centered, with
/// bars, so nothing is cropped away.
pub(super) fn reset_transform(source: (u32, u32), canvas: (u32, u32), opacity: f32) -> Transform {
    let aspect = |(w, h): (u32, u32)| w.max(1) as f32 / h.max(1) as f32;
    let (a, b) = (aspect(source), aspect(canvas));
    let mismatch = a.max(b) / a.min(b);
    Transform {
        fit: if mismatch > 1.12 {
            crate::framing::Fit::Fit
        } else {
            crate::framing::Fit::Fill
        },
        opacity,
        ..Default::default()
    }
}

impl ClipperApp {
    /// How "Fit to screen" frames this clip on the current canvas.
    fn fitted_transform(&self, id: ClipId) -> Option<Transform> {
        let p = self.project.as_ref()?;
        let media = p.canvas_media();
        let canvas = (media.width, media.height);
        let clip = track::clip_by_id(&p.clips, id)?;
        let m = p.clip_source(clip).1;
        Some(reset_transform(
            (m.width, m.height),
            canvas,
            clip.transform.opacity,
        ))
    }

    /// The clip shows where "Fit to screen" would put it. Compares the
    /// drawn frame, not the fields: Fit and Fill look the same when the
    /// shapes match.
    pub(super) fn frame_is_fitted(&self, id: ClipId) -> bool {
        let Some(p) = self.project.as_ref() else {
            return true;
        };
        let (Some(clip), Some(fitted)) =
            (track::clip_by_id(&p.clips, id), self.fitted_transform(id))
        else {
            return true;
        };
        let media = p.canvas_media();
        let canvas = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(media.width.max(1) as f32, media.height.max(1) as f32),
        );
        let m = p.clip_source(clip).1;
        let a = layer_rect(canvas, m.width, m.height, clip.transform);
        let b = layer_rect(canvas, m.width, m.height, fitted);
        (a.min - b.min).length() < 0.5 && (a.max - b.max).length() < 0.5
    }

    /// Centers the clip: fills the canvas when the shapes match, fits it
    /// whole with bars when they don't (gameplay in 9:16). One undo step.
    pub(super) fn fit_to_screen(&mut self, id: ClipId) {
        let Some(fitted) = self.fitted_transform(id) else {
            return;
        };
        self.canvas_ui.stop_wheel();
        self.push_undo();
        if let Some(clip) = self
            .project
            .as_mut()
            .and_then(|p| p.clips.iter_mut().find(|c| c.id == id))
        {
            clip.transform = fitted;
        }
    }
}

#[derive(Clone)]
pub(super) struct CanvasDrag {
    clip: ClipId,
    origin: Transform,
    pointer: egui::Pos2,
    rect: egui::Rect,
    corner: Option<(f32, f32)>,
}
impl ClipperApp {
    pub(super) fn show_canvas(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project.as_ref() else {
            return;
        };
        let media = p.canvas_media();
        let mut settings = p.canvas;
        let previous = settings;
        let focused = p.selected_clips.iter().copied().find(|id| p.is_video(*id));
        let mut reset: Option<ClipId> = None;
        let mut add_text = false;
        if let Background::Color(rgb) = settings.background {
            // Follow undo/redo and project loads without disturbing a hue
            // the picker is holding for a grey.
            if self.canvas_ui.picker.rgb() != rgb {
                self.canvas_ui.picker.set_rgb(rgb);
            }
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let aspect_label = settings.aspect.label();
            let picked_aspect = &mut settings.aspect;
            widgets::dropdown_button(
                ui,
                "canvas_aspect",
                &mut self.canvas_ui.aspect_open,
                egui::vec2(196.0, 28.0),
                220.0,
                |painter, rect, color| {
                    painter.text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        aspect_label,
                        theme::medium(12.5),
                        color,
                    );
                },
                |ui| {
                    let mut chosen = false;
                    for aspect in crate::framing::Aspect::ALL {
                        if widgets::menu_choice(ui, aspect.label(), *picked_aspect == aspect)
                            .clicked()
                        {
                            *picked_aspect = aspect;
                            chosen = true;
                        }
                    }
                    chosen
                },
            )
            .on_hover_text("Output shape");

            let shown_background = settings.background;
            let background = &mut settings.background;
            let picker = &mut self.canvas_ui.picker;
            widgets::dropdown_button(
                ui,
                "canvas_background",
                &mut self.canvas_ui.background_open,
                egui::vec2(150.0, 28.0),
                256.0,
                |painter, rect, color| {
                    let swatch = egui::Rect::from_center_size(
                        rect.left_center() + egui::vec2(7.0, 0.0),
                        egui::vec2(14.0, 14.0),
                    );
                    paint_background_swatch(painter, swatch, shown_background);
                    painter.text(
                        swatch.right_center() + egui::vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "Background",
                        theme::medium(12.5),
                        color,
                    );
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(4.0);
                    let selected = match background {
                        Background::Black => 0,
                        Background::Blur => 1,
                        Background::Color(_) => 2,
                    };
                    ui.vertical_centered(|ui| {
                        let options = [
                            ("Black", ""),
                            (
                                "Blur video",
                                "A blurred, zoomed copy of the video fills the bars",
                            ),
                            ("Color", ""),
                        ];
                        match widgets::segmented(ui, "canvas_background_mode", &options, selected) {
                            Some(0) => *background = Background::Black,
                            Some(1) => *background = Background::Blur,
                            Some(_) => *background = Background::Color(picker.rgb()),
                            None => {}
                        }
                    });
                    if let Background::Color(_) = background {
                        ui.add_space(10.0);
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(4.0, 0.0))
                            .show(ui, |ui| {
                                if picker::show(ui, picker) {
                                    *background = Background::Color(picker.rgb());
                                }
                            });
                    }
                    ui.add_space(4.0);
                    false
                },
            );

            let moved = focused.is_some_and(|id| !self.frame_is_fitted(id));
            if crate::widgets::fit_button(ui, moved, None).clicked() {
                reset = focused;
            }
            if ui
                .button("Add text")
                .on_hover_text(
                    "New text layer at the playhead (T). Drag it anywhere on the preview.",
                )
                .clicked()
            {
                add_text = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} × {}", media.width, media.height))
                        .font(theme::mono(11.5))
                        .color(theme::TEXT_MUTED),
                )
                .on_hover_text("Uses the highest resolution video placed on the timeline");
            });
        });
        let editing = ui.input(|i| i.pointer.any_down()) || ui.ctx().wants_keyboard_input();
        if settings != previous {
            ui.ctx().request_repaint();
            if !self.canvas_ui.color_editing {
                self.push_undo();
            }
            self.canvas_ui.color_editing = editing;
            if let Some(p) = &mut self.project {
                p.canvas = settings;
            }
        }
        if !editing {
            self.canvas_ui.color_editing = false;
        }
        if add_text {
            self.add_text_layer();
        }
        ui.add_space(8.0);
        let available = ui.available_rect_before_wrap();
        let aspect = media.width.max(1) as f32 / media.height.max(1) as f32;
        let w = available.width().min(available.height() * aspect).max(1.0);
        let h = w / aspect;
        let canvas = egui::Rect::from_center_size(available.center(), egui::vec2(w, h));
        let rgb = settings.background.rgb();
        ui.painter()
            .rect_filled(canvas, 2.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        let bg = ui.interact(
            canvas,
            ui.id().with("canvas_background"),
            egui::Sense::click(),
        );
        let frames = self.live.frames.clone();
        let mut selected = self
            .project
            .as_ref()
            .and_then(|p| p.selected_clips.iter().next().copied());
        let mut start_drag = None;
        let mut drag_pointer = None;
        let mut stopped = false;
        let mut clicked = false;
        for layer in &frames {
            let rect = layer_rect(canvas, layer.width, layer.height, layer.transform);
            let response = ui
                .interact(
                    rect.intersect(canvas),
                    ui.id().with(("video_layer", layer.id)),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::Move);
            response.context_menu(|ui| {
                if ui.button("Fit to screen").clicked() {
                    reset = Some(layer.id);
                    ui.close_menu();
                }
            });
            if response.clicked() || response.drag_started() {
                selected = Some(layer.id);
                clicked = true;
            }
            if response.drag_started() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    start_drag = Some(CanvasDrag {
                        clip: layer.id,
                        origin: layer.transform,
                        pointer: ui.input(|i| i.pointer.press_origin()).unwrap_or(pointer),
                        rect,
                        corner: None,
                    });
                }
            }
            if response.dragged() {
                drag_pointer = response.interact_pointer_pos();
            }
            stopped |= response.drag_stopped();
        }
        let now = ui.input(|i| i.time);
        let moving =
            ui.rect_contains_pointer(canvas) && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        if selected != self.canvas_ui.outline_clip
            || moving
            || clicked
            || start_drag.is_some()
            || drag_pointer.is_some()
        {
            self.canvas_ui.outline_clip = selected;
            self.canvas_ui.outline_until = now + 1.8;
        }
        if let Some(layer) = frames.iter().find(|layer| Some(layer.id) == selected) {
            let rect = layer_rect(canvas, layer.width, layer.height, layer.transform);
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let point =
                    rect.center() + egui::vec2(sx * rect.width() * 0.5, sy * rect.height() * 0.5);
                let hit = egui::Rect::from_center_size(point, egui::vec2(20.0, 20.0))
                    .intersect(canvas.expand(4.0));
                let response = ui
                    .interact(
                        hit,
                        ui.id().with(("resize", layer.id, sx as i32, sy as i32)),
                        egui::Sense::drag(),
                    )
                    .on_hover_cursor(if sx == sy {
                        egui::CursorIcon::ResizeNwSe
                    } else {
                        egui::CursorIcon::ResizeNeSw
                    });
                if response.drag_started() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        start_drag = Some(CanvasDrag {
                            clip: layer.id,
                            origin: layer.transform,
                            pointer,
                            rect,
                            corner: Some((sx, sy)),
                        });
                    }
                }
                if response.dragged() {
                    drag_pointer = response.interact_pointer_pos();
                }
                stopped |= response.drag_stopped();
            }
        }
        // Text layers sit above the video, so they register last and win
        // overlapping hits.
        let video_rects: Vec<egui::Rect> = frames
            .iter()
            .map(|f| layer_rect(canvas, f.width, f.height, f.transform))
            .collect();
        let text_took = self.interact_canvas_texts(ui, canvas, &video_rects);
        if text_took {
            selected = None;
            start_drag = None;
            drag_pointer = None;
        } else {
            if bg.clicked() && !clicked {
                selected = None;
            }
            if clicked || bg.clicked() || start_drag.is_some() {
                if let Some(p) = &mut self.project {
                    p.selected_clips = selected.into_iter().collect();
                    p.selected_text = None;
                }
            }
        }
        if let Some(drag) = start_drag {
            self.canvas_ui.stop_wheel();
            self.push_undo();
            self.live.pause();
            self.canvas_drag = Some(drag);
        }
        if self.canvas_drag.is_none() && !self.text_dragging() {
            self.canvas_ui.guides.clear();
        }
        if let (Some(drag), Some(pointer)) = (&self.canvas_drag, drag_pointer) {
            let mut transform = drag.origin;
            let snapping = !ui.input(|i| i.modifiers.alt);
            let targets = SnapTargets::new(
                canvas,
                frames
                    .iter()
                    .filter(|f| f.id != drag.clip)
                    .map(|f| layer_rect(canvas, f.width, f.height, f.transform)),
            );
            self.canvas_ui.guides.clear();
            if let Some((sx, sy)) = drag.corner {
                let anchor = drag.rect.center()
                    - egui::vec2(sx * drag.rect.width() * 0.5, sy * drag.rect.height() * 0.5);
                let relative = egui::vec2((pointer.x - anchor.x) * sx, (pointer.y - anchor.y) * sy);
                let diagonal = drag.rect.size();
                let mut factor = (relative.dot(diagonal) / diagonal.length_sq().max(1.0)).max(0.02);
                if snapping {
                    if let Some((snapped, guide)) =
                        snap_resize(anchor, (sx, sy), diagonal * factor, &targets)
                            .map(|(f, g)| (f * factor, g))
                    {
                        factor = snapped;
                        self.canvas_ui.guides.push(guide);
                    }
                }
                transform.scale = (drag.origin.scale * factor).clamp(0.03, 8.0);
                let size = drag.rect.size() * (transform.scale / drag.origin.scale);
                let center = anchor + egui::vec2(sx * size.x * 0.5, sy * size.y * 0.5);
                transform.x = (center.x - canvas.left()) / canvas.width();
                transform.y = (center.y - canvas.top()) / canvas.height();
            } else {
                let mut moved = drag.rect.translate(pointer - drag.pointer);
                if snapping {
                    let (snapped, guides) = snap_move(moved, &targets);
                    moved = snapped;
                    self.canvas_ui.guides = guides;
                }
                transform.x = (moved.center().x - canvas.left()) / canvas.width();
                transform.y = (moved.center().y - canvas.top()) / canvas.height();
            }
            if let Some(c) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|c| c.id == drag.clip))
            {
                c.transform = transform;
            }
            ui.ctx().request_repaint();
        }
        if stopped {
            self.canvas_drag = None;
        }
        if ui.input(|i| i.raw_scroll_delta.y != 0.0) {
            self.canvas_ui.wheel_blocked = false;
        }
        if !self.canvas_ui.background_open
            && !self.canvas_ui.wheel_blocked
            && self.canvas_drag.is_none()
            && ui.rect_contains_pointer(canvas)
        {
            let zoom = selected
                .and_then(|id| frames.iter().find(|f| f.id == id))
                .and_then(|frame| {
                    ui.input_mut(|i| {
                        let delta = i.smooth_scroll_delta.y;
                        if delta.abs() < 0.001 {
                            return None;
                        }
                        let pointer = i.pointer.hover_pos()?;
                        i.smooth_scroll_delta.y = 0.0;
                        i.raw_scroll_delta.y = 0.0;
                        Some((
                            frame.id,
                            zoom_at(
                                canvas,
                                frame.width,
                                frame.height,
                                frame.transform,
                                pointer,
                                delta,
                            ),
                            i.time,
                        ))
                    })
                });
            if let Some((id, transform, time)) = zoom {
                if self.canvas_ui.wheel_clip != Some(id) || time - self.canvas_ui.wheel_time > 0.35
                {
                    self.push_undo();
                }
                self.canvas_ui.outline_until = now + 1.8;
                self.canvas_ui.wheel_clip = Some(id);
                self.canvas_ui.wheel_time = time;
                if let Some(clip) = self
                    .project
                    .as_mut()
                    .and_then(|p| p.clips.iter_mut().find(|c| c.id == id))
                {
                    clip.transform = transform;
                }
                ui.ctx().request_repaint();
            }
        }
        if let Some(id) = reset {
            self.fit_to_screen(id);
            if let Some(p) = &mut self.project {
                p.selected_clips = [id].into_iter().collect();
            }
            selected = Some(id);
            self.canvas_ui.outline_until = now + 1.8;
            ui.ctx().request_repaint();
        }
        for frame in &mut self.live.frames {
            if let Some(c) = self
                .project
                .as_ref()
                .and_then(|p| track::clip_by_id(&p.clips, frame.id))
            {
                let at = self.project.as_ref().map_or(0, |p| p.playhead_ms);
                (frame.transform, frame.flash) =
                    c.animation
                        .sample(c.transform, at - c.cut_start_ms, c.duration_ms());
            }
        }
        self.paint_canvas_layers(ui, canvas, settings.background);
        self.show_inline_text_editor(ui, canvas);
        let visible = now < self.canvas_ui.outline_until || self.canvas_drag.is_some();
        if visible {
            ui.ctx().request_repaint_after(Duration::from_secs_f64(
                (self.canvas_ui.outline_until - now).max(0.01),
            ));
        }
        let fade =
            ui.ctx()
                .animate_bool_with_time(ui.id().with("selection_outline"), visible, 0.15);
        if let Some(layer) = self
            .live
            .frames
            .iter()
            .find(|layer| Some(layer.id) == selected && fade > 0.0)
        {
            let rect = layer_rect(canvas, layer.width, layer.height, layer.transform);
            let painter = ui.painter().with_clip_rect(canvas.expand(5.0));
            painter.rect_stroke(
                rect,
                1.0,
                Stroke::new(1.5_f32, theme::ACCENT.gamma_multiply(fade)),
            );
            for point in [
                rect.left_top(),
                rect.right_top(),
                rect.left_bottom(),
                rect.right_bottom(),
            ] {
                painter.rect_filled(
                    egui::Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                    1.0,
                    theme::TEXT_PRIMARY.gamma_multiply(fade),
                );
                painter.rect_stroke(
                    egui::Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                    1.0,
                    Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(fade)),
                );
            }
        }
        if !self.canvas_ui.guides.is_empty() {
            let painter = ui.painter().with_clip_rect(canvas.expand(1.0));
            let stroke = Stroke::new(1.0_f32, theme::ACCENT);
            for guide in &self.canvas_ui.guides {
                let points = match *guide {
                    Guide::X(x) => [egui::pos2(x, canvas.top()), egui::pos2(x, canvas.bottom())],
                    Guide::Y(y) => [egui::pos2(canvas.left(), y), egui::pos2(canvas.right(), y)],
                };
                painter.line_segment(points, stroke);
            }
        }
        if self.project.as_ref().is_some_and(|p| p.clips.is_empty()) {
            ui.painter().text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "Drag media onto the timeline",
                egui::FontId::proportional(16.0),
                theme::TEXT_MUTED,
            );
        }
        ui.allocate_rect(available, egui::Sense::hover());
    }
}
/// The Background button's swatch: the solid color, or a soft two-tone
/// tile standing in for the blurred video.
fn paint_background_swatch(painter: &egui::Painter, rect: egui::Rect, background: Background) {
    match background {
        Background::Blur => {
            let (top, bottom) = rect.split_top_bottom_at_fraction(0.5);
            painter.rect_filled(
                top,
                egui::Rounding {
                    nw: 3.0,
                    ne: 3.0,
                    sw: 0.0,
                    se: 0.0,
                },
                Color32::from_rgb(92, 118, 150),
            );
            painter.rect_filled(
                bottom,
                egui::Rounding {
                    nw: 0.0,
                    ne: 0.0,
                    sw: 3.0,
                    se: 3.0,
                },
                Color32::from_rgb(52, 70, 64),
            );
        }
        other => {
            let [r, g, b] = other.rgb();
            painter.rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));
        }
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_zoom_keeps_the_point_under_the_pointer_and_is_reversible() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
        let t = Transform::default();
        let pointer = egui::pos2(600.0, 300.0);
        let before = layer_rect(canvas, 1920, 1080, t);
        let zoomed = zoom_at(canvas, 1920, 1080, t, pointer, 100.0);
        let after = layer_rect(canvas, 1920, 1080, zoomed);
        let a = (pointer - before.min) / before.size();
        let b = (pointer - after.min) / after.size();
        assert!((a.x - b.x).abs() < 0.0001 && (a.y - b.y).abs() < 0.0001);
        let restored = zoom_at(canvas, 1920, 1080, zoomed, pointer, -100.0);
        assert!((restored.scale - t.scale).abs() < 0.0001);
        assert!((restored.x - t.x).abs() < 0.0001 && (restored.y - t.y).abs() < 0.0001);
    }
    #[test]
    fn moving_near_an_edge_locks_flush_and_far_moves_stay_free() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let targets = SnapTargets::new(canvas, []);
        let near = egui::Rect::from_min_size(egui::pos2(7.0, 100.0), egui::vec2(300.0, 200.0));
        let (snapped, guides) = snap_move(near, &targets);
        assert_eq!(snapped.left(), 0.0);
        assert!(guides.contains(&Guide::X(0.0)));
        let far = near.translate(egui::vec2(40.0, 0.0));
        assert_eq!(snap_move(far, &targets).0.left(), far.left());
    }
    #[test]
    fn corner_resize_snaps_the_free_edge_to_the_canvas() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 500.0));
        let targets = SnapTargets::new(canvas, []);
        // Anchor top-left at the origin; bottom-right corner lands 6 px
        // short of the right edge.
        let (factor, guide) = snap_resize(
            egui::Pos2::ZERO,
            (1.0, 1.0),
            egui::vec2(994.0, 300.0),
            &targets,
        )
        .unwrap();
        assert_eq!(guide, Guide::X(1000.0));
        assert!((994.0 * factor - 1000.0).abs() < 0.01);
    }
    #[test]
    fn reset_fits_mismatched_shapes_and_fills_close_ones() {
        let vertical = reset_transform((1920, 1080), (1080, 1920), 1.0);
        assert_eq!(vertical.fit, crate::framing::Fit::Fit);
        assert_eq!((vertical.x, vertical.y, vertical.scale), (0.5, 0.5, 1.0));
        let near = reset_transform((1920, 1080), (1920, 1200), 0.5);
        assert_eq!(near.fit, crate::framing::Fit::Fill);
        assert_eq!(near.opacity, 0.5);
    }
    #[test]
    fn a_full_frame_layer_can_move_and_resize_freely() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
        let mut t = Transform::default();
        assert_eq!(layer_rect(canvas, 1920, 1080, t), canvas);
        t.x = 0.75;
        let moved = layer_rect(canvas, 1920, 1080, t);
        assert_eq!(moved.left(), 480.0);
        t.scale = 0.5;
        let small = layer_rect(canvas, 1920, 1080, t);
        assert_eq!(small.width(), 960.0);
        assert_eq!(small.center().x, 1440.0);
    }
}
