//! Text layers in the editor: drawing them on the preview canvas, direct
//! manipulation (drag to move, corners or scroll to resize, snapping), and
//! the Text inspector.
//!
//! Preview pixels come from `textrender`, the same renderer the export
//! uses, cached as one texture per layer and re-rendered only when the
//! layer's look, size or animation state changes.

use super::*;
use crate::text::{self, Align, Intro, Look, Outro, Shadow, TextFont, STYLE_PRESETS};
use crate::{picker, widgets};
use canvas::{snap_move, SnapTargets};

/// Text colors offered as one-click swatches.
const SWATCHES: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 214, 10],
    [255, 92, 92],
    [98, 220, 130],
    [120, 232, 255],
    [255, 120, 210],
    [170, 140, 255],
    [16, 16, 16],
];

/// Outline widths behind the Off / Thin / Bold choice.
const OUTLINES: [f32; 3] = [0.0, 0.05, 0.085];

#[derive(Clone, Copy)]
struct TextDrag {
    id: TextId,
    origin: (f32, f32, f32),
    pointer: egui::Pos2,
    rect: egui::Rect,
    /// Resize from a corner (scales about the center) instead of moving.
    resize: bool,
}

#[derive(Default)]
pub(super) struct TextUi {
    textures: HashMap<TextId, (u64, egui::TextureHandle)>,
    drag: Option<TextDrag>,
    /// An inspector edit is in progress (pointer held / field focused), so
    /// further changes join the same undo step.
    editing: bool,
    inspector_id: Option<TextId>,
    focus_field: bool,
    picker: Option<picker::PickerState>,
    wheel: Option<(TextId, f64)>,
    font_menu_open: bool,
    pub(super) band_drag: Option<BandDrag>,
}

/// Screen-space center of a layer on `canvas`.
fn center(item: &TextItem, canvas: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        canvas.left() + item.x * canvas.width(),
        canvas.top() + item.y * canvas.height(),
    )
}

/// Screen rect of the visible block (glyphs + box), for hit-testing and
/// the selection frame. `ppp` converts canvas points to pixels.
fn block_rect(item: &TextItem, canvas: egui::Rect, ppp: f32) -> egui::Rect {
    let (w, h) = crate::textrender::block_size(item, canvas.width() * ppp, canvas.height() * ppp);
    egui::Rect::from_center_size(
        center(item, canvas),
        egui::vec2((w / ppp).max(24.0), (h / ppp).max(20.0)),
    )
}

fn render_key(item: &TextItem, look: &Look, w: f32, h: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.render_key().hash(&mut hasher);
    (w.to_bits(), h.to_bits()).hash(&mut hasher);
    format!("{look:?}").hash(&mut hasher);
    hasher.finish()
}

impl ClipperApp {
    /// Adds a text layer at the playhead (one undo step), selects it and
    /// puts the cursor in its text field.
    pub(super) fn add_text_layer(&mut self) {
        if self.project.is_none() {
            return;
        }
        self.live.pause();
        self.text_ui.editing = false;
        self.push_undo();
        if let Some(p) = &mut self.project {
            p.add_text();
        }
        self.text_ui.focus_field = true;
    }

    pub(super) fn text_dragging(&self) -> bool {
        self.text_ui.drag.is_some()
    }

