use super::*;

#[derive(Clone, Debug)]
enum EditorDrag {
    Asset(TrackId),
    Clip { id: ClipId, grab_ms: i64 },
    Track(TrackId),
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
        let mut transform = c.transform;
        let previous = transform;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut transform.fit, crate::framing::Fit::Fit, "Fit")
                .on_hover_text("Show the whole video");
            ui.selectable_value(&mut transform.fit, crate::framing::Fit::Fill, "Fill")
                .on_hover_text("Fill the canvas, cropping the edges");
            if ui.small_button("Reset").clicked() {
                transform = Default::default();
            }
        });
        let zoom = ui.add(
            egui::Slider::new(&mut transform.scale, 0.03..=8.0)
                .logarithmic(true)
                .text("Zoom"),
        );
        if transform != previous {
            if zoom.drag_started() || !zoom.dragged() {
                self.push_undo();
            }
            if let Some(clip) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|clip| clip.id == c.id))
            {
                clip.transform = transform;
            }
        }
        let mut opacity = c.transform.opacity;
        let response = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity"));
        if response.drag_started() || (response.changed() && !response.dragged()) {
            self.push_undo();
        }
        if response.changed() {
            if let Some(c) = self
                .project
                .as_mut()
                .and_then(|p| p.clips.iter_mut().find(|clip| clip.id == c.id))
            {
                c.transform.opacity = opacity;
            }
        }
        ui.add_space(8.0);
    }

    pub(super) fn show_media_bin(&mut self, ui: &mut egui::Ui) {
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
        if assets.is_empty() {
            ui.add_space(18.0);
            ui.label(
                RichText::new(if advanced {
                    "Import video or audio, then drag it onto the timeline."
                } else {
                    "Open a video to begin."
                })
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
                        if hover {
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
                    child.add(egui::Label::new(RichText::new(name.as_ref()).size(12.5)).truncate());
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
        if let Some(id) = remove {
            self.push_undo();
            if let Some(p) = &mut self.project {
                p.remove_asset(id);
            }
            self.live.clear();
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
                    .map(|a| {
                        a.source
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }),
                _ => None,
            };
            if let (Some(name), Some(pointer)) = (name, ui.ctx().pointer_hover_pos()) {
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
    pub(super) fn show_editor_timeline(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project.as_ref() else {
            return;
        };
        let playhead = p.playhead_ms;
        let end = p.content_end_ms();
        let can_export = !p.clips.is_empty() && self.export.is_none();
        let mut toggle = false;
        let mut export = false;
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(if self.live.playing { "Pause" } else { "Play" })
                        .min_size(egui::vec2(50.0, 28.0)),
                )
                .clicked()
            {
                toggle = true;
            }
            ui.label(
                RichText::new(media::format_timestamp(playhead))
                    .monospace()
                    .size(14.0),
            );
            ui.label(
                RichText::new(format!("/ {}", media::format_timestamp(end)))
                    .monospace()
                    .size(11.0)
                    .color(theme::TEXT_MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, "Export…", can_export).clicked() {
                    export = true;
                }
                ui.spacing_mut().slider_width = 70.0;
                let response =
                    ui.add(egui::Slider::new(&mut self.volume_ui, 0.0..=130.0).show_value(false));
                if response.changed() {
                    let _ = self.player.set_volume(self.volume_ui);
                }
                let muted = self.player.state().muted;
                if ui
                    .small_button(if muted { "Muted" } else { "Audio" })
                    .clicked()
                {
                    let _ = self.player.toggle_mute();
                }
            });
        });
        ui.add_space(6.0);
        if toggle {
            self.toggle_play();
        }
        if export {
            self.export_options_open = true;
        }
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
        let mut labels: HashMap<_, _> = clips
            .iter()
            .map(|c| {
                (
                    c.id,
                    p.clip_source(c)
                        .0
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
        labels.extend(compound_labels);
        let payload = egui::DragAndDrop::payload::<EditorDrag>(ui.ctx());
        let asset_durations: HashMap<_, _> = p
            .assets
            .iter()
            .map(|a| (a.id, a.source.media.duration_ms))
            .collect();
        let source_kinds: HashMap<_, _> = p
            .assets
            .iter()
            .map(|a| {
                (
                    a.id,
                    if a.source.media.width > 0 {
                        TrackKind::Video
                    } else {
                        TrackKind::Audio
                    },
                )
            })
            .collect();
        let mut seek = None;
        let mut select = None;
        let mut clear = false;
        let mut mute = None;
        let mut drop = None;
        let mut context_action = None;
        let height = (ui.available_height() - 19.0).max(70.0);
        egui::ScrollArea::vertical()
            .id_salt("timeline_lanes")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let lane_h = 38.0;
                let ruler = 24.0;
                let header = 55.0;
                let rows = tracks.len().max(1);
                let total_h = ruler + rows as f32 * lane_h + 34.0;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), total_h),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, theme::BG_PREVIEW);
                let left = rect.left() + header;
                let width = (rect.width() - header - 4.0).max(1.0);
                let x_of = |time: i64| left + time as f32 / duration as f32 * width;
                let time_at =
                    |x: f32| (((x - left) / width) * duration as f32).round().max(0.0) as i64;
                let ruler_rect = egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    egui::pos2(rect.right(), rect.top() + ruler),
                );
                let tick_count = (width / 100.0).floor().max(1.0) as i64;
                for n in 0..=tick_count {
                    let t = duration * n / tick_count;
                    let x = x_of(t);
                    painter.line_segment(
                        [
                            egui::pos2(x, rect.top() + ruler - 5.0),
                            egui::pos2(x, rect.top() + ruler),
                        ],
                        Stroke::new(1.0_f32, theme::BORDER),
                    );
                    if n < tick_count {
                        painter.text(
                            egui::pos2(x + 4.0, rect.top() + 8.0),
                            Align2::LEFT_CENTER,
                            format!("{}:{:02}", t / 60000, (t / 1000) % 60),
                            egui::FontId::monospace(10.0),
                            theme::TEXT_MUTED,
                        );
                    }
                }
                let ruler_response = ui.interact(
                    ruler_rect,
                    ui.id().with("timeline_ruler"),
                    egui::Sense::click_and_drag(),
                );
                if ruler_response.clicked() || ruler_response.dragged() {
                    seek = ruler_response
                        .interact_pointer_pos()
                        .map(|pos| time_at(pos.x).min(duration));
                    self.user_dragging_seek = ruler_response.dragged();
                }
                for (index, t) in tracks.iter().enumerate() {
                    let top = rect.top() + ruler + index as f32 * lane_h;
                    let row = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), top),
                        egui::pos2(rect.right(), top + lane_h),
                    );
                    painter.line_segment(
                        [egui::pos2(left, top), egui::pos2(rect.right(), top)],
                        Stroke::new(1.0_f32, theme::BG_PANEL),
                    );
                    let title =
                        egui::Rect::from_min_max(row.min, egui::pos2(left - 4.0, row.bottom()));
                    let title_response = ui
                        .interact(
                            title,
                            ui.id().with(("track_drag", t.id)),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    title_response.dnd_set_drag_payload(EditorDrag::Track(t.id));
                    painter.text(
                        egui::pos2(title.left() + 7.0, title.center().y),
                        Align2::LEFT_CENTER,
                        &t.name,
                        egui::FontId::monospace(11.0),
                        if t.muted {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT_PRIMARY
                        },
                    );
                    title_response.context_menu(|ui| {
                        if ui
                            .button(if t.muted {
                                "Enable track"
                            } else {
                                "Mute / hide track"
                            })
                            .clicked()
                        {
                            mute = Some(t.id);
                            ui.close_menu();
                        }
                    });
                    let row_response = ui.interact(
                        egui::Rect::from_min_max(egui::pos2(left, top), row.max),
                        ui.id().with(("lane", t.id)),
                        egui::Sense::click(),
                    );
                    if row_response.clicked() {
                        seek = row_response
                            .interact_pointer_pos()
                            .map(|pos| time_at(pos.x).min(duration));
                        clear = true;
                    }
                    for c in clips.iter().filter(|c| c.track_id == t.id) {
                        let block = egui::Rect::from_min_max(
                            egui::pos2(x_of(c.cut_start_ms), top + 4.0),
                            egui::pos2(
                                x_of(c.cut_end_ms()).max(x_of(c.cut_start_ms) + 4.0),
                                top + lane_h - 4.0,
                            ),
                        );
                        let color = if t.kind == TrackKind::Video {
                            Color32::from_rgb(48, 66, 88)
                        } else {
                            Color32::from_rgb(42, 70, 59)
                        };
                        painter.rect_filled(
                            block,
                            3.0,
                            if t.muted {
                                color.linear_multiply(0.4)
                            } else {
                                color
                            },
                        );
                        if selected.contains(&c.id) {
                            painter.rect_stroke(
                                block.shrink(0.5),
                                3.0,
                                Stroke::new(1.5_f32, theme::ACCENT),
                            );
                        }
                        if block.width() > 35.0 {
                            let text_painter = painter.with_clip_rect(block.shrink(5.0));
                            text_painter.text(
                                block.left_center() + egui::vec2(7.0, 0.0),
                                Align2::LEFT_CENTER,
                                labels.get(&c.id).map(String::as_str).unwrap_or("Clip"),
                                egui::FontId::proportional(11.5),
                                theme::TEXT_PRIMARY,
                            );
                        }
                        let response = ui
                            .interact(
                                block,
                                ui.id().with(("timeline_clip", c.id)),
                                egui::Sense::click_and_drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::Grab);
                        if response.clicked() {
                            select = Some(c.id);
                        }
                        let grab = ui
                            .input(|i| i.pointer.press_origin())
                            .map(|pos| time_at(pos.x) - c.cut_start_ms)
                            .unwrap_or(0);
                        response.dnd_set_drag_payload(EditorDrag::Clip {
                            id: c.id,
                            grab_ms: grab,
                        });
                        response.context_menu(|ui| {
                            if ui.button("Split at playhead  ·  S").clicked() {
                                if !selected.contains(&c.id) {
                                    select = Some(c.id);
                                }
                                context_action = Some(ClipAction::Split);
                                ui.close_menu();
                            }
                            let context_selection = if selected.contains(&c.id) {
                                p.selected_clips.clone()
                            } else {
                                [c.id].into_iter().collect()
                            };
                            if p.can_detach_ids(&context_selection)
                                && ui.button("Detach audio  ·  D").clicked()
                            {
                                if !selected.contains(&c.id) {
                                    select = Some(c.id);
                                }
                                context_action = Some(ClipAction::DetachSelected);
                                ui.close_menu();
                            }
                            if p.can_attach_ids(&context_selection)
                                && ui.button("Attach audio  ·  Shift+D").clicked()
                            {
                                if !selected.contains(&c.id) {
                                    select = Some(c.id);
                                }
                                context_action = Some(ClipAction::AttachSelected);
                                ui.close_menu();
                            }
                            if ui.button("Delete  ·  Del").clicked() {
                                if !selected.contains(&c.id) {
                                    select = Some(c.id);
                                }
                                context_action = Some(ClipAction::DeleteSelected);
                                ui.close_menu();
                            }
                        });
                    }
                }
                if tracks.is_empty() {
                    painter.text(
                        egui::pos2(left + 16.0, rect.top() + ruler + 19.0),
                        Align2::LEFT_CENTER,
                        "Drop video or audio here",
                        egui::FontId::proportional(12.0),
                        theme::TEXT_MUTED,
                    );
                }
                let x = x_of(playhead).clamp(left, rect.right());
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top() + 18.0),
                        egui::pos2(x, rect.bottom()),
                    ],
                    Stroke::new(1.5_f32, theme::TEXT_PRIMARY),
                );
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(x - 5.0, rect.top() + 16.0),
                        egui::pos2(x + 5.0, rect.top() + 16.0),
                        egui::pos2(x, rect.top() + 22.0),
                    ],
                    theme::TEXT_PRIMARY,
                    Stroke::NONE,
                ));
                if let (Some(payload), Some(pos)) = (payload.as_ref(), ui.ctx().pointer_hover_pos())
                {
                    if rect.intersect(ui.clip_rect()).contains(pos) {
                        let row =
                            ((pos.y - (rect.top() + ruler)) / lane_h).floor().max(0.0) as usize;
                        let within = (pos.y - (rect.top() + ruler)).rem_euclid(lane_h);
                        let visual_index = if within > lane_h * 0.5 { row + 1 } else { row };
                        let index = tracks
                            .get(visual_index)
                            .and_then(|t| real_indices.get(&t.id))
                            .copied()
                            .unwrap_or(self.project.as_ref().unwrap().tracks.len());
                        match payload.as_ref() {
                            EditorDrag::Track(_) => {
                                let y = rect.top()
                                    + ruler
                                    + visual_index.min(tracks.len()) as f32 * lane_h;
                                painter.line_segment(
                                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                                    Stroke::new(2.0_f32, theme::ACCENT),
                                );
                            }
                            _ => {
                                let (length, grab, kind, exclude) = match payload.as_ref() {
                                    EditorDrag::Asset(id) => (
                                        *asset_durations.get(id).unwrap_or(&0),
                                        0,
                                        *source_kinds.get(id).unwrap_or(&TrackKind::Video),
                                        None,
                                    ),
                                    EditorDrag::Clip { id, grab_ms } => {
                                        let clip = clips.iter().find(|c| c.id == *id);
                                        (
                                            clip.map(|c| c.duration_ms()).unwrap_or(0),
                                            *grab_ms,
                                            clip.and_then(|c| {
                                                tracks.iter().find(|t| t.id == c.track_id)
                                            })
                                            .map(|t| t.kind)
                                            .unwrap_or(TrackKind::Video),
                                            Some(*id),
                                        )
                                    }
                                    _ => unreachable!(),
                                };
                                let videos =
                                    tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
                                let insertion = if kind == TrackKind::Video {
                                    visual_index.min(videos)
                                } else {
                                    visual_index.max(videos)
                                };
                                let index = tracks
                                    .get(insertion)
                                    .and_then(|t| real_indices.get(&t.id))
                                    .copied()
                                    .unwrap_or(self.project.as_ref().unwrap().tracks.len());
                                let mut start = (time_at(pos.x) - grab).max(0);
                                if !ui.input(|i| i.modifiers.shift) {
                                    let threshold = (12.0 / width * duration as f32).ceil() as i64;
                                    let mut candidates = vec![0, playhead];
                                    for c in clips.iter().filter(|c| Some(c.id) != exclude) {
                                        candidates.extend([
                                            c.cut_start_ms,
                                            c.cut_end_ms(),
                                            c.cut_start_ms - length,
                                            c.cut_end_ms() - length,
                                        ]);
                                    }
                                    if let Some(snap) = candidates
                                        .into_iter()
                                        .filter(|t| *t >= 0 && (start - *t).abs() < threshold)
                                        .min_by_key(|t| (start - *t).abs())
                                    {
                                        start = snap;
                                    }
                                }
                                let near_edge = within < 7.0 || within > lane_h - 7.0;
                                let target = if near_edge
                                    || matches!(payload.as_ref(), EditorDrag::Asset(_))
                                {
                                    None
                                } else {
                                    tracks.get(row).filter(|t| t.kind == kind).map(|t| t.id)
                                }
                                .filter(|track| {
                                    !clips.iter().any(|c| {
                                        c.track_id == *track
                                            && Some(c.id) != exclude
                                            && start < c.cut_end_ms()
                                            && start + length > c.cut_start_ms
                                    })
                                });
                                let y = if target.is_some() {
                                    rect.top() + ruler + row as f32 * lane_h + 4.0
                                } else {
                                    rect.top() + ruler + insertion.min(tracks.len()) as f32 * lane_h
                                };
                                let ghost = egui::Rect::from_min_size(
                                    egui::pos2(x_of(start), y),
                                    egui::vec2(
                                        (length as f32 / duration as f32 * width).max(10.0),
                                        lane_h - 8.0,
                                    ),
                                );
                                if target.is_none() {
                                    painter.line_segment(
                                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                                        Stroke::new(2.0_f32, theme::ACCENT),
                                    );
                                }
                                painter.rect_filled(
                                    ghost,
                                    2.0,
                                    theme::ACCENT.linear_multiply(0.45),
                                );
                                painter.rect_stroke(
                                    ghost,
                                    2.0,
                                    Stroke::new(1.0_f32, theme::ACCENT),
                                );
                                if ui.input(|i| i.pointer.any_released()) {
                                    drop = Some((payload.as_ref().clone(), start, target, index));
                                }
                            }
                        }
                        if let EditorDrag::Track(id) = payload.as_ref() {
                            if ui.input(|i| i.pointer.any_released()) {
                                drop = Some((EditorDrag::Track(*id), 0, None, index));
                            }
                        }
                    }
                }
            });
        if clear {
            if let Some(p) = &mut self.project {
                p.selected_clips.clear();
            }
        }
        if let Some(id) = select {
            ui.ctx().request_repaint();
            if let Some(p) = &mut self.project {
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
        ui.label(
            RichText::new(
                "Drag media to place · drag track names to reorder · S split · Del remove",
            )
            .size(10.0)
            .color(theme::TEXT_MUTED),
        );
    }
}
