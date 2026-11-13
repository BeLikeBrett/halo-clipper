use super::*;
use crate::track::Transform;

pub(super) fn layer_rect(canvas: egui::Rect, width: u32, height: u32, t: Transform) -> egui::Rect {
    let fit = (canvas.width() / width.max(1) as f32).min(canvas.height() / height.max(1) as f32);
    let size = egui::vec2(width as f32 * fit * t.scale, height as f32 * fit * t.scale);
    egui::Rect::from_center_size(
        egui::pos2(
            canvas.left() + canvas.width() * t.x,
            canvas.top() + canvas.height() * t.y,
        ),
        size,
    )
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
        let mut canvas_override = p.canvas_override;
        let previous_override = canvas_override;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preview").small().color(theme::TEXT_MUTED));
            egui::ComboBox::from_id_salt("canvas_resolution")
                .selected_text(if canvas_override.is_none() {
                    format!("Auto · {} × {}", media.width, media.height)
                } else {
                    format!("{} × {}", media.width, media.height)
                })
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut canvas_override,
                        None,
                        "Auto · largest timeline video",
                    );
                    for (w, h) in [
                        (1280, 720),
                        (1920, 1080),
                        (2560, 1440),
                        (3840, 2160),
                        (1080, 1920),
                    ] {
                        ui.selectable_value(
                            &mut canvas_override,
                            Some((w, h)),
                            format!("{w} × {h}"),
                        );
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(self.inspector_open, "Inspector")
                    .on_hover_text("Audio, speed and captions")
                    .clicked()
                {
                    self.inspector_open = !self.inspector_open;
                }
            });
        });
        if canvas_override != previous_override {
            self.push_undo();
            if let Some(p) = &mut self.project {
                p.canvas_override = canvas_override;
            }
        }
        ui.add_space(8.0);
        let available = ui.available_rect_before_wrap();
        let aspect = media.width.max(1) as f32 / media.height.max(1) as f32;
        let w = available.width().min(available.height() * aspect).max(1.0);
        let h = w / aspect;
        let canvas = egui::Rect::from_center_size(available.center(), egui::vec2(w, h));
        ui.painter().rect_filled(canvas, 2.0, Color32::BLACK);
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
        if let Some(p) = &mut self.project {
            p.selected_clips = selected.into_iter().collect();
        }
        if let Some(drag) = start_drag {
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
        for frame in &mut self.live.frames {
            if let Some(c) = self
                .project
                .as_ref()
                .and_then(|p| track::clip_by_id(&p.clips, frame.id))
            {
                frame.transform = c.transform;
            }
        }
        self.live.paint(ui, canvas);
        if let Some(layer) = self
            .live
            .frames
            .iter()
            .find(|layer| Some(layer.id) == selected)
        {
            let rect = layer_rect(canvas, layer.width, layer.height, layer.transform);
            let painter = ui.painter().with_clip_rect(canvas.expand(5.0));
            painter.rect_stroke(rect, 1.0, Stroke::new(1.5_f32, theme::ACCENT));
            for point in [
                rect.left_top(),
                rect.right_top(),
                rect.left_bottom(),
                rect.right_bottom(),
            ] {
                painter.rect_filled(
                    egui::Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                    1.0,
                    theme::TEXT_PRIMARY,
                );
                painter.rect_stroke(
                    egui::Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                    1.0,
                    Stroke::new(1.0_f32, theme::ACCENT),
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
