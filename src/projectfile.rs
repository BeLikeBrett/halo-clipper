//! Saving and opening an edit ("project file"). The whole timeline is
//! written to a small JSON file that references the source media by path —
//! it does not embed the video. Opening re-reads it exactly as saved; if a
//! referenced file has moved, the open fails with a message naming it.
use super::grouping::MediaGroup;
use super::simple::SourceEdit;
use super::*;
use crate::text::TextId;
use crate::track::{ClipId, IdGen, LinkId, TrackId};
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// On-disk edit format. Bumped if the shape changes incompatibly; older
/// apps refuse a newer number rather than mis-reading it.
const FORMAT_VERSION: u32 = 1;

/// The file's extension (without the dot).
pub(super) const EXTENSION: &str = "haloedit";

/// A serializable mirror of [`Project`]: the persistent state only. Maps
/// with non-string keys become `Vec`s (JSON can't key a map by a number),
/// the scrub proxy and ID generator are left out (rebuilt on open), and a
/// format/version header rides along for compatibility checks.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ProjectFile {
    format: u32,
    app_version: String,
    source: std::path::PathBuf,
    media: crate::media::MediaInfo,
    mode: EditMode,
    playhead_ms: i64,
    #[serde(default)]
    in_ms: Option<i64>,
    #[serde(default)]
    out_ms: Option<i64>,
    #[serde(default)]
    simple_audio_muted: bool,
    canvas: crate::framing::CanvasSettings,
    #[serde(default)]
    assets: Vec<crate::track::MediaAsset>,
    #[serde(default)]
    tracks: Vec<crate::track::Track>,
    #[serde(default)]
    clips: Vec<crate::track::Clip>,
    #[serde(default)]
    texts: Vec<crate::text::TextItem>,
    #[serde(default)]
    groups: Vec<(LinkId, MediaGroup)>,
    #[serde(default)]
    selected_clips: Vec<ClipId>,
    #[serde(default)]
    selected_text: Option<TextId>,
    #[serde(default)]
    simple_focus: Option<TrackId>,
    #[serde(default)]
    simple_edits: Vec<(TrackId, SourceEdit)>,
    #[serde(default)]
    simple_advanced_position: i64,
}

impl ProjectFile {
    fn from_project(p: &Project) -> Self {
        Self {
            format: FORMAT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            source: p.source.clone(),
            media: p.media.clone(),
            mode: p.mode,
            playhead_ms: p.playhead_ms,
            in_ms: p.in_ms,
            out_ms: p.out_ms,
            simple_audio_muted: p.simple_audio_muted,
            canvas: p.canvas,
            assets: p.assets.clone(),
            tracks: p.tracks.clone(),
            clips: p.clips.clone(),
            texts: p.texts.clone(),
            groups: p.groups.iter().map(|(k, v)| (*k, v.clone())).collect(),
            selected_clips: p.selected_clips.iter().copied().collect(),
            selected_text: p.selected_text,
            simple_focus: p.simple.focus,
            simple_edits: p.simple.edits.iter().map(|(k, v)| (*k, *v)).collect(),
            simple_advanced_position: p.simple.advanced_position,
        }
    }

    /// Every source file the edit depends on.
    fn referenced_paths(&self) -> Vec<&Path> {
        let mut out: Vec<&Path> = vec![self.source.as_path()];
        for t in &self.tracks {
            if let Some(s) = &t.source {
                out.push(&s.path);
            }
        }
        for a in &self.assets {
            out.push(&a.source.path);
        }
        out
    }

