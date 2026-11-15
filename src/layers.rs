use super::*;
use crate::track::{LayerSource, Transform};

impl Project {
    pub(super) fn empty() -> Self {
        Self {
            source: PathBuf::new(),
            proxy: None,
            media: Self::blank_media(),
            playhead_ms: 0,
            in_ms: None,
            out_ms: None,
            simple: Default::default(),
            simple_audio_muted: false,
            assets: Vec::new(),
            canvas: Default::default(),
            groups: HashMap::new(),
            tracks: Vec::new(),
            clips: Vec::new(),
            selected_clips: HashSet::new(),
            texts: Vec::new(),
            selected_text: None,
            id_gen: IdGen::default(),
            mode: EditMode::MultiTrack,
        }
    }
    pub(super) fn blank_media() -> MediaInfo {
        MediaInfo {
            duration_ms: 0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            video_codec: String::new(),
            audio_codec: None,
            audio_stream_count: 0,
        }
    }
    pub(super) fn clip_source(&self, c: &Clip) -> (&std::path::Path, &MediaInfo) {
        if let Some(asset) = c
            .asset_id
            .and_then(|id| self.assets.iter().find(|a| a.id == id))
        {
            return (&asset.source.path, &asset.source.media);
        }
        if let Some(source) = self
            .tracks
            .iter()
            .find(|t| t.id == c.track_id)
            .and_then(|t| t.source.as_ref())
        {
            return (&source.path, &source.media);
        }
        (&self.source, &self.media)
    }
    pub(super) fn canvas_media(&self) -> MediaInfo {
        if self.mode == EditMode::SimpleClip {
            return self.media.clone();
        }
        let mut media = self
            .clips
            .iter()
            .filter(|c| {
                self.tracks
                    .iter()
                    .any(|t| t.id == c.track_id && t.kind == TrackKind::Video)
            })
            .map(|c| self.clip_source(c).1)
            .filter(|m| m.width > 0 && m.height > 0)
            .max_by_key(|m| {
                (
                    m.width as u64 * m.height as u64,
                    m.width,
                    m.height,
                    (m.fps * 1000.0) as u64,
                )
            })
            .cloned()
            .unwrap_or_else(Self::blank_media);
        (media.width, media.height) = self.canvas.aspect.dimensions(media.width, media.height);
        media
    }
    pub(super) fn import_asset(&mut self, source: LayerSource) -> TrackId {
        if let Some(a) = self.assets.iter().find(|a| a.source.path == source.path) {
            return a.id;
        }
        let id = self.id_gen.next_track();
        if self.source.as_os_str().is_empty() {
            self.source = source.path.clone();
            self.media = source.media.clone();
        }
        self.assets.push(MediaAsset {
            id,
            import_number: self.id_gen.next_import_number(),
            source,
        });
        id
    }
    pub(super) fn remove_asset(&mut self, id: TrackId) {
        let Some(asset) = self.assets.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let legacy_tracks: HashSet<_> = self
            .tracks
            .iter()
            .filter(|t| {
                t.source
                    .as_ref()
                    .is_some_and(|s| s.path == asset.source.path)
            })
            .map(|t| t.id)
            .collect();
        let removed: HashSet<_> = self
            .clips
            .iter()
            .filter(|c| {
                c.asset_id == Some(id)
                    || legacy_tracks.contains(&c.track_id)
                    || (c.asset_id.is_none() && self.source == asset.source.path)
            })
            .map(|c| c.id)
            .collect();
        self.clips.retain(|c| !removed.contains(&c.id));
        self.assets.retain(|a| a.id != id);
        self.simple.edits.remove(&id);
        if self.simple.focus == Some(id) {
            self.simple.focus = None;
        }
        self.prune_tracks();
        self.selected_clips.retain(|id| !removed.contains(id));
        if self.source == asset.source.path {
            self.proxy = None;
            if let Some(a) = self.assets.first() {
                self.source = a.source.path.clone();
                self.media = a.source.media.clone();
            } else {
                self.source = PathBuf::new();
                self.media = Self::blank_media();
                self.playhead_ms = 0;
                self.proxy = None;
            }
        }
        if self.mode == EditMode::SimpleClip
            && !self.assets.iter().any(|a| a.source.media.width > 0)
        {
            self.source = PathBuf::new();
            self.proxy = None;
            self.media = Self::blank_media();
            self.playhead_ms = 0;
            self.in_ms = None;
            self.out_ms = None;
            self.simple_audio_muted = false;
            self.simple.focus = None;
        }
    }
    pub(super) fn next_layer_name(&self, kind: TrackKind) -> String {
        let prefix = match kind {
            TrackKind::Video => 'V',
            TrackKind::Audio => 'A',
            TrackKind::Text => 'T',
        };
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
    pub(super) fn new_track(&mut self, kind: TrackKind, index: usize) -> TrackId {
        let id = self.id_gen.next_track();
        let track = Track {
            id,
            kind,
            name: self.next_layer_name(kind),
            muted: false,
            gain: 1.0,
            source_audio_index: None,
            source: None,
        };
        let video_count = self.tracks.iter().filter(|t| t.kind.is_visual()).count();
        let index = if kind.is_visual() {
            index.min(video_count)
        } else {
            index.max(video_count).min(self.tracks.len())
        };
        self.tracks.insert(index, track);
        id
    }
    pub(super) fn available_lane(
        &self,
        kind: TrackKind,
        target: Option<TrackId>,
        start: i64,
        end: i64,
    ) -> Option<TrackId> {
        target
            .filter(|id| self.tracks.iter().any(|t| t.id == *id && t.kind == kind))
            .filter(|id| {
                !self
                    .clips
                    .iter()
                    .any(|c| c.track_id == *id && start < c.cut_end_ms() && end > c.cut_start_ms)
            })
    }
    pub(super) fn place_asset(
        &mut self,
        id: TrackId,
        start: i64,
        target: Option<TrackId>,
        index: usize,
    ) -> Option<ClipId> {
        let asset = self.assets.iter().find(|a| a.id == id)?.clone();
        let start = start.max(0);
        let end = start + asset.source.media.duration_ms;
        let has_video = asset.source.media.width > 0;
        let link = has_video.then(|| self.id_gen.next_link());
        let mut selected = None;
        if has_video {
            let lane = self
                .available_lane(TrackKind::Video, target, start, end)
                .unwrap_or_else(|| self.new_track(TrackKind::Video, index));
            let clip = Clip {
                id: self.id_gen.next_clip(),
                track_id: lane,
                cut_start_ms: start,
                src_start_ms: 0,
                src_end_ms: asset.source.media.duration_ms,
                link,
                gain: 1.0,
                speed: 1.0,
                transform: Transform::default(),
                animation: Default::default(),
                asset_id: Some(id),
                audio_stream: None,
            };
            selected = Some(clip.id);
            self.clips.push(clip);
        }
        for stream in 0..asset.source.media.audio_stream_count {
            let target = if has_video || stream > 0 {
                None
            } else {
                target
            };
            let lane = self
                .available_lane(TrackKind::Audio, target, start, end)
                .unwrap_or_else(|| self.new_track(TrackKind::Audio, self.tracks.len()));
            let clip = Clip {
                id: self.id_gen.next_clip(),
                track_id: lane,
                cut_start_ms: start,
                src_start_ms: 0,
                src_end_ms: asset.source.media.duration_ms,
                link,
                gain: 1.0,
                speed: 1.0,
                transform: Transform::default(),
                animation: Default::default(),
                asset_id: Some(id),
                audio_stream: Some(stream),
            };
            if selected.is_none() {
                selected = Some(clip.id);
            }
            self.clips.push(clip);
        }
        if let Some(link) = link {
            let members = self
                .clips
                .iter()
                .filter(|c| c.link == Some(link))
                .map(|c| c.id)
                .collect();
            self.groups.insert(
                link,
                grouping::MediaGroup {
                    members,
                    attached: true,
                },
            );
        }
        self.selected_clips = selected.into_iter().collect();
        selected
    }
    pub(super) fn prune_tracks(&mut self) {
        for g in self.groups.values_mut() {
            g.members
                .retain(|id| self.clips.iter().any(|c| c.id == *id));
        }
        self.groups.retain(|_, g| !g.members.is_empty());
        self.tracks.retain(|t| {
            self.clips.iter().any(|c| c.track_id == t.id)
                || self.texts.iter().any(|x| x.track == t.id)
        });
    }
    pub(super) fn reorder_track(&mut self, id: TrackId, index: usize) {
        let Some(old) = self.tracks.iter().position(|t| t.id == id) else {
            return;
        };
        let track = self.tracks.remove(old);
        let index = if index > old { index - 1 } else { index };
        let videos = self.tracks.iter().filter(|t| t.kind.is_visual()).count();
        let index = if track.kind.is_visual() {
            index.min(videos)
        } else {
            index.max(videos).min(self.tracks.len())
        };
        self.tracks.insert(index, track);
    }
    pub(super) fn move_segment(
        &mut self,
        id: ClipId,
        start: i64,
        target: Option<TrackId>,
        index: usize,
    ) {
        let Some(primary) = track::clip_by_id(&self.clips, id).copied() else {
            return;
        };
        let Some(kind) = self
            .tracks
            .iter()
            .find(|t| t.id == primary.track_id)
            .map(|t| t.kind)
        else {
            return;
        };
        let group = self.edit_ids(id);
        let lower = self
            .clips
            .iter()
            .filter(|c| group.contains(&c.id))
            .map(|c| c.cut_start_ms)
            .min()
            .unwrap_or(0);
        let delta = (start - self.group_bounds(id).0).max(-lower);
        let start = primary.cut_start_ms + delta;
        let lane = target
            .filter(|tid| self.tracks.iter().any(|t| t.id == *tid && t.kind == kind))
            .filter(|tid| {
                !self.clips.iter().any(|c| {
                    c.track_id == *tid
                        && !group.contains(&c.id)
                        && start < c.cut_end_ms()
                        && start + primary.duration_ms() > c.cut_start_ms
                })
            })
            .unwrap_or_else(|| self.new_track(kind, index));
        for c in &mut self.clips {
            if group.contains(&c.id) {
                c.cut_start_ms += delta;
            }
            if c.id == id {
                c.track_id = lane;
            }
        }
        self.prune_tracks();
        self.selected_clips = [id].into_iter().collect();
    }
}
impl ClipperApp {
    /// OS file-manager drops stage assets in Media in either editing mode.
    /// Keep the current Simple Clip source/marks and the Advanced timeline.
    pub(super) fn import_dropped_media(&mut self, paths: Vec<PathBuf>) {
        let mut imported = 0;
        let mut already_present = 0;
        let mut errors = Vec::new();
        let mut saved_undo = false;
        for path in paths {
            if self
                .project
                .as_ref()
                .is_some_and(|p| p.assets.iter().any(|a| a.source.path == path))
            {
                already_present += 1;
                continue;
            }
            crate::diagnostics::register_media(&path);
            let info = match media::probe_layer(&path) {
                Ok(info) if path.is_file() => info,
                Ok(_) => {
                    errors.push("Drop video or audio files, rather than folders.".to_string());
                    continue;
                }
                Err(e) => {
                    errors.push(format!("Could not import: {e:#}"));
                    continue;
                }
            };
            // A first video opens a Simple Clip workspace. Later files only
            // enter Media, so dropping a batch never cycles the active source.
            let needs_source = self
                .project
                .as_ref()
                .is_none_or(|p| p.mode == EditMode::SimpleClip && p.simple.focus.is_none());
            if needs_source && info.width > 0 {
                self.open_path(path);
            } else {
                if self.project.is_none() {
                    self.project = Some(Project::empty());
                }
                let advanced = self
                    .project
                    .as_ref()
                    .is_some_and(|p| p.mode == EditMode::MultiTrack);
                if advanced && !saved_undo {
                    self.push_undo();
                    saved_undo = true;
                }
                self.project
                    .as_mut()
                    .unwrap()
                    .import_asset(LayerSource { path, media: info });
            }
            imported += 1;
        }
        self.status = if errors.is_empty() {
            if imported == 0 {
                format!("{already_present} already in Media.")
            } else {
                format!("Imported {imported} into Media.")
            }
        } else {
            format!(
                "Imported {imported}. {} file(s) skipped. {}",
                errors.len(),
                errors[0]
            )
        };
    }

