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
    picker: Option<OpenPicker>,
    wheel: Option<(TextId, f64)>,
    font_menu_open: bool,
    pub(super) band_drag: Option<BandDrag>,
    /// Layer being typed into on the preview itself.
    inline: Option<TextId>,
    /// Give the inline field keyboard focus next frame.
    inline_focus: bool,
    /// The inline session already took its undo snapshot.
    inline_dirty: bool,
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
    let (l, t, r, b) =
        crate::textrender::ink_bounds(item, canvas.width() * ppp, canvas.height() * ppp);
    let c = center(item, canvas);
    let rect = egui::Rect::from_min_max(c + egui::vec2(l, t) / ppp, c + egui::vec2(r, b) / ppp);
    // A little air around the letters; tiny text stays grabbable.
    let rect = rect.expand(3.0);
    egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(rect.width().max(24.0), rect.height().max(20.0)),
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
        self.add_text_layer_on(None);
    }

    /// `add_text_layer` on a given text lane when it has room.
    pub(super) fn add_text_layer_on(&mut self, lane: Option<TrackId>) {
        if self.project.is_none() {
            return;
        }
        self.live.pause();
        self.text_ui.editing = false;
        self.push_undo();
        if let Some(p) = &mut self.project {
            let id = p.add_text(lane);
            // Type straight onto the preview.
            self.text_ui.inline = Some(id);
            self.text_ui.inline_focus = true;
            self.text_ui.inline_dirty = true;
        }
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
        let disabled: HashSet<TrackId> =
            p.tracks.iter().filter(|t| t.muted).map(|t| t.id).collect();
        p.texts
            .iter()
            .filter(|t| !disabled.contains(&t.track))
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
            // Clicking text that's already selected, or double-clicking
            // any text, types into it right on the preview.
            if (response.clicked() && selected == Some(item.id)) || response.double_clicked() {
                self.text_ui.inline = Some(item.id);
                self.text_ui.inline_focus = true;
                self.text_ui.inline_dirty = false;
            }
            if response.clicked() || response.drag_started() {
                took = true;
                if let Some(p) = &mut self.project {
                    p.select_text(Some(item.id));
                }
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
                // The frame hugs the ink, which needn't be centered on the
                // layer's anchor; keep that offset while moving.
                let anchor = egui::pos2(
                    canvas.left() + x0 * canvas.width(),
                    canvas.top() + y0 * canvas.height(),
                );
                let offset = drag.rect.center() - anchor;
                let c = moved.center() - offset;
                update.0 = ((c.x - canvas.left()) / canvas.width()).clamp(0.0, 1.0);
                update.1 = ((c.y - canvas.top()) / canvas.height()).clamp(0.0, 1.0);
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

    /// The floating field for typing into a text layer on the preview. It
    /// sits just under the text (above it near the bottom edge) so the
    /// styled result stays visible while typing. Enter finishes,
    /// Shift+Enter adds a line, Esc or clicking away closes it.
    pub(super) fn show_inline_text_editor(&mut self, ui: &mut egui::Ui, canvas: egui::Rect) {
        let Some(id) = self.text_ui.inline else {
            return;
        };
        let selected = self.project.as_ref().and_then(|p| p.selected_text);
        let Some(item) = self
            .project
            .as_ref()
            .and_then(|p| p.text(id))
            .filter(|_| selected == Some(id))
            .cloned()
        else {
            self.text_ui.inline = None;
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        let block = block_rect(&item, canvas, ppp);
        let width = block
            .width()
            .clamp(220.0, (canvas.width() - 16.0).max(220.0));
        let field_id = ui.id().with(("inline_text", id));
        let height = 64.0;
        let below = block.bottom() + 10.0 + height < canvas.bottom();
        let pos = egui::pos2(
            (block.center().x - width * 0.5).clamp(
                canvas.left() + 8.0,
                (canvas.right() - 8.0 - width).max(canvas.left() + 8.0),
            ),
            if below {
                block.bottom() + 10.0
            } else {
                (block.top() - 10.0 - height).max(canvas.top() + 4.0)
            },
        );
        let finish = ui.ctx().memory(|m| m.has_focus(field_id))
            // `consume_key` matches Enter with Shift held too (egui ignores
            // unrequested Shift), so leave Shift+Enter to the field's newline.
            && ui.input_mut(|i| {
                !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
            });
        let mut text = item.text.clone();
        let response = egui::Area::new(ui.id().with("inline_text_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(theme::BG_MODAL)
                    .stroke(Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.7)))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .shadow(egui::Shadow {
                        offset: egui::vec2(0.0, 4.0),
                        blur: 16.0,
                        spread: 0.0,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        let field = ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .id(field_id)
                                .frame(false)
                                .hint_text("Type your text…")
                                .font(theme::medium(15.0))
                                .text_color(theme::TEXT_PRIMARY)
                                .desired_rows(1)
                                .desired_width(width - 20.0)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    egui::Key::Enter,
                                )),
                        );
                        ui.label(
                            RichText::new("Enter to finish · Shift+Enter for a new line")
                                .font(theme::regular(10.5))
                                .color(theme::TEXT_FAINT),
                        );
                        field
                    })
                    .inner
            })
            .inner;
        if std::mem::take(&mut self.text_ui.inline_focus) {
            response.request_focus();
        }
        if text != item.text {
            if !self.text_ui.inline_dirty {
                self.push_undo();
                self.text_ui.inline_dirty = true;
            }
            if let Some(t) = self.project.as_mut().and_then(|p| p.text_mut(id)) {
                t.text = text;
            }
        }
        if finish || (response.lost_focus() && !self.text_ui.inline_focus) {
            self.text_ui.inline = None;
            self.text_ui.inline_dirty = false;
            response.surrender_focus();
        }
    }

    /// Paints video and text in lane order (deepest first), so a text lane
    /// placed under a video lane is covered by it, as in the export. Then
    /// the selection frame of the selected text.
    pub(super) fn paint_canvas_layers(
        &mut self,
        ui: &egui::Ui,
        canvas: egui::Rect,
        background: crate::framing::Background,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = (canvas.width() * ppp, canvas.height() * ppp);
        let selected = self.project.as_ref().and_then(|p| p.selected_text);
        let painter = ui.painter().with_clip_rect(canvas);
        let mut shown = self.canvas_texts();
        let live: HashSet<TextId> = shown.iter().map(|(t, _, _)| t.id).collect();
        self.text_ui.textures.retain(|id, _| live.contains(id));
        let depth_of: HashMap<TrackId, usize> = self
            .project
            .as_ref()
            .map(|p| {
                p.tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (t.id, i))
                    .collect()
            })
            .unwrap_or_default();
        let depth = |item: &TextItem| depth_of.get(&item.track).copied().unwrap_or(0);
        // Bottom to top; ties keep list order.
        shown.sort_by_key(|(t, _, _)| std::cmp::Reverse(depth(t)));
        let frames: Vec<usize> = self.live.frames.iter().map(|f| f.depth).collect();
        let mut painted = 0;
        let mut first = true;

        for (item, look, ghost) in &shown {
            // Video lanes below this text go first.
            let below = painted
                + frames[painted..]
                    .iter()
                    .take_while(|d| **d > depth(item))
                    .count();
            if first || below > painted {
                self.live
                    .paint_range(ui, canvas, background, painted..below, first);
                painted = below;
                first = false;
            }
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

        if first || painted < frames.len() {
            self.live
                .paint_range(ui, canvas, background, painted..frames.len(), first);
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
        // Everything below sizes to the panel; nothing may push it wider.
        let width = ui.available_width() - 2.0;
        ui.set_max_width(width);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Text")
                    .font(theme::semibold(17.0))
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close_button = widgets::icon_button(
                    ui,
                    widgets::Icon::Close,
                    24.0,
                    false,
                    theme::TEXT_PRIMARY,
                )
                .on_hover_text("Deselect");
                if close_button.clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(10.0);

        // Content. The same text can also be typed straight on the preview.
        let field_id = ui.id().with(("text_field", id));
        let field = ui.add(
            egui::TextEdit::multiline(&mut item.text)
                .id(field_id)
                .hint_text("Type your text…")
                .font(theme::medium(14.0))
                .margin(egui::vec2(10.0, 8.0))
                .desired_rows(2)
                .desired_width(width),
        );
        if std::mem::take(&mut self.text_ui.focus_field) {
            field.request_focus();
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new("Tip: double-click the text on the preview to type there.")
                .font(theme::regular(11.0))
                .color(theme::TEXT_FAINT),
        );

        divider(ui);
        section(ui, "Style");
        let current = item.style.preset_index();
        chip_grid(ui, 3, STYLE_PRESETS.len(), |ui, i, cell| {
            let (name, preset) = &STYLE_PRESETS[i];
            if style_chip(ui, name, preset, current == Some(i), cell).clicked() {
                item.style = item.style.with_preset(preset);
            }
        });

        divider(ui);
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
            egui::vec2(width, 34.0),
            width,
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
        ui.add_space(8.0);
        let align = match item.style.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        };
        if let Some(i) = widgets::segmented_fill(
            ui,
            "text_align",
            &[
                ("Left", "Align left"),
                ("Center", "Center"),
                ("Right", "Align right"),
            ],
            align,
            width,
        ) {
            item.style.align = [Align::Left, Align::Center, Align::Right][i];
        }

        // Color: fill, then the optional outline and box, each with the
        // same control (swatches, solid/gradient, stops, direction).
        divider(ui);
        let picker = &mut self.text_ui.picker;
        paint_header(ui, "Color", &mut item.style.gradient, item.style.color);
        fill_editor(
            ui,
            Paint::Text,
            &mut item.style.color,
            &mut item.style.gradient,
            picker,
            width,
        );

        divider(ui);
        let outline = OUTLINES
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - item.style.outline)
                    .abs()
                    .total_cmp(&(b.1 - item.style.outline).abs())
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        if let Some(i) = header_choice(
            ui,
            "Outline",
            "text_outline",
            &[("Off", ""), ("Thin", ""), ("Bold", "")],
            outline,
        ) {
            item.style.outline = OUTLINES[i];
        }
        if item.style.outline > 0.0 {
            ui.add_space(8.0);
            gradient_switch(
                ui,
                "outline_kind",
                &mut item.style.outline_gradient,
                item.style.outline_color,
                width,
            );
            fill_editor(
                ui,
                Paint::Outline,
                &mut item.style.outline_color,
                &mut item.style.outline_gradient,
                picker,
                width,
            );
        }

        divider(ui);
        let shadow = match item.style.shadow {
            Shadow::None => 0,
            Shadow::Soft => 1,
            Shadow::Glow => 2,
        };
        if let Some(i) = header_choice(
            ui,
            "Shadow",
            "text_shadow",
            &[
                ("Off", ""),
                ("Soft", "Drop shadow"),
                ("Glow", "Neon halo in the text color"),
            ],
            shadow,
        ) {
            item.style.shadow = [Shadow::None, Shadow::Soft, Shadow::Glow][i];
        }

        divider(ui);
        let boxed = match item.style.background {
            None => 0,
            Some(([r, ..], _)) if r < 128 => 1,
            Some(_) => 2,
        };
        if let Some(i) = header_choice(
            ui,
            "Box",
            "text_box",
            &[("Off", ""), ("Dark", ""), ("Light", "")],
            boxed,
        ) {
            item.style.background = match i {
                0 => None,
                1 => Some(([0, 0, 0], 0.72)),
                _ => Some(([255, 255, 255], 1.0)),
            };
        }
        if let Some((rgb, opacity)) = &mut item.style.background {
            ui.add_space(8.0);
            gradient_switch(ui, "box_kind", &mut item.style.box_gradient, *rgb, width);
            fill_editor(
                ui,
                Paint::Box,
                rgb,
                &mut item.style.box_gradient,
                picker,
                width,
            );
            ui.add_space(6.0);
            let mut percent = *opacity * 100.0;
            if labeled_slider(ui, "Opacity", &mut percent, 0.0..=100.0, "%", width) {
                *opacity = percent / 100.0;
            }
        }

        // Animation.
        divider(ui);
        section(ui, "Animate in");
        chip_grid(ui, 3, Intro::ALL.len(), |ui, i, cell| {
            let intro = Intro::ALL[i];
            if chip(ui, intro.label(), item.intro == intro, cell).clicked() {
                item.intro = intro;
            }
        });
        if item.intro != Intro::None {
            duration_control(ui, "Length", &mut item.intro_ms);
        }
        ui.add_space(10.0);
        section(ui, "Animate out");
        let per_row = if width >= 4.0 * 56.0 { 4 } else { 2 };
        chip_grid(ui, per_row, Outro::ALL.len(), |ui, i, cell| {
            let outro = Outro::ALL[i];
            if chip(ui, outro.label(), item.outro == outro, cell).clicked() {
                item.outro = outro;
            }
        });
        if item.outro != Outro::None {
            duration_control(ui, "Length", &mut item.outro_ms);
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

/// Which colored part of a text layer a color control edits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Paint {
    Text,
    Outline,
    Box,
}

