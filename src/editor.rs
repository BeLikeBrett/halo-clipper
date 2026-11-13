use super::*;

#[derive(Clone, Debug)]
pub(super) enum EditorDrag {
    Asset(TrackId),
    Clip { id: ClipId, grab_ms: i64 },
    Track(TrackId),
}
const LANE_HEIGHT: f32 = 54.0;
const RULER_HEIGHT: f32 = 30.0;

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
fn insertion_slot(y: f32, top: f32, tracks: &[Track], kind: TrackKind) -> usize {
    let nearest = ((y - top) / LANE_HEIGHT + 0.5).floor().max(0.0) as usize;
    let videos = tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
    if kind == TrackKind::Video {
        nearest.min(videos)
    } else {
        nearest.max(videos).min(tracks.len())
    }
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
fn placement(
    p: &Project,
    tracks: &[Track],
    clips: &[Clip],
    payload: &EditorDrag,
    pos: egui::Pos2,
    rect: egui::Rect,
    ctx: &egui::Context,
) -> Option<Placement> {
    let width = (rect.width() - 68.0).max(1.0);
    let duration = p.canvas_length_ms().max(1000);
    let left = rect.left() + 64.0;
    let time_at = |x: f32| (((x - left) / width) * duration as f32).round().max(0.0) as i64;
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
                a.source
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
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
    let mut start = (time_at(pos.x) - grab).max(0);
    let mut snap = None;
    if !ctx.input(|i| i.modifiers.shift) {
        let threshold = (12.0 / width * duration as f32).ceil() as i64;
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
    let top = rect.top() + RULER_HEIGHT;
    let slot = insertion_slot(pos.y, top, tracks, kind);
    let row = ((pos.y - top) / LANE_HEIGHT).floor().max(0.0) as usize;
    let within = (pos.y - top).rem_euclid(LANE_HEIGHT);
    let target = if matches!(payload, EditorDrag::Clip { .. })
        && within > 10.0
        && within < LANE_HEIGHT - 10.0
    {
        tracks
            .get(row)
            .filter(|t| t.kind == kind)
            .map(|t| t.id)
            .filter(|id| {
                !clips.iter().any(|c| {
                    c.track_id == *id
                        && Some(c.id) != exclude
                        && start < c.cut_end_ms()
                        && start + length > c.cut_start_ms
                })
            })
    } else {
        None
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
        let mut focus = None;
        let active_simple = self.project.as_ref().and_then(|p| p.simple.focus);
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
        let images: HashMap<_, _> = clips
            .iter()
            .filter(|c| p.is_video(c.id))
            .filter_map(|c| {
                self.thumbnails
                    .get(ui.ctx(), p.clip_source(c).0)
                    .map(|texture| (c.id, texture))
            })
            .collect();
        if payload.is_none() {
            ui.ctx().data_mut(|data| {
                data.remove::<(TrackId, f32, bool)>(egui::Id::new("first_timeline_placement"))
            });
        }
        let mut seek = None;
        let mut select = None;
        let mut clear = false;
        let mut mute = None;
        let mut drop = None;
        let mut context_action = None;
        let height = (ui.available_height() - 30.0).max(70.0);
        egui::ScrollArea::vertical()
            .id_salt("timeline_lanes")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let lane_h = LANE_HEIGHT;
                let ruler = RULER_HEIGHT;
                let header = 64.0;
                let rows = tracks.len().max(1);
                let reserve = usize::from(payload.is_some() && !tracks.is_empty());
                let total_h = (ruler + (rows + reserve) as f32 * lane_h + 12.0).max(height - 2.0);
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
                let hover = ui
                    .ctx()
                    .pointer_hover_pos()
                    .filter(|pos| rect.intersect(ui.clip_rect()).contains(*pos));
                let placement = payload.as_ref().and_then(|drag| {
                    hover.and_then(|pos| placement(p, &tracks, &clips, drag, pos, rect, ui.ctx()))
                });
                let gap = placement
                    .as_ref()
                    .filter(|drop| drop.target.is_none())
                    .map(|drop| drop.slot);
                let step = ruler_step(duration, width);
                for n in 0..=duration / step {
                    let t = n * step;
                    let x = x_of(t);
                    painter.line_segment(
                        [
                            egui::pos2(x, rect.top() + ruler - 7.0),
                            egui::pos2(x, rect.bottom()),
                        ],
                        Stroke::new(1.0_f32, Color32::from_rgb(29, 33, 40)),
                    );
                    let label = if step < 1000 {
                        format!("{}:{:02}.{}", t / 60000, (t / 1000) % 60, (t % 1000) / 100)
                    } else {
                        format!("{}:{:02}", t / 60000, (t / 1000) % 60)
                    };
                    painter.text(
                        egui::pos2(x + 4.0, rect.top() + 11.0),
                        Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(11.0),
                        theme::TEXT_MUTED,
                    );
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
                    let target_shift = if gap.is_some_and(|slot| index >= slot) {
                        lane_h
                    } else {
                        0.0
                    };
                    let shift = ui.ctx().animate_value_with_time(
                        ui.id().with(("lane_insertion", t.id)),
                        target_shift,
                        0.10,
                    );
                    let top = rect.top() + ruler + index as f32 * lane_h + shift;
                    let row = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), top),
                        egui::pos2(rect.right(), top + lane_h),
                    );
                    painter.rect_filled(
                        row,
                        0.0,
                        if index % 2 == 0 {
                            Color32::from_rgb(18, 21, 26)
                        } else {
                            Color32::from_rgb(21, 25, 31)
                        },
                    );
                    painter.line_segment(
                        [row.left_bottom(), row.right_bottom()],
                        Stroke::new(1.0_f32, theme::BORDER),
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
                        egui::FontId::monospace(12.5),
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
                            egui::pos2(x_of(c.cut_start_ms), top + 5.0),
                            egui::pos2(
                                x_of(c.cut_end_ms()).max(x_of(c.cut_start_ms) + 4.0),
                                top + lane_h - 5.0,
                            ),
                        );
                        let color = if t.kind == TrackKind::Video {
                            Color32::from_rgb(59, 79, 107)
                        } else {
                            Color32::from_rgb(44, 82, 67)
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
                        let active = selected.contains(&c.id);
                        painter.rect_stroke(
                            block.shrink(0.5),
                            3.0,
                            Stroke::new(
                                if active { 1.8_f32 } else { 1.0 },
                                if active {
                                    theme::ACCENT
                                } else {
                                    color.gamma_multiply(1.4)
                                },
                            ),
                        );
                        if active {
                            painter.rect_filled(
                                egui::Rect::from_min_max(
                                    block.min,
                                    egui::pos2(block.left() + 3.0, block.bottom()),
                                ),
                                1.0,
                                theme::ACCENT,
                            );
                        }
                        let mut text_left = block.left() + 10.0;
                        if block.width() > 100.0 {
                            if let Some(texture) = images.get(&c.id) {
                                let thumb = egui::Rect::from_min_size(
                                    block.left_top() + egui::vec2(7.0, 7.0),
                                    egui::vec2(48.0, 27.0),
                                );
                                painter.image(
                                    texture.id(),
                                    thumb,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                                text_left = thumb.right() + 9.0;
                            }
                        }
                        if block.width() > 35.0 {
                            painter.with_clip_rect(block.shrink(6.0)).text(
                                egui::pos2(text_left, block.center().y),
                                Align2::LEFT_CENTER,
                                labels.get(&c.id).map(String::as_str).unwrap_or("Clip"),
                                egui::FontId::proportional(13.0),
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
                if let (Some(payload), Some(pos)) = (payload.as_ref(), hover) {
                    if let Some(place) = placement {
                        let y = rect.top()
                            + ruler
                            + if place.target.is_some() {
                                tracks
                                    .iter()
                                    .position(|t| Some(t.id) == place.target)
                                    .unwrap_or(0) as f32
                                    * lane_h
                            } else {
                                place.slot as f32 * lane_h
                            };
                        if place.target.is_none() {
                            let lane = egui::Rect::from_min_size(
                                egui::pos2(rect.left(), y),
                                egui::vec2(rect.width(), lane_h),
                            );
                            painter.rect_filled(lane, 2.0, Color32::from_rgb(26, 39, 56));
                            painter.rect_stroke(
                                lane.shrink(0.5),
                                2.0,
                                Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.6)),
                            );
                            painter.text(
                                egui::pos2(rect.left() + header * 0.5, y + lane_h * 0.5),
                                Align2::CENTER_CENTER,
                                if place.kind == TrackKind::Video {
                                    "+ V"
                                } else {
                                    "+ A"
                                },
                                egui::FontId::monospace(12.0),
                                theme::ACCENT,
                            );
                        }
                        let ghost = egui::Rect::from_min_max(
                            egui::pos2(x_of(place.start), y + 5.0),
                            egui::pos2(
                                x_of(place.start + place.length).max(x_of(place.start) + 12.0),
                                y + lane_h - 5.0,
                            ),
                        );
                        painter.rect_filled(ghost, 3.0, Color32::from_rgb(55, 91, 133));
                        painter.rect_stroke(
                            ghost.shrink(0.5),
                            3.0,
                            Stroke::new(2.0_f32, theme::ACCENT),
                        );
                        painter.with_clip_rect(ghost.shrink(6.0)).text(
                            ghost.left_center() + egui::vec2(10.0, 0.0),
                            Align2::LEFT_CENTER,
                            &place.label,
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                        if let Some(time) = place.snap {
                            let x = x_of(time);
                            painter.line_segment(
                                [egui::pos2(x, rect.top() + ruler), egui::pos2(x, y + lane_h)],
                                Stroke::new(1.5_f32, theme::ACCENT),
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
                        let slot = (((pos.y - rect.top() - ruler) / lane_h) + 0.5)
                            .floor()
                            .max(0.0) as usize;
                        let y = rect.top() + ruler + slot.min(tracks.len()) as f32 * lane_h;
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
        let (footer, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(footer);
        let mut x = footer.left();
        for (key, action) in [
            ("Space", "Play"),
            ("S", "Split"),
            ("Del", "Delete"),
            ("D", "Detach"),
            ("Shift+D", "Attach"),
            ("Ctrl+Z", "Undo"),
        ] {
            let key_text = painter.layout_no_wrap(
                key.into(),
                egui::FontId::monospace(10.5),
                theme::TEXT_PRIMARY,
            );
            let key_rect = egui::Rect::from_min_size(
                egui::pos2(x, footer.center().y - 9.0),
                key_text.size() + egui::vec2(8.0, 6.0),
            );
            painter.rect_filled(key_rect, 3.0, theme::BG_BUTTON);
            painter.galley(
                key_rect.min + egui::vec2(4.0, 3.0),
                key_text,
                theme::TEXT_PRIMARY,
            );
            let label = painter.layout_no_wrap(
                action.into(),
                egui::FontId::proportional(11.5),
                theme::TEXT_MUTED,
            );
            painter.galley(
                egui::pos2(
                    key_rect.right() + 5.0,
                    footer.center().y - label.size().y * 0.5,
                ),
                label.clone(),
                theme::TEXT_MUTED,
            );
            x = key_rect.right() + 5.0 + label.size().x + 16.0;
        }
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
            placement(&p, &[], &[], &drag, egui::pos2(x, 60.0), rect, &ctx)
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
            egui::pos2(64.0, RULER_HEIGHT + 10.0),
            rect,
            &ctx,
        )
        .unwrap();
        let below = placement(
            &p,
            &tracks,
            &clips,
            &drag,
            egui::pos2(64.0, RULER_HEIGHT + LANE_HEIGHT - 8.0),
            rect,
            &ctx,
        )
        .unwrap();
        assert_eq!((above.slot, above.target), (0, None));
        assert_eq!((below.slot, below.target), (1, None));
    }
    #[test]
    fn ruler_ticks_never_repeat_whole_second_labels() {
        assert_eq!(ruler_step(13388, 1800.0), 1000);
        assert!(ruler_step(2000, 1200.0) < 1000); // fractional labels at close scale
    }
}
