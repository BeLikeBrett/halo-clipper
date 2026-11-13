use super::*;
use crate::track::Transform;

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
    color: egui::ecolor::Hsva,
    color_editing: bool,
    wheel_clip: Option<ClipId>,
    wheel_time: f64,
    wheel_blocked: bool,
    outline_clip: Option<ClipId>,
    outline_until: f64,
}
impl Default for CanvasUi {
    fn default() -> Self {
        Self {
            background_open: false,
            color: Color32::from_rgb(32, 32, 32).into(),
            color_editing: false,
            wheel_clip: None,
            wheel_time: -1.0,
            wheel_blocked: false,
            outline_clip: None,
            outline_until: 0.0,
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
        let mut background_anchor = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Preview").small().color(theme::TEXT_MUTED));
            egui::ComboBox::from_id_salt("canvas_format")
                .selected_text(settings.aspect.label())
                .width(174.0)
                .show_ui(ui, |ui| {
                    for aspect in crate::framing::Aspect::ALL {
                        ui.selectable_value(&mut settings.aspect, aspect, aspect.label());
                    }
                });
            let background =
                ui.add(egui::Button::new("Background").selected(self.canvas_ui.background_open));
            if background.clicked() {
                self.canvas_ui.background_open = !self.canvas_ui.background_open;
                if self.canvas_ui.background_open {
                    background_anchor = Some(background.rect.left_bottom() + egui::vec2(0.0, 6.0));
                }
                if let crate::framing::Background::Color(rgb) = settings.background {
                    self.canvas_ui.color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]).into();
                }
            }
            if ui
                .add_enabled(focused.is_some(), egui::Button::new("Reset frame"))
                .on_hover_text("Center the selected video and fill the canvas. Ctrl+Z to undo.")
                .clicked()
            {
                reset = focused;
            }
            ui.label(
                RichText::new(format!("{} × {}", media.width, media.height))
                    .small()
                    .color(theme::TEXT_MUTED),
            )
            .on_hover_text("Uses the highest resolution video placed on the timeline");
        });
        if self.canvas_ui.background_open {
            let mut open = true;
            let mut done = false;
            let mut editor = egui::Window::new("Background")
                .id(egui::Id::new("canvas_background_editor"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .order(egui::Order::Foreground)
                .vscroll(true)
                .max_height((ui.ctx().screen_rect().height() - 100.0).max(240.0));
            if let Some(anchor) = background_anchor {
                editor = editor.current_pos(anchor);
            }
            editor.show(ui.ctx(), |ui| {
                ui.spacing_mut().slider_width =
                    (ui.ctx().screen_rect().height() - 300.0).clamp(140.0, 240.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut settings.background,
                        crate::framing::Background::Black,
                        "Black",
                    );
                    ui.selectable_value(
                        &mut settings.background,
                        crate::framing::Background::Blur,
                        "Blur video",
                    );
                    if ui
                        .selectable_label(
                            matches!(settings.background, crate::framing::Background::Color(_)),
                            "Color",
                        )
                        .clicked()
                    {
                        let color: Color32 = self.canvas_ui.color.into();
                        settings.background =
                            crate::framing::Background::Color([color.r(), color.g(), color.b()]);
                    }
                });
                ui.separator();
                if egui::color_picker::color_picker_hsva_2d(
                    ui,
                    &mut self.canvas_ui.color,
                    egui::color_picker::Alpha::Opaque,
                ) {
                    let color: Color32 = self.canvas_ui.color.into();
                    settings.background =
                        crate::framing::Background::Color([color.r(), color.g(), color.b()]);
                }
                done = ui.button("Done").clicked();
            });
            if !open || done || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.canvas_ui.background_open = false;
            }
        }
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
                if ui.button("Reset frame · fill & center").clicked() {
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
        if bg.clicked() && !clicked {
            selected = None;
        }
        if clicked || bg.clicked() || start_drag.is_some() {
            if let Some(p) = &mut self.project {
                p.selected_clips = selected.into_iter().collect();
            }
        }
        if let Some(drag) = start_drag {
            self.canvas_ui.stop_wheel();
            self.push_undo();
            self.live.pause();
            self.canvas_drag = Some(drag);
        }
        if let (Some(drag), Some(pointer)) = (&self.canvas_drag, drag_pointer) {
            let mut transform = drag.origin;
            if let Some((sx, sy)) = drag.corner {
                let anchor = drag.rect.center()
                    - egui::vec2(sx * drag.rect.width() * 0.5, sy * drag.rect.height() * 0.5);
                let relative = egui::vec2((pointer.x - anchor.x) * sx, (pointer.y - anchor.y) * sy);
                let diagonal = drag.rect.size();
                let factor = (relative.dot(diagonal) / diagonal.length_sq().max(1.0)).max(0.02);
                transform.scale = (drag.origin.scale * factor).clamp(0.03, 8.0);
                let size = drag.rect.size() * (transform.scale / drag.origin.scale);
                let center = anchor + egui::vec2(sx * size.x * 0.5, sy * size.y * 0.5);
                transform.x = (center.x - canvas.left()) / canvas.width();
                transform.y = (center.y - canvas.top()) / canvas.height();
            } else {
                let delta = pointer - drag.pointer;
                transform.x = drag.origin.x + delta.x / canvas.width();
                transform.y = drag.origin.y + delta.y / canvas.height();
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
            self.canvas_ui.stop_wheel();
            self.push_undo();
            if let Some(clip) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|c| c.id == id))
            {
                clip.transform = Transform {
                    fit: crate::framing::Fit::Fill,
                    opacity: clip.transform.opacity,
                    ..Default::default()
                };
            }
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
                frame.transform = c.transform;
            }
        }
        self.live.paint(ui, canvas, settings.background);
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