/// One end of a gradient (`Start` is the solid color).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Stop {
    Start,
    End,
}

/// The open color picker: which part, which stop.
pub(super) struct OpenPicker {
    paint: Paint,
    stop: Stop,
    /// Picker shown inline (else only the stop is selected for swatches).
    open: bool,
    state: picker::PickerState,
}

pub(super) fn divider(ui: &mut egui::Ui) {
    ui.add_space(12.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::BORDER_SUBTLE);
    ui.add_space(12.0);
}

/// Section title with a choice on the right of the same row.
fn header_choice(
    ui: &mut egui::Ui,
    title: &str,
    id: &str,
    options: &[(&str, &str)],
    selected: usize,
) -> Option<usize> {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::medium(12.5))
                .color(theme::TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::segmented(ui, id, options, selected)
        })
        .inner
    })
    .inner
}

/// Title row for the main color: title plus Solid / Gradient.
fn paint_header(
    ui: &mut egui::Ui,
    title: &str,
    gradient: &mut Option<text::Gradient>,
    color: [u8; 3],
) {
    let kind = usize::from(gradient.is_some());
    if let Some(i) = header_choice(
        ui,
        title,
        "fill_kind",
        &[("Solid", ""), ("Gradient", "Blend two colors")],
        kind,
    ) {
        *gradient = (i == 1).then(|| default_gradient(color));
    }
    ui.add_space(8.0);
}