    /// Fail with a clear, itemized message when any referenced file moved.
    fn ensure_media_present(&self) -> anyhow::Result<()> {
        let mut missing: Vec<String> = Vec::new();
        for p in self.referenced_paths() {
            if !p.exists() {
                let p = p.display().to_string();
                if !missing.contains(&p) {
                    missing.push(p);
                }
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let list = missing
            .iter()
            .map(|p| format!("  •  {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "This edit can’t open — some of the video it uses has moved or been \
             deleted:\n{list}\n\nPut the file(s) back where they were and open the edit again."
        );
    }

    fn into_project(self) -> Project {
        // Rebuild the ID generator above every restored ID so new edits
        // never reuse one. Track IDs are shared by tracks and imported
        // assets, so both feed the track counter.
        let next = |it: &mut dyn Iterator<Item = u32>| it.max().map(|m| m + 1).unwrap_or(0);
        let clip = next(&mut self.clips.iter().map(|c| c.id.0));
        let link = next(
            &mut self
                .clips
                .iter()
                .filter_map(|c| c.link.map(|l| l.0))
                .chain(self.groups.iter().map(|(k, _)| k.0)),
        );
        let track = next(
            &mut self
                .tracks
                .iter()
                .map(|t| t.id.0)
                .chain(self.assets.iter().map(|a| a.id.0))
                .chain(self.clips.iter().map(|c| c.track_id.0))
                .chain(self.clips.iter().filter_map(|c| c.asset_id.map(|id| id.0)))
                .chain(self.texts.iter().map(|t| t.track.0)),
        );
        let text = next(&mut self.texts.iter().map(|t| t.id.0));
        // `next_import_number` returns counter+1, so seed with the current
        // max (not max+1) to hand out the next unused number.
        let import = self
            .assets
            .iter()
            .map(|a| a.import_number)
            .max()
            .unwrap_or(0);

        Project {
            source: self.source,
            proxy: None,
            media: self.media,
            playhead_ms: self.playhead_ms,
            simple: super::simple::Workspace {
                focus: self.simple_focus,
                edits: self.simple_edits.into_iter().collect(),
                advanced_position: self.simple_advanced_position,
            },
            in_ms: self.in_ms,
            out_ms: self.out_ms,
            simple_audio_muted: self.simple_audio_muted,
            assets: self.assets,
            canvas: self.canvas,
            groups: self.groups.into_iter().collect::<HashMap<_, _>>(),
            tracks: self.tracks,
            clips: self.clips,
            selected_clips: self.selected_clips.into_iter().collect::<HashSet<_>>(),
            texts: self.texts,
            selected_text: self.selected_text,
            id_gen: IdGen::seeded(clip, link, track, text, import),
            mode: self.mode,
        }
    }
}

/// Write `project` to `path` as a Halo Clipper edit.
pub(super) fn save(path: &Path, project: &Project) -> anyhow::Result<()> {
    let file = ProjectFile::from_project(project);
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read a Halo Clipper edit from `path`, verifying its format and that all
/// of its source media still exists.
pub(super) fn load(path: &Path) -> anyhow::Result<Project> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let file: ProjectFile = serde_json::from_slice(&bytes)
        .context("This isn’t a Halo Clipper edit, or the file is damaged.")?;
    anyhow::ensure!(
        file.format <= FORMAT_VERSION,
        "This edit was saved by a newer version of Halo Clipper ({}). Update, then open it.",
        file.app_version
    );
    file.ensure_media_present()?;
    Ok(file.into_project())
}

impl ClipperApp {
    /// Menu → Save Edit…: pick a location and write the timeline there.
    pub(super) fn save_edit(&mut self) {
        let Some(project) = self.project.as_ref() else {
            self.status = "Open a video before saving an edit.".into();
            return;
        };
        let stem = project
            .source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Halo Clipper edit", &[EXTENSION])
            .set_file_name(format!("{stem}.{EXTENSION}"))
            .save_file()
        else {
            return;
        };
        match save(&path, project) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("edit");
                self.status = format!(
                    "Saved {name}. It links to your source videos — if you move them, \
                     reopening will tell you which are missing."
                );
            }
            Err(e) => self.status = format!("Could not save edit: {e:#}"),
        }
    }

    /// Menu → Open Edit…: pick an edit and restore it exactly, or report
    /// clearly why it can't open.
    pub(super) fn open_edit(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Halo Clipper edit", &[EXTENSION])
            .pick_file()
        else {
            return;
        };
        match load(&path) {
            Ok(project) => self.apply_loaded_edit(project, &path),
            Err(e) => {
                self.status = format!("{e:#}");
                log::warn!("open edit failed: {e:#}");
            }
        }
    }

    /// Swap in a freshly loaded project, resetting the same transient state
    /// (preview, scrub proxy, undo history) that opening a video does.
    fn apply_loaded_edit(&mut self, project: Project, path: &Path) {
        self.live.clear();
        if let Some(h) = self.proxy_handle.take() {
            h.cancel();
        }
        let preview = project.source.clone();
        let seek = if project.mode == EditMode::SimpleClip {
            project.playhead_ms.max(0)
        } else {
            0
        };
        self.project = Some(project);
        if let Err(e) = self.player.set_file(&preview, seek) {
            log::warn!("preview load after open edit: {e:#}");
        }
        self.histories.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("edit");
        self.status = format!("Opened {name}.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{Clip, Track, TrackKind, Transform};

    fn fixture() -> Project {
        let mut p = Project::empty();
        p.source = std::path::PathBuf::from("/tmp/halo-clipper-test-source.mp4");
        p.mode = EditMode::MultiTrack;
        p.playhead_ms = 4_321;
        let vid = p.id_gen.next_track();
        p.tracks.push(Track {
            id: vid,
            kind: TrackKind::Video,
            name: "V1".into(),
            muted: true, // disabled track must survive the round-trip too
            gain: 1.0,
            source_audio_index: None,
            source: None,
        });
        let cid = p.id_gen.next_clip();
        p.clips.push(Clip {
            id: cid,
            track_id: vid,
            cut_start_ms: 0,
            src_start_ms: 0,
            src_end_ms: 1_000,
            link: None,
            gain: 1.0,
            speed: 1.5,
            transform: Transform::default(),
            animation: Default::default(),
            asset_id: None,
            audio_stream: None,
        });
        let tid = p.id_gen.next_track();
        p.tracks.push(Track {
            id: tid,
            kind: TrackKind::Text,
            name: "T1".into(),
            muted: false,
            gain: 1.0,
            source_audio_index: None,
            source: None,
        });
        let txt = p.id_gen.next_text();
        p.texts.push(crate::text::TextItem::new(txt, tid, 0, 2_000));
        p
    }

    // A project survives a save → load round-trip unchanged (the thing the
    // user is promised: "it opens right where you saved").
    #[test]
    fn roundtrip_preserves_the_timeline() {
        let p = fixture();
        let json = serde_json::to_string(&ProjectFile::from_project(&p)).unwrap();
        let back: ProjectFile = serde_json::from_str(&json).unwrap();
        let q = back.into_project();
        assert!(q.mode == p.mode, "edit mode changed across the round-trip");
        assert_eq!(q.playhead_ms, p.playhead_ms);
        assert_eq!(q.source, p.source);
        assert_eq!(q.clips.len(), p.clips.len());
        assert_eq!(q.tracks.len(), p.tracks.len());
        assert_eq!(q.texts.len(), p.texts.len());
        assert_eq!(q.clips[0].speed, p.clips[0].speed);
        assert!(q.tracks[0].muted, "disabled track stays disabled");
        // New IDs must not collide with restored ones.
        let max_clip = q.clips.iter().map(|c| c.id.0).max().unwrap_or(0);
        assert!(q.id_gen.next_clip().0 > max_clip);
    }

    #[test]
    fn missing_media_is_reported_by_path() {
        let mut p = fixture();
        p.source = std::path::PathBuf::from("/nonexistent/moved-away.mp4");
        let err = ProjectFile::from_project(&p).ensure_media_present().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("moved-away.mp4"), "message names the file: {msg}");
    }
}
