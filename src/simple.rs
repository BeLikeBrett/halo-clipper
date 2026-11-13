//! Simple Clip keeps independent source marks and histories; Advanced owns its timeline.
use super::*;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SourceEdit {
    pub position: i64,
    pub in_ms: Option<i64>,
    pub out_ms: Option<i64>,
    pub muted: bool,
}
#[derive(Default)]
pub(super) struct Workspace {
    pub focus: Option<TrackId>,
    pub edits: HashMap<TrackId, SourceEdit>,
    pub advanced_position: i64,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum HistoryKey {
    Advanced,
    Simple(Option<TrackId>),
}
impl Project {
    pub(super) fn history_key(&self) -> HistoryKey {
        if self.mode == EditMode::MultiTrack {
            HistoryKey::Advanced
        } else {
            HistoryKey::Simple(self.simple.focus)
        }
    }
    pub(super) fn remember_simple(&mut self) {
        if let Some(id) = self.simple.focus {
            self.simple.edits.insert(
                id,
                SourceEdit {
                    position: self.playhead_ms,
                    in_ms: self.in_ms,
                    out_ms: self.out_ms,
                    muted: self.simple_audio_muted,
                },
            );
        }
    }
    pub(super) fn focus_simple(&mut self, id: TrackId) -> bool {
        let Some(asset) = self
            .assets
            .iter()
            .find(|a| a.id == id && a.source.media.width > 0)
            .cloned()
        else {
            return false;
        };
        if self.mode == EditMode::SimpleClip {
            self.remember_simple();
        }
        self.simple.focus = Some(id);
        let edit = self.simple.edits.get(&id).copied().unwrap_or_default();
        self.source = asset.source.path;
        self.media = asset.source.media;
        self.proxy = None;
        self.in_ms = edit.in_ms;
        self.out_ms = edit.out_ms;
        self.simple_audio_muted = edit.muted;
        self.playhead_ms = edit.position.min(self.media.duration_ms);
        true
    }
}
impl ClipperApp {
    pub(super) fn stash_history(&mut self) {
        if let Some(p) = &self.project {
            self.histories.insert(
                p.history_key(),
                (
                    std::mem::take(&mut self.undo_stack),
                    std::mem::take(&mut self.redo_stack),
                ),
            );
        }
    }
    pub(super) fn restore_history(&mut self) {
        let history = self
            .project
            .as_ref()
            .and_then(|p| self.histories.remove(&p.history_key()))
            .unwrap_or_default();
        (self.undo_stack, self.redo_stack) = history;
    }
    pub(super) fn select_simple_media(&mut self, id: TrackId) {
        if !self.project.as_ref().is_some_and(|p| {
            p.mode == EditMode::SimpleClip
                && p.simple.focus != Some(id)
                && p.assets
                    .iter()
                    .any(|a| a.id == id && a.source.media.width > 0)
        }) {
            return;
        }
        self.stash_history();
        self.project.as_mut().unwrap().focus_simple(id);
        self.restore_history();
        self.load_simple_preview();
    }
    pub(super) fn load_simple_preview(&mut self) {
        if let Some(h) = self.proxy_handle.take() {
            h.cancel();
        }
        let Some(p) = &mut self.project else { return };
        if p.source.as_os_str().is_empty() {
            let _ = self.player.set_paused(true);
            return;
        }
        let quality = self.settings.proxy_quality;
        p.proxy = if proxy::proxy_exists(&p.source, quality) {
            proxy::proxy_path_for(&p.source, quality)
        } else {
            None
        };
        let _ = self.player.set_paused(true);
        if let Err(e) = self
            .player
            .set_file(p.proxy.as_deref().unwrap_or(&p.source), p.playhead_ms)
        {
            self.status = format!("Could not load video: {e:#}");
            return;
        }
        self.status = format!(
            "Clipping {}",
            p.source.file_name().unwrap_or_default().to_string_lossy()
        );
        let source = p.source.clone();
        let duration = p.media.duration_ms;
        let needs_proxy = p.proxy.is_none();
        if needs_proxy {
            match proxy::spawn_transcode(&source, duration, self.proxy_can_use_vaapi(), quality) {
                Ok(h) => self.proxy_handle = Some(h),
                Err(e) => log::warn!("proxy spawn failed: {e:#}"),
            }
        }
        self.seek_cooldown_until = Some(std::time::Instant::now() + Duration::from_millis(250));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn source(p: &mut Project, name: &str) -> TrackId {
        p.import_asset(track::LayerSource {
            path: name.into(),
            media: MediaInfo {
                duration_ms: 8000,
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
    fn switching_sources_remembers_marks_and_does_not_touch_advanced_layers() {
        let mut p = Project::empty();
        let a = source(&mut p, "a.mp4");
        let b = source(&mut p, "b.mp4");
        p.place_asset(a, 1200, None, 0);
        let composition = format!("{:?}", p.composition(false));
        p.mode = EditMode::SimpleClip;
        p.focus_simple(a);
        p.in_ms = Some(500);
        p.out_ms = Some(2500);
        p.playhead_ms = 1400;
        p.simple_audio_muted = true;
        p.focus_simple(b);
        assert_eq!(p.in_ms, None);
        p.in_ms = Some(3000);
        p.out_ms = Some(7000);
        p.focus_simple(a);
        assert_eq!(
            (p.in_ms, p.out_ms, p.playhead_ms, p.simple_audio_muted),
            (Some(500), Some(2500), 1400, true)
        );
        p.focus_simple(b);
        assert_eq!((p.in_ms, p.out_ms), (Some(3000), Some(7000)));
        p.mode = EditMode::MultiTrack;
        assert_eq!(format!("{:?}", p.composition(false)), composition);
    }
    #[test]
    fn removing_last_video_clears_simple_marks_even_with_audio_imports_remaining() {
        let mut p = Project::empty();
        let a = source(&mut p, "a.mp4");
        p.import_asset(track::LayerSource {
            path: "music.wav".into(),
            media: MediaInfo {
                width: 0,
                height: 0,
                ..p.media.clone()
            },
        });
        p.mode = EditMode::SimpleClip;
        p.focus_simple(a);
        p.in_ms = Some(1000);
        p.out_ms = Some(3000);
        p.remove_asset(a);
        assert!(p.source.as_os_str().is_empty());
        assert_eq!((p.in_ms, p.out_ms), (None, None));
        assert_eq!(p.simple.focus, None);
        let next = source(&mut p, "next.mp4");
        p.focus_simple(next);
        assert_eq!((p.in_ms, p.out_ms), (None, None));
    }
    #[test]
    fn simple_undo_restores_marks_without_reverting_advanced_work() {
        let mut p = Project::empty();
        let a = source(&mut p, "a.mp4");
        p.mode = EditMode::SimpleClip;
        p.focus_simple(a);
        p.in_ms = Some(100);
        let undo = UndoSnapshot::capture(&p);
        p.in_ms = Some(500);
        p.place_asset(a, 2000, None, 0);
        let clips = format!("{:?}", p.clips);
        undo.apply(&mut p);
        assert_eq!(p.in_ms, Some(100));
        assert_eq!(format!("{:?}", p.clips), clips);
    }
}