/// Solid / Gradient switch on its own row (outline, box).
fn gradient_switch(
    ui: &mut egui::Ui,
    id: &str,
    gradient: &mut Option<text::Gradient>,
    color: [u8; 3],
    width: f32,
) {
    let kind = usize::from(gradient.is_some());
    if let Some(i) = widgets::segmented_fill(
        ui,
        id,
        &[("Solid", ""), ("Gradient", "Blend two colors")],
        kind,
        width,
    ) {
        *gradient = (i == 1).then(|| default_gradient(color));
    }
    ui.add_space(8.0);
}

/// A gradient that starts as a visible variation of `color`.
fn default_gradient(color: [u8; 3]) -> text::Gradient {
    let hsva = egui::ecolor::Hsva::from_srgb(color);
    let end = if hsva.s < 0.15 {
        // White/grey/black: blend toward a soft blue.
        [120, 170, 255]
    } else {
        egui::ecolor::Hsva::new((hsva.h + 0.12).fract(), hsva.s, hsva.v.max(0.6), 1.0).to_srgb()
    };
    text::Gradient { end, angle: 0.0 }
}

/// Swatches + (for gradients) a stop bar and direction, + inline picker.
/// Swatches color the selected stop.
fn fill_editor(
    ui: &mut egui::Ui,
    paint: Paint,
    rgb: &mut [u8; 3],
    gradient: &mut Option<text::Gradient>,
    picker: &mut Option<OpenPicker>,
    width: f32,
) {
    ui.push_id(paint, |ui| {
        if gradient.is_none()
            && picker
                .as_ref()
                .is_some_and(|p| p.paint == paint && p.stop == Stop::End)
        {
            *picker = None;
        }
        let stop = picker
            .as_ref()
            .filter(|p| p.paint == paint)
            .map_or(Stop::Start, |p| p.stop);
        let open = picker.as_ref().is_some_and(|p| p.paint == paint && p.open);
        let target =
            |rgb: &mut [u8; 3], gradient: &mut Option<text::Gradient>, stop: Stop| -> [u8; 3] {
                match (stop, gradient.as_ref()) {
                    (Stop::End, Some(g)) => g.end,
                    _ => *rgb,
                }
            };
        let current = target(rgb, gradient, stop);
        let mut set: Option<[u8; 3]> = None;

        if let Some(g) = gradient.as_mut() {
            // Two color wells: pick one, then a swatch (or the picker)
            // colors it. Clicking the picked well again opens the picker.
            let swap_w = 30.0;
            let well_w = ((width - swap_w - 12.0) / 2.0).floor();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for which in [Stop::Start, Stop::End] {
                    let color = match which {
                        Stop::Start => *rgb,
                        Stop::End => g.end,
                    };
                    let active = stop == which;
                    let response = color_well(
                        ui,
                        match which {
                            Stop::Start => "Start",
                            Stop::End => "End",
                        },
                        color,
                        active,
                        well_w,
                    );
                    if response.clicked() {
                        *picker = Some(OpenPicker {
                            paint,
                            stop: which,
                            open: if active { !open } else { open },
                            state: picker::PickerState::new(color),
                        });
                    }
                    if which == Stop::Start
                        && swap_button(ui, swap_w)
                            .on_hover_text("Swap colors")
                            .clicked()
                    {
                        std::mem::swap(rgb, &mut g.end);
                    }
                }
            });
            ui.add_space(8.0);
            // Direction: each tile shows this gradient running that way.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Direction")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    for (angle, tip) in [
                        (135.0, "Top right to bottom left"),
                        (90.0, "Top to bottom"),
                        (45.0, "Top left to bottom right"),
                        (0.0, "Left to right"),
                    ] {
                        let on = (g.angle.rem_euclid(360.0) - angle).abs() < 1.0;
                        if direction_tile(ui, *rgb, g.end, angle, on)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            g.angle = angle;
                        }
                    }
                });
            });
            ui.add_space(10.0);
        }

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 4.0);
            for color in SWATCHES {
                if swatch(ui, color, current == color).clicked() {
                    set = Some(color);
                }
            }
            let custom = !SWATCHES.contains(&current);
            if custom_swatch(ui, current, custom || open).clicked() {
                *picker = Some(OpenPicker {
                    paint,
                    stop,
                    open: !open,
                    state: picker::PickerState::new(current),
                });
            }
        });
        if let Some(p) = picker.as_mut().filter(|p| p.paint == paint && p.open) {
            if p.state.rgb() != current && set.is_none() {
                p.state.set_rgb(current);
            }
            ui.add_space(8.0);
            if picker::show(ui, &mut p.state) {
                set = Some(p.state.rgb());
            }
        }
        if let Some(color) = set {
            match (stop, gradient.as_mut()) {
                (Stop::End, Some(g)) => g.end = color,
                _ => *rgb = color,
            }
        }
    });
}