    /// Layers shown on the canvas right now: those under the playhead, plus
    /// the selected one (ghosted when outside its time range, so it can
    /// still be placed).
    fn canvas_texts(&self) -> Vec<(TextItem, Look, bool)> {
        let Some(p) = &self.project else {
            return Vec::new();
        };
        let playing = self.live.playing;
        p.texts
            .iter()
            .filter_map(|t| {
                let selected = p.selected_text == Some(t.id);
                if t.contains(p.playhead_ms) {
                    // While paused the selected layer shows settled, so it
                    // is never edited on a half-faded or half-typed frame.
                    let look = if selected && !playing {
                        Look::STILL
                    } else {
                        text::look_at(t, p.playhead_ms)
                    };
                    Some((t.clone(), look, false))
                } else if selected && !playing {
                    Some((t.clone(), Look::STILL, true))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Hit-testing and drags for text layers. Call after the video layers
    /// registered their interactions so text wins overlapping hits.
    /// Returns true when a text layer took the click.
    pub(super) fn interact_canvas_texts(
        &mut self,
        ui: &mut egui::Ui,
        canvas: egui::Rect,
        video_rects: &[egui::Rect],
    ) -> bool {
        let ppp = ui.ctx().pixels_per_point();
        let selected = self.project.as_ref().and_then(|p| p.selected_text);
        let mut took = false;
        let mut start: Option<TextDrag> = None;
        let mut pointer = None;
        let mut stopped = false;
        for (item, _, _) in self.canvas_texts() {
            let rect = block_rect(&item, canvas, ppp);
            let response = ui
                .interact(
                    rect.expand(4.0),
                    ui.id().with(("text_layer", item.id)),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::Move);
            if response.clicked() || response.drag_started() {
                took = true;
                if let Some(p) = &mut self.project {
                    p.select_text(Some(item.id));
                }
            }
            if response.double_clicked() {
                self.text_ui.focus_field = true;
            }
            if response.drag_started() {
                start = Some(TextDrag {
                    id: item.id,
                    origin: (item.x, item.y, item.size),
                    pointer: ui
                        .input(|i| i.pointer.press_origin())
                        .or(response.interact_pointer_pos())
                        .unwrap_or(rect.center()),
                    rect,
                    resize: false,
                });
            }
            if response.dragged() {
                pointer = response.interact_pointer_pos();
            }
            stopped |= response.drag_stopped();

            if selected == Some(item.id) {
                for (index, corner) in [
                    rect.left_top(),
                    rect.right_top(),
                    rect.left_bottom(),
                    rect.right_bottom(),
                ]
                .into_iter()
                .enumerate()
                {
                    let handle = ui
                        .interact(
                            egui::Rect::from_center_size(corner, egui::vec2(18.0, 18.0)),
                            ui.id().with(("text_corner", item.id, index)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(
                            if (corner.x < rect.center().x) == (corner.y < rect.center().y) {
                                egui::CursorIcon::ResizeNwSe
                            } else {
                                egui::CursorIcon::ResizeNeSw
                            },
                        );
                    if handle.drag_started() {
                        took = true;
                        start = Some(TextDrag {
                            id: item.id,
                            origin: (item.x, item.y, item.size),
                            pointer: ui
                                .input(|i| i.pointer.press_origin())
                                .or(handle.interact_pointer_pos())
                                .unwrap_or(corner),
                            rect,
                            resize: true,
                        });
                    }
                    if handle.dragged() {
                        pointer = handle.interact_pointer_pos();
                    }
                    stopped |= handle.drag_stopped();
                }

                // Scroll over the selected layer resizes it.
                if response.hovered() {
                    let delta = ui.input(|i| i.smooth_scroll_delta.y);
                    if delta.abs() > 0.01 {
                        ui.input_mut(|i| {
                            i.smooth_scroll_delta.y = 0.0;
                            i.raw_scroll_delta.y = 0.0;
                        });
                        let now = ui.input(|i| i.time);
                        let same_gesture = self
                            .text_ui
                            .wheel
                            .is_some_and(|(id, at)| id == item.id && now - at < 0.35);
                        if !same_gesture {
                            self.push_undo();
                        }
                        self.text_ui.wheel = Some((item.id, now));
                        if let Some(t) = self.project.as_mut().and_then(|p| p.text_mut(item.id)) {
                            t.size = (t.size * (delta * 0.0025).exp())
                                .clamp(text::MIN_SIZE, text::MAX_SIZE);
                        }
                        ui.ctx().request_repaint();
                    }
                }
            }
        }

        if let Some(drag) = start {
            self.live.pause();
            self.push_undo();
            self.text_ui.drag = Some(drag);
        }
        if let (Some(drag), Some(pointer)) = (self.text_ui.drag, pointer) {
            let snapping = !ui.input(|i| i.modifiers.alt);
            self.canvas_ui.guides.clear();
            let (x0, y0, size0) = drag.origin;
            let mut update = (x0, y0, size0);
            if drag.resize {
                let c = drag.rect.center();
                let from = (drag.pointer - c).length().max(4.0);
                let factor = (pointer - c).length() / from;
                update.2 = (size0 * factor).clamp(text::MIN_SIZE, text::MAX_SIZE);
            } else {
                let mut moved = drag.rect.translate(pointer - drag.pointer);
                if snapping {
                    let targets = SnapTargets::new(canvas, video_rects.iter().copied());
                    let (snapped, guides) = snap_move(moved, &targets);
                    moved = snapped;
                    self.canvas_ui.guides = guides;
                }
                update.0 = ((moved.center().x - canvas.left()) / canvas.width()).clamp(0.0, 1.0);
                update.1 = ((moved.center().y - canvas.top()) / canvas.height()).clamp(0.0, 1.0);
            }
            if let Some(t) = self.project.as_mut().and_then(|p| p.text_mut(drag.id)) {
                (t.x, t.y, t.size) = update;
            }
            ui.ctx().request_repaint();
        }
        if stopped {
            self.text_ui.drag = None;
            self.canvas_ui.guides.clear();
        }
        took || self.text_ui.drag.is_some()
    }

    /// Paints text layers over the composited video, plus the selection
    /// frame of the selected layer.
    pub(super) fn paint_canvas_texts(&mut self, ui: &egui::Ui, canvas: egui::Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = (canvas.width() * ppp, canvas.height() * ppp);
        let selected = self.project.as_ref().and_then(|p| p.selected_text);
        let painter = ui.painter().with_clip_rect(canvas);
        let shown = self.canvas_texts();
        let live: HashSet<TextId> = shown.iter().map(|(t, _, _)| t.id).collect();
        self.text_ui.textures.retain(|id, _| live.contains(id));

        for (item, look, ghost) in &shown {
            let key = render_key(item, look, w, h);
            let fresh = self
                .text_ui
                .textures
                .get(&item.id)
                .is_some_and(|(k, _)| *k == key);
            if !fresh {
                let Some(rendered) = crate::textrender::render(item, look, w, h) else {
                    continue;
                };
                let p = &rendered.pixmap;
                let image = egui::ColorImage::from_rgba_premultiplied(
                    [p.width() as usize, p.height() as usize],
                    p.data(),
                );
                let texture = ui.ctx().load_texture(
                    format!("text-layer-{}", item.id.0),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.text_ui.textures.insert(item.id, (key, texture));
            }
            let Some((_, texture)) = self.text_ui.textures.get(&item.id) else {
                continue;
            };
            let size = texture.size_vec2() / ppp;
            let rect = egui::Rect::from_center_size(center(item, canvas), size);
            // Placeholder text and out-of-range selections are previews only.
            let tint = if *ghost || item.text.trim().is_empty() {
                Color32::from_white_alpha(110)
            } else {
                Color32::WHITE
            };
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }

        if let Some((item, _, _)) = shown.iter().find(|(t, _, _)| Some(t.id) == selected) {
            let rect = block_rect(item, canvas, ppp);
            let frame = ui.painter().with_clip_rect(canvas.expand(6.0));
            frame.rect_stroke(rect, 2.0, Stroke::new(1.5_f32, theme::ACCENT));
            for corner in [
                rect.left_top(),
                rect.right_top(),
                rect.left_bottom(),
                rect.right_bottom(),
            ] {
                frame.circle_filled(corner, 4.5, theme::TEXT_PRIMARY);
                frame.circle_stroke(corner, 4.5, Stroke::new(1.0_f32, theme::ACCENT));
            }
        }
    }

    /// The Edit panel for a selected text layer.
    pub(super) fn show_text_inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.project.as_ref().and_then(|p| p.selected_text) else {
            return;
        };
        let Some(before) = self.project.as_ref().and_then(|p| p.text(id)).cloned() else {
            return;
        };
        if self.text_ui.inspector_id != Some(id) {
            self.text_ui.inspector_id = Some(id);
            self.text_ui.editing = false;
            self.text_ui.picker = None;
            self.text_ui.font_menu_open = false;
        }
        let mut item = before.clone();
        let mut close = false;

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Text")
                    .font(theme::semibold(17.0))
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("✕").size(14.0).color(theme::TEXT_MUTED))
                            .frame(false),
                    )
                    .on_hover_text("Deselect")
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.add_space(8.0);

        // Content.
        let field_id = ui.id().with(("text_field", id));
        let field = ui.add(
            egui::TextEdit::multiline(&mut item.text)
                .id(field_id)
                .hint_text("Type your text…")
                .font(theme::medium(14.0))
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.text_ui.focus_field) {
            field.request_focus();
        }
        ui.add_space(14.0);

        // Style presets.
        section(ui, "Style");
        let current = item.style.preset_index();
        chip_grid(ui, 3, STYLE_PRESETS.len(), |ui, i, width| {
            let (name, preset) = &STYLE_PRESETS[i];
            let response = style_chip(ui, name, preset, current == Some(i), width);
            if response.clicked() {
                item.style = item.style.with_preset(preset);
            }
        });
        ui.add_space(12.0);

        // Font, shown in its own face.
        section(ui, "Font");
        let font_face = egui::FontId::new(
            15.0,
            egui::FontFamily::Name(item.style.font.egui_family().into()),
        );
        let mut chosen_font = None;
        widgets::dropdown_button(
            ui,
            "text_font",
            &mut self.text_ui.font_menu_open,
            egui::vec2(ui.available_width(), 32.0),
            ui.available_width(),
            |painter, rect, color| {
                painter.text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    item.style.font.label(),
                    font_face.clone(),
                    color,
                );
            },
            |ui| {
                let mut picked = false;
                for font in TextFont::ALL {
                    if font_row(ui, font, font == item.style.font).clicked() {
                        chosen_font = Some(font);
                        picked = true;
                    }
                }
                picked
            },
        );
        if let Some(font) = chosen_font {
            item.style.font = font;
        }
        ui.add_space(12.0);

        // Color.
        section(ui, "Color");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 4.0);
            for rgb in SWATCHES {
                if swatch(ui, rgb, item.style.color == rgb).clicked() {
                    item.style.color = rgb;
                    self.text_ui.picker = None;
                }
            }
            let custom = !SWATCHES.contains(&item.style.color);
            if custom_swatch(
                ui,
                item.style.color,
                custom || self.text_ui.picker.is_some(),
            )
            .clicked()
            {
                self.text_ui.picker = match self.text_ui.picker {
                    Some(_) => None,
                    None => Some(picker::PickerState::new(item.style.color)),
                };
            }
        });
        if let Some(state) = &mut self.text_ui.picker {
            if state.rgb() != item.style.color {
                state.set_rgb(item.style.color);
            }
            ui.add_space(8.0);
            if picker::show(ui, state) {
                item.style.color = state.rgb();
            }
        }
        ui.add_space(12.0);

        egui::CollapsingHeader::new("Color mix")
            .id_salt("text_color_mix")
            .show(ui, |ui| {
                color_mix(ui, "Text", &mut item.style.color, &mut item.style.gradient);
                if item.style.outline > 0.0 {
                    color_mix(
                        ui,
                        "Outline",
                        &mut item.style.outline_color,
                        &mut item.style.outline_gradient,
                    );
                }
                if let Some((rgb, opacity)) = &mut item.style.background {
                    color_mix(ui, "Box", rgb, &mut item.style.box_gradient);
                    ui.add(egui::Slider::new(opacity, 0.0..=1.0).text("Box opacity"));
                }
            });
        ui.add_space(8.0);

        // Fine look controls.
        let row = |ui: &mut egui::Ui, label: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(56.0, 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(56.0);
                        ui.label(
                            RichText::new(label)
                                .font(theme::regular(12.5))
                                .color(theme::TEXT_MUTED),
                        );
                    },
                );
                add(ui);
            });
            ui.add_space(4.0);
        };
        row(ui, "Align", &mut |ui| {
            let index = match item.style.align {
                Align::Left => 0,
                Align::Center => 1,
                Align::Right => 2,
            };
            if let Some(i) = widgets::segmented(
                ui,
                "text_align",
                &[("Left", ""), ("Center", ""), ("Right", "")],
                index,
            ) {
                item.style.align = [Align::Left, Align::Center, Align::Right][i];
            }
        });
        row(ui, "Outline", &mut |ui| {
            let index = OUTLINES
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 - item.style.outline)
                        .abs()
                        .total_cmp(&(b.1 - item.style.outline).abs())
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            if let Some(i) = widgets::segmented(
                ui,
                "text_outline",
                &[("Off", ""), ("Thin", ""), ("Bold", "")],
                index,
            ) {
                item.style.outline = OUTLINES[i];
            }
        });
        row(ui, "Shadow", &mut |ui| {
            let index = match item.style.shadow {
                Shadow::None => 0,
                Shadow::Soft => 1,
                Shadow::Glow => 2,
            };
            if let Some(i) = widgets::segmented(
                ui,
                "text_shadow",
                &[
                    ("Off", ""),
                    ("Soft", "Drop shadow"),
                    ("Glow", "Neon halo in the text color"),
                ],
                index,
            ) {
                item.style.shadow = [Shadow::None, Shadow::Soft, Shadow::Glow][i];
            }
        });
        row(ui, "Box", &mut |ui| {
            let index = match item.style.background {
                None => 0,
                Some(([r, ..], _)) if r < 128 => 1,
                Some(_) => 2,
            };
            if let Some(i) = widgets::segmented(
                ui,
                "text_box",
                &[("Off", ""), ("Dark", ""), ("Light", "")],
                index,
            ) {
                item.style.background = match i {
                    0 => None,
                    1 => Some(([0, 0, 0], 0.72)),
                    _ => Some(([255, 255, 255], 1.0)),
                };
            }
        });
        ui.add_space(10.0);

        // Animation.
        section(ui, "Animate in");
        chip_grid(ui, 3, Intro::ALL.len(), |ui, i, width| {
            let intro = Intro::ALL[i];
            if chip(ui, intro.label(), item.intro == intro, width).clicked() {
                item.intro = intro;
            }
        });
        if item.intro != Intro::None {
            duration_control(ui, "In time", &mut item.intro_ms);
        }
        ui.add_space(10.0);
        section(ui, "Animate out");
        chip_grid(ui, 4, Outro::ALL.len(), |ui, i, width| {
            let outro = Outro::ALL[i];
            if chip(ui, outro.label(), item.outro == outro, width).clicked() {
                item.outro = outro;
            }
        });
        ui.add_space(10.0);
        if item.outro != Outro::None {
            duration_control(ui, "Out time", &mut item.outro_ms);
        }

        // Commit: one undo step per gesture (drag, typing burst, click).
        let busy = ui.input(|i| i.pointer.any_down()) || field.has_focus();
        if item != before {
            if !self.text_ui.editing {
                self.push_undo();
            }
            self.text_ui.editing = busy;
            if let Some(t) = self.project.as_mut().and_then(|p| p.text_mut(id)) {
                *t = item;
            }
        } else if !busy {
            self.text_ui.editing = false;
        }
        if close {
            if let Some(p) = &mut self.project {
                p.selected_text = None;
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .font(theme::medium(12.0))
            .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(6.0);
}

/// Lays `count` equal-width cells out `per_row` to a row.
fn chip_grid(
    ui: &mut egui::Ui,
    per_row: usize,
    count: usize,
    mut cell: impl FnMut(&mut egui::Ui, usize, f32),
) {
    let gap = 6.0;
    let width = ((ui.available_width() - gap * (per_row as f32 - 1.0)) / per_row as f32).floor();
    for start in (0..count).step_by(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for i in start..(start + per_row).min(count) {
                cell(ui, i, width);
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// Selectable pill with a centered label.
fn chip(ui: &mut egui::Ui, label: &str, active: bool, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), widgets::CLICK);
    let fill = if active {
        theme::BG_BUTTON_ACTIVE
    } else if response.hovered() {
        theme::BG_BUTTON_HOVER
    } else {
        theme::BG_BUTTON
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        6.0,
        Stroke::new(1.0_f32, if active { theme::ACCENT } else { theme::BORDER }),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        theme::medium(12.0),
        if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A style preset tile: a miniature "Aa" in the preset's colors.
fn style_chip(
    ui: &mut egui::Ui,
    name: &str,
    style: &text::TextStyle,
    active: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 52.0), widgets::CLICK);
    let painter = ui.painter();
    painter.rect_filled(rect, 7.0, Color32::from_rgb(44, 50, 62));
    let sample_center = rect.center() - egui::vec2(0.0, 7.0);
    let font = egui::FontId::new(
        17.0,
        egui::FontFamily::Name(TextFont::Montserrat.egui_family().into()),
    );
    let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    if let Some((bg, alpha)) = style.background {
        let galley = painter.layout_no_wrap("Aa".into(), font.clone(), Color32::WHITE);
        painter.rect_filled(
            egui::Rect::from_center_size(sample_center, galley.size() + egui::vec2(12.0, 2.0)),
            4.0,
            rgb(bg).gamma_multiply(alpha),
        );
    }
    match style.shadow {
        Shadow::Glow => {
            for r in [2.0, 1.0] {
                for (dx, dy) in [(r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)] {
                    painter.text(
                        sample_center + egui::vec2(dx, dy),
                        egui::Align2::CENTER_CENTER,
                        "Aa",
                        font.clone(),
                        rgb(style.color).gamma_multiply(0.35),
                    );
                }
            }
        }
        Shadow::Soft => {
            painter.text(
                sample_center + egui::vec2(0.0, 1.5),
                egui::Align2::CENTER_CENTER,
                "Aa",
                font.clone(),
                Color32::from_black_alpha(150),
            );
        }
        Shadow::None => {}
    }
    if style.outline > 0.0 {
        for (dx, dy) in [
            (-1.2, 0.0),
            (1.2, 0.0),
            (0.0, -1.2),
            (0.0, 1.2),
            (-0.9, -0.9),
            (0.9, 0.9),
            (-0.9, 0.9),
            (0.9, -0.9),
        ] {
            painter.text(
                sample_center + egui::vec2(dx, dy),
                egui::Align2::CENTER_CENTER,
                "Aa",
                font.clone(),
                rgb(style.outline_color),
            );
        }
    }
    painter.text(
        sample_center,
        egui::Align2::CENTER_CENTER,
        "Aa",
        font,
        rgb(style.color),
    );
    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        name,
        theme::regular(10.5),
        if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_MUTED
        },
    );
    let edge = if active {
        Stroke::new(1.5_f32, theme::ACCENT)
    } else if response.hovered() {
        Stroke::new(1.0_f32, theme::TEXT_FAINT)
    } else {
        Stroke::new(1.0_f32, theme::BORDER)
    };
    painter.rect_stroke(rect.shrink(0.5), 7.0, edge);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One font in the font menu, drawn in its own face.
fn font_row(ui: &mut egui::Ui, font: TextFont, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), widgets::CLICK);
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, theme::BG_BUTTON_HOVER);
    }
    if selected {
        ui.painter().circle_filled(
            rect.left_center() + egui::vec2(12.0, 0.0),
            3.0,
            theme::ACCENT,
        );
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(24.0, 0.0),
        egui::Align2::LEFT_CENTER,
        font.label(),
        egui::FontId::new(17.0, egui::FontFamily::Name(font.egui_family().into())),
        if selected {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn swatch(ui: &mut egui::Ui, rgb: [u8; 3], selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(25.0, 25.0), widgets::CLICK);
    let c = rect.center();
    ui.painter()
        .circle_filled(c, 9.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    ui.painter()
        .circle_stroke(c, 9.0, Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    if selected {
        ui.painter()
            .circle_stroke(c, 11.5, Stroke::new(1.5_f32, theme::ACCENT));
    } else if response.hovered() {
        ui.painter()
            .circle_stroke(c, 11.5, Stroke::new(1.0_f32, theme::TEXT_MUTED));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The "any color" swatch: a hue ring that opens the full picker.
fn custom_swatch(ui: &mut egui::Ui, current: [u8; 3], active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(25.0, 25.0), widgets::CLICK);
    let c = rect.center();
    let steps = 24;
    for i in 0..steps {
        let a0 = i as f32 / steps as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
        let color: Color32 =
            egui::ecolor::Hsva::new(i as f32 / steps as f32, 0.75, 1.0, 1.0).into();
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                c,
                c + 9.0 * egui::vec2(a0.cos(), a0.sin()),
                c + 9.0 * egui::vec2(a1.cos(), a1.sin()),
            ],
            color,
            Stroke::NONE,
        ));
    }
    ui.painter().circle_filled(
        c,
        4.5,
        Color32::from_rgb(current[0], current[1], current[2]),
    );
    if active {
        ui.painter()
            .circle_stroke(c, 11.5, Stroke::new(1.5_f32, theme::ACCENT));
    }
    response
        .on_hover_text("Custom color")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ── Timeline band ──────────────────────────────────────────────────────

/// Height of one row of text bars in the timeline band.
pub(super) const BAND_ROW: f32 = 26.0;
const BAND_MAX_ROWS: usize = 4;
/// Grab zone at each end of a bar for trimming, in points.
const TRIM_GRAB: f32 = 7.0;
/// Snap distance for bar edges, in points.
const BAND_SNAP: f32 = 8.0;

/// Packs layers into rows so overlapping ones stack: each takes the first
/// row that is free at its start (or, past the row cap, the row that frees
/// up soonest). Returns the row of each layer and the row count (≥ 1).
pub(super) fn band_rows(texts: &[TextItem]) -> (HashMap<TextId, usize>, usize) {
    let mut order: Vec<&TextItem> = texts.iter().collect();
    order.sort_by_key(|t| (t.start_ms, t.id));
    let mut ends: Vec<i64> = Vec::new();
    let mut rows = HashMap::new();
    for t in order {
        let row = match ends.iter().position(|end| *end <= t.start_ms) {
            Some(row) => row,
            None if ends.len() < BAND_MAX_ROWS => {
                ends.push(i64::MIN);
                ends.len() - 1
            }
            None => (0..ends.len()).min_by_key(|r| ends[*r]).unwrap_or(0),
        };
        ends[row] = ends[row].max(t.end_ms);
        rows.insert(t.id, row);
    }
    (rows, ends.len().max(1))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Grab {
    Move,
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BandDrag {
    id: TextId,
    grab: Grab,
    origin: (i64, i64),
    press_x: f32,
}

/// What the user did in the band this frame; applied by the caller once the
/// project is no longer borrowed.
#[derive(Debug, PartialEq)]
pub(super) enum BandAction {
    /// New layer, at this time or the playhead.
    Add(Option<i64>),
    Select(TextId),
    Retime {
        id: TextId,
        start: i64,
        end: i64,
        /// First frame of the gesture: take an undo snapshot.
        begin: bool,
    },
    Duplicate(TextId),
    Delete(TextId),
    Seek(i64),
}

/// Moves `value` onto the nearest of `snaps` within `reach` ms.
fn snap_time(value: i64, snaps: &[i64], reach: i64) -> Option<i64> {
    snaps
        .iter()
        .copied()
        .filter(|s| (s - value).abs() <= reach)
        .min_by_key(|s| (s - value).abs())
}

/// New (start, end) for a bar drag of `delta` ms, snapped to `snaps`.
fn retime(
    grab: Grab,
    (start, end): (i64, i64),
    delta: i64,
    snaps: &[i64],
    reach: i64,
) -> (i64, i64) {
    let min = text::MIN_DURATION_MS;
    match grab {
        Grab::Move => {
            let len = end - start;
            let mut s = (start + delta).max(0);
            let by_start = snap_time(s, snaps, reach).map(|t| t - s);
            let by_end = snap_time(s + len, snaps, reach).map(|t| t - (s + len));
            let shift = match (by_start, by_end) {
                (Some(a), Some(b)) => Some(if a.abs() <= b.abs() { a } else { b }),
                (a, b) => a.or(b),
            };
            s = (s + shift.unwrap_or(0)).max(0);
            (s, s + len)
        }
        Grab::Start => {
            let s = (start + delta).max(0);
            let s = snap_time(s, snaps, reach).unwrap_or(s);
            (s.clamp(0, end - min), end)
        }
        Grab::End => {
            let e = end + delta;
            let e = snap_time(e, snaps, reach).unwrap_or(e);
            (start, e.max(start + min))
        }
    }
}

/// Draws the text band (header cell + bars) into `rect` and handles its
/// input. `snaps` are times bar edges stick to (playhead, clip edges).
#[allow(clippy::too_many_arguments)]
pub(super) fn show_text_band(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    lanes: editor::Lanes,
    texts: &[TextItem],
    selected: Option<TextId>,
    playhead: i64,
    snaps: &[i64],
    drag: &mut Option<BandDrag>,
) -> Vec<BandAction> {
    let mut actions = Vec::new();
    let painter = ui.painter_at(rect);
    let header = egui::Rect::from_min_max(rect.min, egui::pos2(lanes.left, rect.bottom()));
    let area = egui::Rect::from_min_max(egui::pos2(lanes.left, rect.top()), rect.max);
    let bars = painter.with_clip_rect(area);
    painter.rect_filled(header, 0.0, Color32::from_rgb(23, 24, 29));
    painter.rect_filled(area, 0.0, theme::BG_HEADER);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER),
    );
    painter.line_segment(
        [header.right_top(), header.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER),
    );
    painter.text(
        egui::pos2(header.left() + 14.0, header.top() + BAND_ROW / 2.0),
        Align2::LEFT_CENTER,
        "Text",
        theme::medium(12.0),
        theme::TEXT_SECONDARY,
    );
    let plus = egui::Rect::from_center_size(
        egui::pos2(header.right() - 20.0, header.top() + BAND_ROW / 2.0),
        egui::vec2(22.0, 20.0),
    );
    let add = ui
        .interact(plus, ui.id().with("text_band_add"), widgets::CLICK)
        .on_hover_text("Add text at the playhead (T)")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if add.hovered() {
        painter.rect_filled(plus, 4.0, theme::BG_BUTTON_HOVER);
    }
    painter.text(
        plus.center(),
        Align2::CENTER_CENTER,
        "+",
        theme::medium(15.0),
        if add.hovered() {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_MUTED
        },
    );
    if add.clicked() {
        actions.push(BandAction::Add(None));
    }

    // Empty band: click seeks, double-click adds a layer there.
    let background = ui
        .interact(area, ui.id().with("text_band_bg"), egui::Sense::click())
        .on_hover_text_at_pointer("Double-click to add text here");
    if let Some(pos) = background.interact_pointer_pos() {
        if background.double_clicked() {
            actions.push(BandAction::Add(Some(lanes.time_at(pos.x))));
        } else if background.clicked() {
            actions.push(BandAction::Seek(lanes.time_at(pos.x)));
        }
    }
    if texts.is_empty() {
        bars.text(
            egui::pos2(area.left() + 10.0, area.top() + BAND_ROW / 2.0),
            Align2::LEFT_CENTER,
            "Double-click or press T to add text",
            theme::regular(11.5),
            theme::TEXT_FAINT,
        );
    }

    let (rows, _) = band_rows(texts);
    let reach = (BAND_SNAP / lanes.width * lanes.span as f32).round() as i64;
    for item in texts {
        let row = rows.get(&item.id).copied().unwrap_or(0) as f32;
        let x0 = lanes.x_of(item.start_ms);
        let x1 = lanes.x_of(item.end_ms).max(x0 + 4.0);
        let bar = egui::Rect::from_min_max(
            egui::pos2(x0, rect.top() + row * BAND_ROW + 3.0),
            egui::pos2(x1, rect.top() + (row + 1.0) * BAND_ROW - 2.0),
        );
        let response = ui.interact(
            bar.intersect(area),
            ui.id().with(("text_bar", item.id)),
            egui::Sense::click_and_drag(),
        );
        let grab_at = |x: f32| {
            if bar.width() > TRIM_GRAB * 3.0 && x - bar.left() < TRIM_GRAB {
                Grab::Start
            } else if bar.width() > TRIM_GRAB * 3.0 && bar.right() - x < TRIM_GRAB {
                Grab::End
            } else {
                Grab::Move
            }
        };
        let hover_grab = response.hover_pos().map(|p| grab_at(p.x));
        let active = drag.filter(|d| d.id == item.id);
        match active.map(|d| d.grab).or(hover_grab) {
            Some(Grab::Start | Grab::End) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal)
            }
            Some(Grab::Move) if active.is_some() => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing)
            }
            Some(Grab::Move) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            None => {}
        }
        if response.clicked() {
            actions.push(BandAction::Select(item.id));
        }
        if response.drag_started() {
            let press_x = ui
                .input(|i| i.pointer.press_origin())
                .map_or(bar.center().x, |p| p.x);
            *drag = Some(BandDrag {
                id: item.id,
                grab: grab_at(press_x),
                origin: (item.start_ms, item.end_ms),
                press_x,
            });
            actions.push(BandAction::Select(item.id));
        }
        if let (Some(d), Some(pos)) = (
            drag.filter(|d| d.id == item.id),
            response.interact_pointer_pos(),
        ) {
            if response.dragged() {
                let delta = ((pos.x - d.press_x) / lanes.width * lanes.span as f32).round() as i64;
                let mut edges: Vec<i64> = snaps.to_vec();
                edges.extend(
                    texts
                        .iter()
                        .filter(|t| t.id != item.id)
                        .flat_map(|t| [t.start_ms, t.end_ms]),
                );
                let reach = if ui.input(|i| i.modifiers.alt) {
                    0
                } else {
                    reach
                };
                let (start, end) = retime(d.grab, d.origin, delta, &edges, reach);
                if (start, end) != (item.start_ms, item.end_ms) || response.drag_started() {
                    actions.push(BandAction::Retime {
                        id: item.id,
                        start,
                        end,
                        begin: response.drag_started(),
                    });
                }
            }
        }
        if response.drag_stopped() {
            *drag = None;
        }
        response.context_menu(|ui| {
            if ui.button("Duplicate").clicked() {
                actions.push(BandAction::Duplicate(item.id));
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                actions.push(BandAction::Delete(item.id));
                ui.close_menu();
            }
        });

        let is_selected = selected == Some(item.id);
        let fill = if is_selected {
            theme::CLIP_TEXT_EDGE.gamma_multiply(0.75)
        } else if response.hovered() {
            theme::CLIP_TEXT.gamma_multiply(1.25)
        } else {
            theme::CLIP_TEXT
        };
        bars.rect_filled(bar, 4.0, fill);
        bars.rect_stroke(
            bar.shrink(0.5),
            4.0,
            if is_selected {
                Stroke::new(1.5_f32, theme::ACCENT)
            } else {
                Stroke::new(1.0_f32, theme::CLIP_TEXT_EDGE)
            },
        );
        let first_line = item.text.lines().next().unwrap_or("").trim();
        let (label, color) = if first_line.is_empty() {
            ("Text", theme::TEXT_MUTED)
        } else {
            (first_line, theme::TEXT_PRIMARY)
        };
        bars.with_clip_rect(bar.shrink2(egui::vec2(6.0, 0.0)).intersect(area))
            .text(
                egui::pos2(bar.left() + 8.0, bar.center().y),
                Align2::LEFT_CENTER,
                label,
                theme::medium(11.5),
                color,
            );
    }

    let x = lanes.x_of(playhead);
    if (area.left()..=area.right()).contains(&x) {
        bars.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(1.5_f32, theme::PLAYHEAD),
        );
    }
    actions
}