    /// Files dropped onto the Advanced timeline: imported (or reused from
    /// Media) and placed one after another from `start` on a new row at
    /// view `index`. One undo step.
    pub(super) fn drop_on_timeline(&mut self, paths: Vec<PathBuf>, start: i64, index: usize) {
        let mut errors = Vec::new();
        let mut placed = 0;
        let mut at = start;
        self.push_undo();
        for path in paths {
            crate::diagnostics::register_media(&path);
            let info = match media::probe_layer(&path) {
                Ok(info) if path.is_file() => info,
                Ok(_) => {
                    errors.push("Drop video or audio files, rather than folders.".to_string());
                    continue;
                }
                Err(e) => {
                    errors.push(format!("Could not import: {e:#}"));
                    continue;
                }
            };
            let Some(p) = &mut self.project else { return };
            let length = info.duration_ms;
            let id = p.import_asset(LayerSource { path, media: info });
            p.place_asset(id, at, None, index);
            at += length;
            placed += 1;
        }
        if placed == 0 {
            self.undo_stack.pop();
        } else {
            self.live.pause();
        }
        self.status = match (placed, errors.first()) {
            (_, None) => format!("Added {placed} to the timeline."),
            (_, Some(e)) => format!("Added {placed}. {} file(s) skipped. {e}", errors.len()),
        };
    }