/// One end of a gradient: a pill with its color and name; ringed in the
/// accent while it's the one the swatches color.
fn color_well(
    ui: &mut egui::Ui,
    label: &str,
    color: [u8; 3],
    active: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), widgets::CLICK);
    let fill = if active {
        theme::BG_BUTTON_ACTIVE
    } else if response.hovered() {
        theme::BG_BUTTON_HOVER
    } else {
        theme::BG_BUTTON
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, fill);
    painter.rect_stroke(
        rect.shrink(0.5),
        8.0,
        Stroke::new(
            if active { 1.5_f32 } else { 1.0 },
            if active { theme::ACCENT } else { theme::BORDER },
        ),
    );
    let dot = egui::pos2(rect.left() + 18.0, rect.center().y);
    painter.circle_filled(dot, 9.0, Color32::from_rgb(color[0], color[1], color[2]));
    painter.circle_stroke(
        dot,
        9.0,
        Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
    );
    painter.text(
        egui::pos2(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::medium(12.5),
        if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response
        .on_hover_text(if active {
            "Click again for the full color picker"
        } else {
            "Choose this end's color"
        })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Round button with two opposing arrows.
fn swap_button(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, 34.0), widgets::CLICK);
    let c = rect.center();
    if response.hovered() {
        ui.painter().circle_filled(c, 13.0, theme::BG_BUTTON_HOVER);
    }
    let stroke = Stroke::new(
        1.5_f32,
        if response.hovered() {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_MUTED
        },
    );
    for (dy, dir) in [(-3.0, 1.0), (3.0, -1.0)] {
        let tail = c + egui::vec2(-5.5 * dir, dy);
        let tip = c + egui::vec2(5.5 * dir, dy);
        ui.painter().line_segment([tail, tip], stroke);
        ui.painter()
            .line_segment([tip, tip + egui::vec2(-3.0 * dir, -2.5)], stroke);
        ui.painter()
            .line_segment([tip, tip + egui::vec2(-3.0 * dir, 2.5)], stroke);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small swatch of the gradient running at `angle` (0° = left to right,
/// 90° = top to bottom), matching how the text renders it.
fn direction_tile(
    ui: &mut egui::Ui,
    from: [u8; 3],
    to: [u8; 3],
    angle: f32,
    on: bool,
) -> egui::Response {
    // Square, so diagonals read differently from verticals.
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), widgets::CLICK);
    let tile = rect.shrink(if on { 2.5 } else { 3.0 });
    let (dy, dx) = angle.to_radians().sin_cos();
    let span = ((tile.width() * dx.abs() + tile.height() * dy.abs()) * 0.5).max(1.0);
    let lerp = |p: egui::Pos2| {
        let v = p - tile.center();
        let t = ((v.x * dx + v.y * dy) / span * 0.5 + 0.5).clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color32::from_rgb(
            mix(from[0], to[0]),
            mix(from[1], to[1]),
            mix(from[2], to[2]),
        )
    };
    let mut mesh = egui::Mesh::default();
    for corner in [
        tile.left_top(),
        tile.right_top(),
        tile.right_bottom(),
        tile.left_bottom(),
    ] {
        mesh.colored_vertex(corner, lerp(corner));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    let painter = ui.painter();
    painter.add(mesh);
    // Square tile corners, then a rounded frame drawn over them.
    painter.rect_stroke(tile, 4.0, Stroke::new(3.0_f32, theme::BG_PANEL));
    painter.rect_stroke(
        rect.shrink(0.75),
        6.0,
        if on {
            Stroke::new(1.5_f32, theme::ACCENT)
        } else if response.hovered() {
            Stroke::new(1.0_f32, theme::TEXT_MUTED)
        } else {
            Stroke::new(1.0_f32, theme::BORDER)
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Label, a slider filling the row, and the value.
pub(super) fn labeled_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    width: f32,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme::regular(12.0))
                .color(theme::TEXT_MUTED),
        );
        ui.spacing_mut().slider_width = (width - 140.0).max(60.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Slider::new(value, range)
                    .fixed_decimals(0)
                    .suffix(suffix),
            )
            .changed()
        })
        .inner
    })
    .inner
}