impl ClipperApp {
    /// Applies what happened in the timeline text band.
    pub(super) fn apply_band_actions(&mut self, actions: Vec<BandAction>) {
        for action in actions {
            match action {
                BandAction::Add(at) => {
                    if let Some(ms) = at {
                        self.seek_to(ms);
                    }
                    self.add_text_layer();
                }
                BandAction::Select(id) => {
                    let Some(p) = &mut self.project else { continue };
                    p.select_text(Some(id));
                    // Park the playhead where the whole text shows, unless
                    // it's already inside the layer.
                    let park = p
                        .text(id)
                        .filter(|t| !t.contains(p.playhead_ms))
                        .map(text::settled_at);
                    if let Some(ms) = park {
                        self.seek_to(ms);
                    }
                }
                BandAction::Retime {
                    id,
                    start,
                    end,
                    begin,
                } => {
                    if begin {
                        self.push_undo();
                    }
                    if let Some(t) = self.project.as_mut().and_then(|p| p.text_mut(id)) {
                        (t.start_ms, t.end_ms) = (start, end);
                    }
                }
                BandAction::Duplicate(id) => {
                    let Some(source) = self.project.as_ref().and_then(|p| p.text(id)).cloned()
                    else {
                        continue;
                    };
                    self.push_undo();
                    if let Some(p) = &mut self.project {
                        let mut copy = source;
                        let len = copy.duration_ms();
                        copy.id = p.id_gen.next_text();
                        copy.start_ms = copy.end_ms;
                        copy.end_ms = copy.start_ms + len;
                        let new_id = copy.id;
                        p.texts.push(copy);
                        p.select_text(Some(new_id));
                    }
                }
                BandAction::Delete(id) => {
                    self.push_undo();
                    if let Some(p) = &mut self.project {
                        p.texts.retain(|t| t.id != id);
                        if p.selected_text == Some(id) {
                            p.selected_text = None;
                        }
                    }
                }
                BandAction::Seek(ms) => {
                    if let Some(p) = &mut self.project {
                        p.selected_text = None;
                    }
                    self.seek_to(ms);
                }
            }
        }
    }