    pub(super) fn import_layer(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Video & audio",
                &[
                    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mp3", "wav", "flac", "ogg", "m4a",
                    "aac", "opus",
                ],
            )
            .pick_files()
        else {
            return;
        };
        for path in paths {
            self.add_layer_path(path);
        }
    }
    pub fn add_layer_path(&mut self, path: PathBuf) {
        if self.project.is_none() {
            self.project = Some(Project::empty());
        }
        self.set_mode(EditMode::MultiTrack);
        crate::diagnostics::register_media(&path);
        match media::probe_layer(&path) {
            Ok(info) => {
                self.push_undo();
                self.project
                    .as_mut()
                    .unwrap()
                    .import_asset(LayerSource { path, media: info });
                self.status = "Imported. Drag media onto the timeline.".into();
            }
            Err(e) => {
                self.status = format!("Could not import: {e:#}");
                log::error!("media import: {e:#}");
            }
        }
    }
    pub(super) fn tick_live_preview(&mut self, ctx: &egui::Context) {
        if !self
            .project
            .as_ref()
            .is_some_and(|p| p.mode == EditMode::MultiTrack)
        {
            self.live.clear();
            return;
        }
        let p = self.project.as_mut().unwrap();
        if self.live.playing {
            if self.user_dragging_seek || self.was_dragging_seek {
                self.live.reanchor(p.playhead_ms);
            } else {
                p.playhead_ms = self.live.position().min(p.content_end_ms());
                if p.playhead_ms >= p.content_end_ms() {
                    self.live.pause();
                }
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if !self.player.state().paused {
            let _ = self.player.set_paused(true);
        }
        self.live
            .sync(p, ctx, self.volume_ui, self.player.state().muted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn asset(path: &str, w: u32, h: u32, duration: i64, audio: usize) -> LayerSource {
        LayerSource {
            path: PathBuf::from(path),
            media: MediaInfo {
                duration_ms: duration,
                width: w,
                height: h,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: None,
                audio_stream_count: audio,
            },
        }
    }
    #[test]
    fn import_numbers_are_stable_across_duplicate_paths_tracks_and_removal() {
        let mut p = Project::empty();
        let first = p.import_asset(asset("first.mp4", 640, 360, 1000, 0));
        p.id_gen.next_track();
        let second = p.import_asset(asset("second.mp4", 640, 360, 1000, 0));
        assert_eq!(p.assets[0].import_number, 1);
        assert_eq!(p.assets[1].import_number, 2);
        assert_eq!(p.import_asset(asset("first.mp4", 640, 360, 1000, 0)), first);
        p.remove_asset(first);
        p.import_asset(asset("third.mp4", 640, 360, 1000, 0));
        assert_eq!(
            p.assets.iter().find(|a| a.id == second).unwrap().label(),
            "2 · second.mp4"
        );
        assert_eq!(p.assets.last().unwrap().import_number, 3);
    }

    #[test]
    fn import_is_bin_only_and_last_removal_leaves_blank() {
        let mut p = Project::empty();
        let id = p.import_asset(asset("one.mp4", 1920, 1080, 5000, 1));
        assert!(p.tracks.is_empty() && p.clips.is_empty());
        p.place_asset(id, 0, None, 0);
        p.remove_asset(id);
        assert!(p.assets.is_empty() && p.clips.is_empty() && p.tracks.is_empty());
        let id = p.import_asset(asset("next.mp4", 1280, 720, 2000, 0));
        assert!(p.clips.is_empty());
        assert!(p.place_asset(id, 0, None, 0).is_some());
    }
    #[test]
    fn canvas_follows_placed_resolution_not_imports() {
        let mut p = Project::empty();
        let small = p.import_asset(asset("small.mp4", 1280, 720, 5000, 1));
        let large = p.import_asset(asset("large.mp4", 3840, 2160, 3000, 0));
        p.place_asset(small, 0, None, 0);
        assert_eq!(p.canvas_media().width, 1280);
        p.place_asset(large, 1000, None, 0);
        assert_eq!(p.canvas_media().width, 3840);
        p.remove_asset(large);
        assert_eq!(p.canvas_media().width, 1280);
    }
    #[test]
    fn moving_across_lanes_retains_per_clip_sources_and_audio() {
        let mut p = Project::empty();
        let a = p.import_asset(asset("a.mp4", 1280, 720, 1000, 1));
        let b = p.import_asset(asset("b.mp4", 1920, 1080, 1000, 0));
        let first = p.place_asset(a, 0, None, 0).unwrap();
        let second = p.place_asset(b, 2000, None, 0).unwrap();
        let lane = track::clip_by_id(&p.clips, second).unwrap().track_id;
        p.move_segment(first, 4000, Some(lane), 0);
        let first = track::clip_by_id(&p.clips, first).unwrap();
        assert_eq!(p.clip_source(first).0, std::path::Path::new("a.mp4"));
        assert_eq!(first.track_id, lane);
        assert_eq!(
            p.clips
                .iter()
                .find(|c| c.audio_stream == Some(0))
                .unwrap()
                .cut_start_ms,
            4000
        );
    }
    #[test]
    fn removal_and_placement_are_undoable() {
        let mut p = Project::empty();
        let id = p.import_asset(asset("a.mp4", 1920, 1080, 2000, 1));
        let before = UndoSnapshot::capture(&p);
        p.place_asset(id, 500, None, 0);
        let after = UndoSnapshot::capture(&p);
        before.apply(&mut p);
        assert!(p.clips.is_empty());
        after.apply(&mut p);
        assert_eq!(p.clips.len(), 2);
        p.remove_asset(id);
        assert!(p.clips.is_empty());
    }
}