pub(super) fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .font(theme::medium(12.0))
            .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(6.0);
}

/// Lays `count` equal-width cells out `per_row` to a row.
pub(super) fn chip_grid(
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
pub(super) fn chip(ui: &mut egui::Ui, label: &str, active: bool, width: f32) -> egui::Response {
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

// ── Timeline text lanes ────────────────────────────────────────────────

/// Grab zone at each end of a bar for trimming, in points.
const TRIM_GRAB: f32 = 7.0;
/// Snap distance for bar edges, in points.
const BAND_SNAP: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Grab {
    Move,
    Start,
    End,
}

/// Where a text bar dragged off its lane lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LaneTarget {
    /// An existing text lane with room.
    Lane(TrackId),
    /// A new text lane inserted at this row of the timeline view.
    New(usize),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BandDrag {
    id: TextId,
    grab: Grab,
    origin: (i64, i64),
    press: egui::Pos2,
    target: Option<LaneTarget>,
}

impl BandDrag {
    /// The text being moved as a whole (not trimmed).
    pub(super) fn moving(&self) -> Option<TextId> {
        (self.grab == Grab::Move).then_some(self.id)
    }
    /// The lane the timeline resolved for the pointer this frame.
    pub(super) fn set_target(&mut self, target: LaneTarget) {
        self.target = Some(target);
    }
}

/// What the user did on a text lane this frame; applied by the caller once
/// the project is no longer borrowed.
#[derive(Debug, PartialEq)]
pub(super) enum BandAction {
    /// New layer at this time (or the playhead), on this lane if it has room.
    Add(Option<i64>, Option<TrackId>),
    Select(TextId),
    Retime {
        id: TextId,
        start: i64,
        end: i64,
        /// First frame of the gesture: take an undo snapshot.
        begin: bool,
    },
    Relane {
        id: TextId,
        target: LaneTarget,
    },
    Split(TextId),
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

/// Paints one text bar.
/// A text bar picked up by a drag: selected look plus a drop shadow.
pub(super) fn paint_lifted_text_bar(painter: &egui::Painter, bar: egui::Rect, item: &TextItem) {
    painter.add(
        egui::Shadow {
            offset: egui::vec2(0.0, 3.0),
            blur: 10.0,
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        }
        .as_shape(bar, 6.0),
    );
    paint_text_bar(painter, bar, item, true, true, false);
}

fn paint_text_bar(
    painter: &egui::Painter,
    bar: egui::Rect,
    item: &TextItem,
    selected: bool,
    hot: bool,
    disabled: bool,
) {
    let fill = if selected {
        theme::CLIP_TEXT_EDGE.gamma_multiply(0.8)
    } else if hot {
        theme::CLIP_TEXT.gamma_multiply(1.25)
    } else {
        theme::CLIP_TEXT
    };
    let fade = if disabled { 0.4 } else { 1.0 };
    painter.rect_filled(bar, 5.0, fill.gamma_multiply(fade));
    painter.rect_stroke(
        bar.shrink(if selected { 1.0 } else { 0.5 }),
        5.0,
        if selected {
            Stroke::new(2.0_f32, theme::ACCENT)
        } else {
            Stroke::new(1.0_f32, theme::CLIP_TEXT_EDGE.gamma_multiply(fade))
        },
    );
    let clip = painter.with_clip_rect(
        bar.shrink2(egui::vec2(6.0, 0.0))
            .intersect(painter.clip_rect()),
    );
    let glyph = egui::pos2(bar.left() + 14.0, bar.center().y);
    if bar.width() > 34.0 {
        clip.text(
            glyph,
            Align2::CENTER_CENTER,
            "T",
            theme::semibold(13.0),
            theme::TEXT_PRIMARY.gamma_multiply(0.85 * fade),
        );
    }
    let first_line = item.text.lines().next().unwrap_or("").trim();
    let (label, color) = if first_line.is_empty() {
        ("Text", theme::TEXT_MUTED)
    } else {
        (first_line, theme::TEXT_PRIMARY)
    };
    clip.text(
        egui::pos2(
            bar.left() + if bar.width() > 34.0 { 26.0 } else { 6.0 },
            bar.center().y,
        ),
        Align2::LEFT_CENTER,
        label,
        theme::medium(12.0),
        color.gamma_multiply(fade),
    );
}

/// Draws the bars of one text lane (`lane` = its time area) and handles
/// their input: drag to move (also onto other text lanes, or between
/// lanes for a new one), drag an end to trim, double-click empty space
/// to add text there. `snaps` are times bar edges stick to.
#[allow(clippy::too_many_arguments)]
pub(super) fn show_text_lane(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    lane: egui::Rect,
    lane_id: TrackId,
    inset: f32,
    lanes: editor::Lanes,
    texts: &[TextItem],
    selected: Option<TextId>,
    snaps: &[i64],
    disabled: bool,
    drag: &mut Option<BandDrag>,
) -> Vec<BandAction> {
    let mut actions = Vec::new();
    // Empty lane space: click seeks, double-click adds text there.
    let background = ui
        .interact(
            lane,
            ui.id().with(("text_lane_bg", lane_id)),
            egui::Sense::click(),
        )
        .on_hover_text_at_pointer("Double-click to add text here");
    if let Some(pos) = background.interact_pointer_pos() {
        if background.double_clicked() {
            actions.push(BandAction::Add(Some(lanes.time_at(pos.x)), Some(lane_id)));
        } else if background.clicked() {
            actions.push(BandAction::Seek(lanes.time_at(pos.x)));
        }
    }

    let reach = (BAND_SNAP / lanes.width * lanes.span as f32).round() as i64;
    for item in texts.iter().filter(|t| t.track == lane_id) {
        let x0 = lanes.x_of(item.start_ms);
        let x1 = lanes.x_of(item.end_ms).max(x0 + 4.0);
        let bar = egui::Rect::from_min_max(
            egui::pos2(x0, lane.top() + inset),
            egui::pos2(x1, lane.bottom() - inset),
        );
        let response = ui.interact(
            bar.intersect(lane),
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
        if response.double_clicked() {
            actions.push(BandAction::Select(item.id));
        }
        if response.drag_started() {
            let press = ui
                .input(|i| i.pointer.press_origin())
                .unwrap_or(bar.center());
            *drag = Some(BandDrag {
                id: item.id,
                grab: grab_at(press.x),
                origin: (item.start_ms, item.end_ms),
                press,
                target: None,
            });
            actions.push(BandAction::Select(item.id));
        }
        if let (Some(d), Some(pos)) = (
            drag.filter(|d| d.id == item.id),
            response.interact_pointer_pos(),
        ) {
            if response.dragged() {
                let delta = ((pos.x - d.press.x) / lanes.width * lanes.span as f32).round() as i64;
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
                let (mut start, mut end) = retime(d.grab, d.origin, delta, &edges, reach);
                // Trims stop at the lane neighbours.
                let prev_end = texts
                    .iter()
                    .filter(|t| t.track == lane_id && t.id != item.id && t.end_ms <= d.origin.0)
                    .map(|t| t.end_ms)
                    .max()
                    .unwrap_or(0);
                let next_start = texts
                    .iter()
                    .filter(|t| t.track == lane_id && t.id != item.id && t.start_ms >= d.origin.1)
                    .map(|t| t.start_ms)
                    .min()
                    .unwrap_or(i64::MAX);
                match d.grab {
                    Grab::Start => start = start.max(prev_end),
                    Grab::End => end = end.min(next_start),
                    Grab::Move => {}
                }
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
        // The timeline resolves the lane (see `editor::resolve_lane`).
        let target = drag.filter(|d| d.id == item.id).and_then(|d| d.target);
        if response.drag_stopped() {
            if let Some(target) = target.filter(|t| *t != LaneTarget::Lane(lane_id)) {
                actions.push(BandAction::Relane {
                    id: item.id,
                    target,
                });
            }
            *drag = None;
        }
        response.context_menu(|ui| {
            ui.set_width(200.0);
            if widgets::menu_item(ui, "Split at playhead", Some("S")).clicked() {
                actions.push(BandAction::Split(item.id));
                ui.close_menu();
            }
            if widgets::menu_item(ui, "Duplicate", None).clicked() {
                actions.push(BandAction::Duplicate(item.id));
                ui.close_menu();
            }
            widgets::menu_separator(ui);
            if widgets::menu_item(ui, "Delete", Some("Del")).clicked() {
                actions.push(BandAction::Delete(item.id));
                ui.close_menu();
            }
        });

        let is_selected = selected == Some(item.id);
        let hot = response.hovered() || active.is_some();
        // A bar being moved is drawn by the timeline, gliding to its lane.
        if target.is_none() {
            paint_text_bar(painter, bar, item, is_selected, hot, disabled);
        }
    }
    actions
}

impl ClipperApp {
    /// Applies what happened in the timeline text band.
    pub(super) fn apply_band_actions(&mut self, actions: Vec<BandAction>, view: &[Track]) {
        for action in actions {
            match action {
                BandAction::Add(at, lane) => {
                    if let Some(ms) = at {
                        self.seek_to(ms);
                    }
                    self.add_text_layer_on(lane);
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
                BandAction::Relane { id, target } => {
                    let Some(p) = &mut self.project else { continue };
                    let lane = match target {
                        LaneTarget::Lane(lane) => lane,
                        LaneTarget::New(slot) => {
                            let index = view
                                .get(slot)
                                .and_then(|t| p.tracks.iter().position(|x| x.id == t.id))
                                .unwrap_or(p.tracks.len());
                            p.new_track(TrackKind::Text, index)
                        }
                    };
                    if let Some(t) = p.text_mut(id) {
                        t.track = lane;
                    }
                    p.prune_tracks();
                }
                BandAction::Split(id) => {
                    self.push_undo();
                    if let Some(p) = &mut self.project {
                        p.select_text(Some(id));
                        p.split_text(id);
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
                        if !p.text_lane_free(copy.track, copy.start_ms, copy.end_ms, None) {
                            let index = p
                                .tracks
                                .iter()
                                .position(|t| t.id == copy.track)
                                .unwrap_or(0);
                            copy.track = p.new_track(TrackKind::Text, index);
                        }
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
                        p.prune_tracks();
                    }
                }
                BandAction::Seek(ms) => {
                    if let Some(p) = &mut self.project {
                        p.selected_text = None;
                        p.selected_clips.clear();
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

/// Shared compact timing control for text and clip animations: a label,
/// a slider filling the row and the length in seconds.
pub(super) fn duration_control(ui: &mut egui::Ui, label: &str, ms: &mut f32) {
    ui.add_space(6.0);
    let width = ui.available_width();
    let mut seconds = *ms / 1000.0;
    let changed = ui
        .horizontal(|ui| {
            ui.label(
                RichText::new(label)
                    .font(theme::regular(12.0))
                    .color(theme::TEXT_MUTED),
            );
            ui.spacing_mut().slider_width = (width - 140.0).max(60.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Slider::new(&mut seconds, 0.05..=5.0)
                        .logarithmic(true)
                        .fixed_decimals(2)
                        .suffix(" s"),
                )
                .on_hover_text("Shorter = faster. Short clips shorten the animation to fit.")
                .changed()
            })
            .inner
        })
        .inner;
    if changed {
        *ms = seconds * 1000.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