    pub(super) fn seek_to(&mut self, ms: i64) {
        if let Some(p) = &mut self.project {
            p.playhead_ms = ms.max(0);
        }
        self.live.reanchor(ms.max(0));
    }
}

/// Shared compact timing control for text and clip animations.
pub(super) fn duration_control(ui: &mut egui::Ui, label: &str, ms: &mut f32) {
    let mut seconds = *ms / 1000.0;
    if ui
        .add(
            egui::Slider::new(&mut seconds, 0.05..=5.0)
                .logarithmic(true)
                .fixed_decimals(2)
                .suffix(" s")
                .text(label),
        )
        .on_hover_text("Shorter = faster. Short clips automatically shorten the animation to fit.")
        .changed()
    {
        *ms = seconds * 1000.0;
    }
}

fn color_mix(
    ui: &mut egui::Ui,
    label: &str,
    rgb: &mut [u8; 3],
    gradient: &mut Option<text::Gradient>,
) {
    ui.push_id(label, |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.color_edit_button_srgb(rgb)
                .on_hover_text(format!("{label} color"));
            let mut enabled = gradient.is_some();
            if ui.checkbox(&mut enabled, "Gradient").changed() {
                *gradient = enabled.then_some(text::Gradient {
                    end: [255, 120, 210],
                    angle: 90.0,
                });
            }
            if let Some(g) = gradient {
                ui.color_edit_button_srgb(&mut g.end)
                    .on_hover_text("End color");
            }
        });
        if let Some(g) = gradient {
            ui.horizontal(|ui| {
                for (name, a, b) in [
                    ("Sunset", [255, 191, 65], [255, 62, 137]),
                    ("Ice", [85, 241, 255], [100, 99, 255]),
                    ("Candy", [255, 132, 229], [122, 255, 208]),
                ] {
                    if ui.small_button(name).clicked() {
                        *rgb = a;
                        g.end = b;
                    }
                }
            });
            ui.add(
                egui::Slider::new(&mut g.angle, 0.0..=360.0)
                    .suffix("°")
                    .text("Direction"),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: u32, start: i64, end: i64) -> TextItem {
        TextItem::new(TextId(id), start, end)
    }

    #[test]
    fn overlapping_layers_stack_and_free_rows_are_reused() {
        let texts = [
            layer(1, 0, 3_000),
            layer(2, 1_000, 2_000),
            layer(3, 3_000, 4_000),
        ];
        let (rows, count) = band_rows(&texts);
        assert_eq!(count, 2);
        assert_eq!(rows[&TextId(1)], 0);
        assert_eq!(rows[&TextId(2)], 1);
        // Starts exactly when the first ends: back on the top row.
        assert_eq!(rows[&TextId(3)], 0);
        assert_eq!(band_rows(&[]).1, 1);
    }

    #[test]
    fn row_count_is_capped() {
        let texts: Vec<_> = (0..10).map(|i| layer(i, 0, 5_000)).collect();
        assert_eq!(band_rows(&texts).1, BAND_MAX_ROWS);
    }

    #[test]
    fn bar_drags_snap_and_keep_a_minimum_length() {
        let snaps = [5_000];
        // A move lands its end on the playhead when that's closer.
        assert_eq!(
            retime(Grab::Move, (1_000, 3_000), 1_950, &snaps, 100),
            (3_000, 5_000)
        );
        assert_eq!(
            retime(Grab::Move, (1_000, 3_000), -5_000, &snaps, 100),
            (0, 2_000)
        );
        assert_eq!(
            retime(Grab::Start, (1_000, 3_000), 5_000, &snaps, 100).0,
            3_000 - text::MIN_DURATION_MS
        );
        assert_eq!(
            retime(Grab::End, (1_000, 3_000), 1_940, &snaps, 100),
            (1_000, 5_000)
        );
        assert_eq!(
            retime(Grab::End, (1_000, 3_000), 300, &snaps, 100),
            (1_000, 3_300)
        );
    }
}
