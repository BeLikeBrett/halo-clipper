use super::*;
use crate::track::{LayerSource, Transform};

impl Project {
    fn next_layer_name(&self, kind: TrackKind) -> String {
        let prefix = if kind == TrackKind::Video { 'V' } else { 'A' };
        let next = self
            .tracks
            .iter()
            .filter(|t| t.kind == kind)
            .filter_map(|t| t.name.strip_prefix(prefix)?.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        format!("{prefix}{next}")
    }
    fn add_layer(
        &mut self,
        source: LayerSource,
        kind: TrackKind,
        audio_index: Option<usize>,
        link: Option<LinkId>,
    ) -> ClipId {
        let id = self.id_gen.next_track();
        let clip = self.id_gen.next_clip();
        let t = Track {
            id,
            kind,
            name: self.next_layer_name(kind),
            muted: false,
            gain: 1.0,
            source_audio_index: audio_index,
            source: Some(source.clone()),
        };
        let index = if kind == TrackKind::Video {
            0
        } else {
            self.tracks.len()
        };
        self.tracks.insert(index, t);
        self.clips.push(Clip {
            id: clip,
            track_id: id,
            cut_start_ms: self.playhead_ms,
            src_start_ms: 0,
            src_end_ms: source.media.duration_ms,
            link,
            gain: 1.0,
            speed: 1.0,
            transform: Transform::default(),
        });
        clip
    }
    fn reorder_layer(&mut self, id: TrackId, up: bool) {
        let Some(index) = self.tracks.iter().position(|t| t.id == id) else {
            return;
        };
        let kind = self.tracks[index].kind;
        let neighbor = if up {
            (0..index).rev().find(|&i| self.tracks[i].kind == kind)
        } else {
            (index + 1..self.tracks.len()).find(|&i| self.tracks[i].kind == kind)
        };
        if let Some(other) = neighbor {
            self.tracks.swap(index, other);
        }
    }
    fn duplicate_layer_clip(&mut self, id: ClipId) {
        let Some(primary) = track::clip_by_id(&self.clips, id).copied() else {
            return;
        };
        let group: Vec<_> = self
            .clips
            .iter()
            .filter(|c| c.id == id || (primary.link.is_some() && c.link == primary.link))
            .copied()
            .collect();
        let new_link = primary.link.map(|_| self.id_gen.next_link());
        self.selected_clips.clear();
        for c in group {
            let Some(index) = self.tracks.iter().position(|t| t.id == c.track_id) else {
                continue;
            };
            let mut t = self.tracks[index].clone();
            t.id = self.id_gen.next_track();
            t.name = self.next_layer_name(t.kind);
            let mut copy = c;
            copy.id = self.id_gen.next_clip();
            copy.track_id = t.id;
            copy.link = new_link;
            self.tracks.insert(index, t);
            self.clips.push(copy);
            if c.id == id {
                self.selected_clips.insert(copy.id);
            }
        }
    }
}
impl ClipperApp {
    pub(super) fn import_layer(&mut self) {
        if !self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack)
        {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Video & audio",
                &[
                    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mp3", "wav", "flac", "ogg", "m4a",
                    "aac", "opus",
                ],
            )
            .pick_file()
        else {
            return;
        };
        self.add_layer_path(path);
    }
    /// Import into the current Advanced session, also used by --layer.
    pub fn add_layer_path(&mut self, path: PathBuf) {
        if self.project.is_none() {
            return;
        }
        self.set_mode(EditMode::MultiTrack);
        crate::diagnostics::register_media(&path);
        match media::probe_layer(&path) {
            Ok(info) => {
                self.push_undo();
                let p = self.project.as_mut().unwrap();
                let source = LayerSource {
                    path,
                    media: info.clone(),
                };
                let link =
                    (info.width > 0 && info.audio_stream_count > 0).then(|| p.id_gen.next_link());
                let mut selected = None;
                if info.width > 0 {
                    selected = Some(p.add_layer(source.clone(), TrackKind::Video, None, link));
                }
                for i in 0..info.audio_stream_count {
                    let id = p.add_layer(source.clone(), TrackKind::Audio, Some(i), link);
                    if selected.is_none() {
                        selected = Some(id);
                    }
                }
                p.selected_clips = selected.into_iter().collect();
                self.status = "Layer added at the playhead. Top video lane appears in front; audio lanes mix together.".into();
            }
            Err(e) => {
                self.status = format!("Could not import layer: {e:#}");
                log::error!("layer import: {e:#}");
            }
        }
    }
    pub(super) fn tick_layered_preview(&mut self, ctx: &egui::Context) {
        let active = self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack && p.needs_composition());
        if !active {
            if self.preview_is_composed {
                self.layered_preview.reset();
                self.preview_is_composed = false;
                if let Some(p) = &self.project {
                    let source_ms = self.playhead_to_src().unwrap_or(0);
                    let _ = self
                        .player
                        .set_file(p.proxy.as_deref().unwrap_or(&p.source), source_ms);
                }
            }
            return;
        }
        let composition = self.project.as_ref().unwrap().composition(true);
        if let Some(path) = self.layered_preview.tick(&composition) {
            let _ = self.player.set_lavfi_complex("");
            self.last_lavfi_complex.clear();
            let _ = self.player.set_speed(1.0);
            self.last_clip_speed = 1.0;
            let ms = self.project.as_ref().unwrap().playhead_ms;
            match self.player.set_file(&path, ms) {
                Ok(()) => self.preview_is_composed = true,
                Err(e) => {
                    self.layered_preview.ready = false;
                    self.status = format!("Could not load composed preview: {e:#}");
                }
            }
        }
        if !self.layered_preview.ready {
            let _ = self.player.set_paused(true);
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
    pub(super) fn show_layers(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self
            .project
            .as_ref()
            .filter(|p| p.mode == EditMode::MultiTrack)
        else {
            return;
        };
        let tracks = p.tracks.clone();
        ui.label(RichText::new("Layers").strong().color(theme::TEXT_PRIMARY));
        ui.label(
            RichText::new("Top video is in front · audio mixes")
                .small()
                .color(theme::TEXT_MUTED),
        );
        if secondary_button(ui, "Add video / audio…", true).clicked() {
            self.import_layer();
        }
        let mut reorder = None;
        let mut mute = None;
        let mut select = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("layer_list")
            .max_height(300.0)
            .show(ui, |ui| {
                for t in &tracks {
                    ui.push_id(t.id, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(if t.muted { "Off" } else { "On" })
                                .on_hover_text(if t.kind == TrackKind::Video {
                                    "Show / hide video layer"
                                } else {
                                    "Mute / unmute audio layer"
                                })
                                .clicked()
                            {
                                mute = Some(t.id);
                            }
                            if ui
                                .selectable_label(
                                    self.project.as_ref().unwrap().clips.iter().any(|c| {
                                        c.track_id == t.id
                                            && self
                                                .project
                                                .as_ref()
                                                .unwrap()
                                                .selected_clips
                                                .contains(&c.id)
                                    }),
                                    &t.name,
                                )
                                .on_hover_text(
                                    t.source
                                        .as_ref()
                                        .map(|s| {
                                            s.path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned()
                                        })
                                        .unwrap_or_else(|| "Original source".into()),
                                )
                                .clicked()
                            {
                                select = Some(t.id);
                            }
                            let same: Vec<_> =
                                tracks.iter().filter(|other| other.kind == t.kind).collect();
                            if ui
                                .add_enabled(
                                    same.first().is_some_and(|x| x.id != t.id),
                                    egui::Button::new("Up").small(),
                                )
                                .on_hover_text("Move layer above")
                                .clicked()
                            {
                                reorder = Some((t.id, true));
                            }
                            if ui
                                .add_enabled(
                                    same.last().is_some_and(|x| x.id != t.id),
                                    egui::Button::new("Down").small(),
                                )
                                .on_hover_text("Move layer below")
                                .clicked()
                            {
                                reorder = Some((t.id, false));
                            }
                            if !self
                                .project
                                .as_ref()
                                .unwrap()
                                .clips
                                .iter()
                                .any(|c| c.track_id == t.id)
                                && ui
                                    .small_button("×")
                                    .on_hover_text("Remove empty layer")
                                    .clicked()
                            {
                                remove = Some(t.id);
                            }
                        });
                    });
                }
            });
        if reorder.is_some() || mute.is_some() || remove.is_some() {
            self.push_undo();
        }
        if let Some(p) = &mut self.project {
            if let Some((id, up)) = reorder {
                p.reorder_layer(id, up);
            }
            if let Some(id) = mute {
                if let Some(t) = p.tracks.iter_mut().find(|t| t.id == id) {
                    t.muted = !t.muted;
                }
            }
            if let Some(id) = remove {
                p.tracks.retain(|t| t.id != id);
            }
            if let Some(id) = select {
                p.selected_clips = p
                    .clips
                    .iter()
                    .filter(|c| c.track_id == id)
                    .min_by_key(|c| c.cut_start_ms)
                    .map(|c| c.id)
                    .into_iter()
                    .collect();
            }
        }
        ui.separator();
        ui.add_space(8.0);
    }
    pub(super) fn show_layer_properties(&mut self, ui: &mut egui::Ui) {
        if !ui.input(|i| i.pointer.primary_down()) {
            self.layer_edit_active = false;
        }
        let Some(p) = self.project.as_ref() else {
            return;
        };
        let Some(c) = p
            .selected_clips
            .iter()
            .min()
            .and_then(|id| track::clip_by_id(&p.clips, *id))
            .copied()
        else {
            return;
        };
        let Some(t) = p.tracks.iter().find(|t| t.id == c.track_id).cloned() else {
            return;
        };
        let source_duration = t
            .source
            .as_ref()
            .map(|s| s.media.duration_ms)
            .unwrap_or(p.media.duration_ms) as f64
            / 1000.0;
        ui.label(RichText::new(format!("{} · layer", t.name)).strong());
        if let Some(source) = &t.source {
            ui.label(
                source
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            );
        }
        ui.horizontal(|ui| {
            if ui.button("Above").clicked() {
                self.push_undo();
                self.project.as_mut().unwrap().reorder_layer(t.id, true);
            }
            if ui.button("Below").clicked() {
                self.push_undo();
                self.project.as_mut().unwrap().reorder_layer(t.id, false);
            }
            if ui
                .button("Duplicate")
                .on_hover_text("Copy this segment and linked audio to new layers above")
                .clicked()
            {
                self.push_undo();
                self.project.as_mut().unwrap().duplicate_layer_clip(c.id);
            }
        });
        let mut start = c.cut_start_ms as f64 / 1000.0;
        let mut src_start = c.src_start_ms as f64 / 1000.0;
        let mut src_end = c.src_end_ms as f64 / 1000.0;
        let mut changed = false;
        changed |= ui
            .add(
                egui::DragValue::new(&mut start)
                    .range(0.0..=86400.0)
                    .speed(0.05)
                    .prefix("Start  ")
                    .suffix(" s"),
            )
            .changed();
        changed |= ui
            .add(
                egui::DragValue::new(&mut src_start)
                    .range(0.0..=(src_end - 0.001).max(0.0))
                    .speed(0.05)
                    .prefix("Source in  ")
                    .suffix(" s"),
            )
            .changed();
        changed |= ui
            .add(
                egui::DragValue::new(&mut src_end)
                    .range((src_start + 0.001)..=source_duration)
                    .speed(0.05)
                    .prefix("Source out  ")
                    .suffix(" s"),
            )
            .changed();
        if changed {
            if !self.layer_edit_active {
                self.push_undo();
            }
            self.layer_edit_active = ui.input(|i| i.pointer.primary_down());
            if let Some(p) = &mut self.project {
                for clip in &mut p.clips {
                    if clip.id == c.id || (c.link.is_some() && clip.link == c.link) {
                        clip.cut_start_ms = (start * 1000.0).round() as i64;
                        clip.src_start_ms = (src_start * 1000.0).round() as i64;
                        clip.src_end_ms = (src_end * 1000.0).round() as i64;
                    }
                }
            }
        }
        if t.kind == TrackKind::Video {
            let mut tr = c.transform;
            let mut change = false;
            change |= ui
                .add(egui::Slider::new(&mut tr.opacity, 0.0..=1.0).text("Video opacity"))
                .changed();
            change |= ui
                .add(egui::Slider::new(&mut tr.scale, 0.1..=2.0).text("Video size"))
                .changed();
            change |= ui
                .add(egui::Slider::new(&mut tr.x, 0.0..=1.0).text("Horizontal"))
                .changed();
            change |= ui
                .add(egui::Slider::new(&mut tr.y, 0.0..=1.0).text("Vertical"))
                .changed();
            if ui.button("Reset video placement").clicked() {
                tr = Transform::default();
                change = tr != c.transform;
            }
            if change {
                if !self.layer_edit_active {
                    self.push_undo();
                }
                self.layer_edit_active = ui.input(|i| i.pointer.primary_down());
                if let Some(clip) = self
                    .project
                    .as_mut()
                    .unwrap()
                    .clips
                    .iter_mut()
                    .find(|clip| clip.id == c.id)
                {
                    clip.transform = tr;
                }
            }
        }
        ui.separator();
        ui.add_space(8.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project() -> Project {
        let mut p = Project {
            source: PathBuf::from("base.mp4"),
            proxy: None,
            media: MediaInfo {
                duration_ms: 5000,
                width: 64,
                height: 64,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: Some("aac".into()),
                audio_stream_count: 1,
            },
            playhead_ms: 1000,
            in_ms: None,
            out_ms: None,
            simple_audio_muted: false,
            tracks: Vec::new(),
            clips: Vec::new(),
            selected_clips: HashSet::new(),
            text_overlays: HashMap::new(),
            id_gen: IdGen::default(),
            mode: EditMode::MultiTrack,
        };
        p.ensure_tracks_initialized();
        p
    }
    #[test]
    fn import_order_source_and_undo_are_preserved() {
        let mut p = project();
        let old = UndoSnapshot::capture(&p);
        let id = p.add_layer(
            LayerSource {
                path: PathBuf::from("overlay.mp4"),
                media: p.media.clone(),
            },
            TrackKind::Video,
            None,
            None,
        );
        assert_eq!(
            p.tracks[0].id,
            track::clip_by_id(&p.clips, id).unwrap().track_id
        );
        let composition = p.composition(false);
        assert_eq!(
            composition
                .segments
                .iter()
                .filter(|s| s.audio_index.is_none())
                .next_back()
                .unwrap()
                .source,
            PathBuf::from("overlay.mp4")
        );
        let imported = UndoSnapshot::capture(&p);
        old.apply(&mut p);
        assert!(!p.needs_composition());
        imported.apply(&mut p);
        assert!(p.needs_composition());
        let top = p.tracks[0].id;
        p.reorder_layer(top, false);
        assert_eq!(
            p.composition(false)
                .segments
                .iter()
                .filter(|s| s.audio_index.is_none())
                .next_back()
                .unwrap()
                .source,
            PathBuf::from("base.mp4")
        );
    }
    #[test]
    fn duplicate_creates_independent_linked_layers() {
        let mut p = project();
        let id = p.clips[0].id;
        p.duplicate_layer_clip(id);
        assert_eq!(p.tracks.len(), 4);
        assert_eq!(p.clips.len(), 4);
        let copy = *track::clip_by_id(&p.clips, *p.selected_clips.iter().next().unwrap()).unwrap();
        assert_ne!(copy.link, p.clips[0].link);
        assert_ne!(copy.track_id, p.clips[0].track_id);
        assert_eq!(p.clips.iter().filter(|c| c.link == copy.link).count(), 2);
        track::delete_with_links(&mut p.clips, copy.id);
        assert_eq!(p.clips.len(), 2);
    }
    #[test]
    fn speed_split_preserves_timeline_and_transform() {
        let mut p = project();
        let id = p.clips[0].id;
        p.clips[0].speed = 2.0;
        p.clips[0].transform.opacity = 0.5;
        assert_eq!(p.clips[0].cut_end_ms(), 2500);
        let right = p.split_segments(&[id], 1000)[0];
        let left = track::clip_by_id(&p.clips, id).unwrap();
        let right = track::clip_by_id(&p.clips, right).unwrap();
        assert_eq!(left.src_end_ms, 2000);
        assert_eq!(left.cut_end_ms(), 1000);
        assert_eq!(right.cut_end_ms(), 2500);
        assert_eq!(right.transform.opacity, 0.5);
    }
    #[test]
    fn hidden_video_and_muted_audio_do_not_enter_composition() {
        let mut p = project();
        p.tracks[0].muted = true;
        let comp = p.composition(false);
        assert!(comp.segments.iter().all(|s| s.audio_index.is_some()));
        p.tracks[1].muted = true;
        assert!(p.composition(false).segments.is_empty());
        assert_eq!(p.composition(false).duration_ms, 5000);
    }
}
