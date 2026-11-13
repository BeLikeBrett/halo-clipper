//! Audio attachment is a reversible view/edit group. Media edits stay on clips.
use super::*;
#[derive(Clone, Debug)]
pub(super) struct MediaGroup {
    pub members: HashSet<ClipId>,
    pub attached: bool,
}
impl Project {
    pub(super) fn group_for(&self, id: ClipId) -> Option<(LinkId, &MediaGroup)> {
        self.groups
            .iter()
            .find(|(_, g)| g.members.contains(&id))
            .map(|(id, g)| (*id, g))
    }
    pub(super) fn edit_ids(&self, id: ClipId) -> HashSet<ClipId> {
        if let Some((_, g)) = self.group_for(id) {
            return if g.attached {
                g.members
                    .iter()
                    .filter(|id| track::clip_by_id(&self.clips, **id).is_some())
                    .copied()
                    .collect()
            } else {
                [id].into_iter().collect()
            };
        }
        match track::clip_by_id(&self.clips, id).and_then(|c| c.link) {
            Some(link) => self
                .clips
                .iter()
                .filter(|c| c.link == Some(link))
                .map(|c| c.id)
                .collect(),
            None => [id].into_iter().collect(),
        }
    }
    pub(super) fn group_bounds(&self, id: ClipId) -> (i64, i64) {
        let ids = self.edit_ids(id);
        let mut clips = self.clips.iter().filter(|c| ids.contains(&c.id));
        let Some(first) = clips.next() else {
            return (0, 0);
        };
        clips.fold((first.cut_start_ms, first.cut_end_ms()), |(a, b), c| {
            (a.min(c.cut_start_ms), b.max(c.cut_end_ms()))
        })
    }
    /// Audio tracks folded into compound (V+A) bars on `video_track`. The
    /// row's monitor toggle acts on these, since their own lanes are hidden
    /// while attached.
    pub(super) fn attached_audio_tracks(&self, video_track: TrackId) -> HashSet<TrackId> {
        let track_of = |id: &ClipId| track::clip_by_id(&self.clips, *id).map(|c| c.track_id);
        self.groups
            .values()
            .filter(|g| g.attached && g.members.iter().any(|id| track_of(id) == Some(video_track)))
            .flat_map(|g| {
                g.members
                    .iter()
                    .filter(|id| !self.is_video(**id))
                    .filter_map(track_of)
            })
            .collect()
    }
    pub(super) fn is_video(&self, id: ClipId) -> bool {
        track::clip_by_id(&self.clips, id).is_some_and(|c| {
            self.tracks
                .iter()
                .any(|t| t.id == c.track_id && t.kind == TrackKind::Video)
        })
    }
    pub(super) fn detach_selection(&mut self) -> bool {
        let selected = self.selected_clips.clone();
        let mut changed = false;
        for id in selected {
            if let Some((key, g)) = self.group_for(id) {
                if g.attached {
                    self.groups.get_mut(&key).unwrap().attached = false;
                    changed = true;
                }
            } else if let Some(link) = track::clip_by_id(&self.clips, id).and_then(|c| c.link) {
                let members = self
                    .clips
                    .iter()
                    .filter(|c| c.link == Some(link))
                    .map(|c| c.id)
                    .collect();
                self.groups.insert(
                    link,
                    MediaGroup {
                        members,
                        attached: false,
                    },
                );
                changed = true;
            }
        }
        if changed {
            log::info!(
                "Detached audio for {} selected clips",
                self.selected_clips.len()
            );
        }
        changed
    }
    fn attachment_members(&self, selection: &HashSet<ClipId>) -> HashSet<ClipId> {
        let mut members = selection.clone();
        let selected_video = members.iter().any(|id| self.is_video(*id));
        let groups: Vec<_> = self
            .groups
            .values()
            .filter(|g| {
                g.members
                    .iter()
                    .any(|id| selection.contains(id) && (!selected_video || self.is_video(*id)))
            })
            .cloned()
            .collect();
        for g in groups {
            members.extend(g.members);
        }
        members.retain(|id| track::clip_by_id(&self.clips, *id).is_some());
        members
    }
    pub(super) fn can_attach_ids(&self, selection: &HashSet<ClipId>) -> bool {
        let members = self.attachment_members(selection);
        members.iter().any(|id| self.is_video(*id))
            && members.iter().any(|id| !self.is_video(*id))
            && !self
                .groups
                .values()
                .any(|g| g.attached && g.members == members)
    }
    pub(super) fn can_detach_ids(&self, selection: &HashSet<ClipId>) -> bool {
        selection.iter().any(|id| {
            self.group_for(*id).is_some_and(|(_, g)| {
                g.attached
                    && g.members
                        .iter()
                        .any(|id| self.clips.iter().any(|c| c.id == *id) && !self.is_video(*id))
            })
        })
    }
    pub(super) fn attach_selection(&mut self) -> bool {
        if !self.can_attach_ids(&self.selected_clips) {
            return false;
        }
        let members = self.attachment_members(&self.selected_clips);
        for g in self.groups.values_mut() {
            g.members.retain(|id| !members.contains(id));
        }
        self.groups.retain(|_, g| !g.members.is_empty());
        let id = self.id_gen.next_link();
        let anchor = self
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .find_map(|t| {
                self.clips
                    .iter()
                    .find(|c| c.track_id == t.id && members.contains(&c.id))
                    .map(|c| c.id)
            });
        log::info!(
            "Attached {} edited clips into one media group",
            members.len()
        );
        self.groups.insert(
            id,
            MediaGroup {
                members,
                attached: true,
            },
        );
        self.selected_clips = anchor.into_iter().collect();
        true
    }
    pub(super) fn remember_split_groups(
        &mut self,
        before: &[Clip],
        new_ids: &[ClipId],
        targets: &[ClipId],
        cut: i64,
    ) {
        let keys: Vec<_> = self.groups.keys().copied().collect();
        for key in keys {
            let old = self.groups[&key].members.clone();
            if !targets.iter().any(|id| old.contains(id)) {
                continue;
            }
            let mut right_ids = Vec::new();
            for left in before.iter().filter(|c| old.contains(&c.id)) {
                if let Some(index) = self.clips.iter().position(|c| c.id == left.id) {
                    if let Some(right) = self.clips.get(index + 1) {
                        if new_ids.contains(&right.id) {
                            right_ids.push(right.id);
                        }
                    }
                }
            }
            self.groups.get_mut(&key).unwrap().members.extend(right_ids);
            if self.groups[&key].attached {
                let right: HashSet<_> = self.groups[&key]
                    .members
                    .iter()
                    .filter(|id| {
                        track::clip_by_id(&self.clips, **id).is_some_and(|c| c.cut_start_ms >= cut)
                    })
                    .copied()
                    .collect();
                let left_exists = self.groups[&key].members.iter().any(|id| {
                    track::clip_by_id(&self.clips, *id).is_some_and(|c| c.cut_start_ms < cut)
                });
                if left_exists && !right.is_empty() {
                    self.groups
                        .get_mut(&key)
                        .unwrap()
                        .members
                        .retain(|id| !right.contains(id));
                    let id = self.id_gen.next_link();
                    self.groups.insert(
                        id,
                        MediaGroup {
                            members: right,
                            attached: true,
                        },
                    );
                }
            }
        }
    }
    /// One compound bar when attached; original clips and audio lanes when detached.
    pub(super) fn timeline_view(&self) -> (Vec<Track>, Vec<Clip>, HashMap<ClipId, String>) {
        let mut hidden = HashSet::new();
        let mut compounds = Vec::new();
        let mut labels = HashMap::new();
        for g in self.groups.values().filter(|g| g.attached) {
            let anchor = self
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .find_map(|t| {
                    self.clips
                        .iter()
                        .filter(|c| g.members.contains(&c.id) && c.track_id == t.id)
                        .min_by_key(|c| c.cut_start_ms)
                })
                .copied();
            let Some(mut anchor) = anchor else { continue };
            let members: Vec<_> = self
                .clips
                .iter()
                .filter(|c| g.members.contains(&c.id))
                .collect();
            if members.len() < 2 {
                continue;
            }
            let start = members.iter().map(|c| c.cut_start_ms).min().unwrap();
            let end = members.iter().map(|c| c.cut_end_ms()).max().unwrap();
            hidden.extend(g.members.iter().copied());
            anchor.cut_start_ms = start;
            anchor.src_start_ms = 0;
            anchor.src_end_ms = end - start;
            anchor.speed = 1.0;
            let audio = members.iter().filter(|c| !self.is_video(c.id)).count();
            let name = self
                .clip_source(&anchor)
                .0
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            labels.insert(
                anchor.id,
                if audio > 0 {
                    format!("{name} · V+A")
                } else {
                    format!("{name} · group")
                },
            );
            compounds.push(anchor);
        }
        let mut clips: Vec<_> = self
            .clips
            .iter()
            .filter(|c| !hidden.contains(&c.id))
            .copied()
            .collect();
        clips.extend(compounds);
        let tracks = self
            .tracks
            .iter()
            .filter(|t| clips.iter().any(|c| c.track_id == t.id))
            .cloned()
            .collect();
        (tracks, clips, labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::LayerSource;
    fn add(p: &mut Project, name: &str, audio: usize) -> ClipId {
        let id = p.import_asset(LayerSource {
            path: name.into(),
            media: MediaInfo {
                duration_ms: 6000,
                width: 1920,
                height: 1080,
                fps: 30.0,
                video_codec: "h264".into(),
                audio_codec: Some("aac".into()),
                audio_stream_count: audio,
            },
        });
        p.place_asset(id, 0, None, 0).unwrap()
    }
    fn state(p: &Project) -> String {
        format!("{:?}", p.clips)
    }
    #[test]
    fn defaults_to_one_bar_without_changing_audio_playback_or_export() {
        let mut p = Project::empty();
        let video = add(&mut p, "one.mp4", 2);
        assert_eq!(p.clips.len(), 3);
        assert_eq!(p.timeline_view().1.len(), 1);
        assert_eq!(p.edit_ids(video).len(), 3);
        let composition = format!("{:?}", p.composition(false));
        let original = state(&p);
        assert!(p.detach_selection());
        assert_eq!(p.timeline_view().1.len(), 3);
        assert_eq!(p.edit_ids(video).len(), 1);
        assert_eq!(state(&p), original);
        assert_eq!(format!("{:?}", p.composition(false)), composition);
        assert!(p.attach_selection());
        assert_eq!(p.timeline_view().1.len(), 1);
        assert_eq!(state(&p), original);
        assert_eq!(format!("{:?}", p.composition(false)), composition);
    }
    #[test]
    fn detached_cuts_gaps_speed_gain_and_deleted_pieces_survive_repeated_attachment() {
        let mut p = Project::empty();
        let video = add(&mut p, "edited.mp4", 2);
        p.detach_selection();
        let audio = p.clips.iter().find(|c| !p.is_video(c.id)).unwrap().id;
        let right = p.split_segments(&[audio], 2000)[0];
        let tail = p.split_segments(&[right], 4000)[0];
        p.clips.retain(|c| c.id != right); // deliberate 2-second hole
        let clip = p.clips.iter_mut().find(|c| c.id == tail).unwrap();
        clip.cut_start_ms = 4500;
        clip.speed = 1.25;
        clip.gain = 0.37;
        clip.src_start_ms += 125;
        let original = state(&p);
        let exported = format!("{:?}", p.composition(false));
        for _ in 0..4 {
            p.selected_clips = [video].into_iter().collect();
            assert!(p.attach_selection());
            assert_eq!(p.timeline_view().1.len(), 1);
            assert_eq!(state(&p), original);
            assert_eq!(format!("{:?}", p.composition(false)), exported);
            assert!(p.detach_selection());
            assert_eq!(state(&p), original);
        }
    }
    #[test]
    fn multiple_media_and_external_audio_collapse_and_restore_with_undo() {
        let mut p = Project::empty();
        let first = add(&mut p, "one.mp4", 1);
        let second = add(&mut p, "two.mp4", 1);
        p.selected_clips = [first, second].into_iter().collect();
        p.detach_selection();
        let id = p.import_asset(LayerSource {
            path: "music.wav".into(),
            media: MediaInfo {
                width: 0,
                height: 0,
                ..p.media.clone()
            },
        });
        let music = p.place_asset(id, 1500, None, p.tracks.len()).unwrap();
        p.selected_clips = [first, second, music].into_iter().collect();
        let original = state(&p);
        let undo = UndoSnapshot::capture(&p);
        assert!(p.attach_selection());
        assert_eq!(p.timeline_view().1.len(), 1);
        let after = UndoSnapshot::capture(&p);
        undo.apply(&mut p);
        assert_eq!(p.timeline_view().1.len(), 5);
        after.apply(&mut p);
        assert_eq!(p.timeline_view().1.len(), 1);
        assert!(p.detach_selection());
        assert_eq!(state(&p), original);
    }
    #[test]
    fn moving_compound_preserves_every_offset_even_when_audio_extends_before_video() {
        let mut p = Project::empty();
        let video = add(&mut p, "one.mp4", 1);
        p.detach_selection();
        p.clips
            .iter_mut()
            .find(|c| c.id == video)
            .unwrap()
            .cut_start_ms = 1000;
        p.selected_clips = [video].into_iter().collect();
        p.attach_selection();
        let old = p.clips.clone();
        let lane = p.clips.iter().find(|c| c.id == video).unwrap().track_id;
        p.move_segment(video, 3000, Some(lane), 0);
        for c in &p.clips {
            let before = old.iter().find(|old| old.id == c.id).unwrap();
            assert_eq!(c.cut_start_ms, before.cut_start_ms + 3000);
            assert_eq!(c.src_start_ms, before.src_start_ms);
        }
        assert_eq!(p.timeline_view().1[0].cut_start_ms, 3000);
    }
    #[test]
    fn splitting_compound_at_an_internal_gap_preserves_both_sides() {
        let mut p = Project::empty();
        let video = add(&mut p, "gap.mp4", 1);
        p.detach_selection();
        let audio = p.clips.iter().find(|c| !p.is_video(c.id)).unwrap().id;
        for id in [video, audio] {
            p.clips.iter_mut().find(|c| c.id == id).unwrap().src_end_ms = 1000;
        }
        let asset = p.clips[0].asset_id.unwrap();
        let next = p.place_asset(asset, 4000, None, 0).unwrap();
        p.selected_clips = [video, next].into_iter().collect();
        assert!(p.attach_selection());
        let targets: Vec<_> = p.edit_ids(video).into_iter().collect();
        let before = state(&p);
        let groups = p.groups.len();
        assert!(p.split_segments(&targets, 2000).is_empty());
        assert_eq!(p.groups.len(), groups + 1);
        assert_eq!(state(&p), before);
        assert_eq!(p.timeline_view().1.len(), 2);
    }
    #[test]
    fn video_and_audio_edits_remain_separate_under_the_collapsed_bar() {
        let mut p = Project::empty();
        let video = add(&mut p, "many-cuts.mp4", 1);
        p.detach_selection();
        let right = p.split_segments(&[video], 2000)[0];
        let tail = p.split_segments(&[right], 4000)[0];
        p.clips.retain(|c| c.id != right);
        p.clips
            .iter_mut()
            .find(|c| c.id == tail)
            .unwrap()
            .transform
            .scale = 0.6;
        let original = state(&p);
        p.selected_clips = [video].into_iter().collect();
        p.attach_selection();
        assert_eq!(p.timeline_view().1.len(), 1);
        p.detach_selection();
        assert_eq!(p.timeline_view().1.len(), 3);
        assert_eq!(state(&p), original);
    }
    #[test]
    fn splitting_attached_bar_makes_independent_compounds_and_remembers_detached_splits() {
        let mut p = Project::empty();
        let video = add(&mut p, "one.mp4", 1);
        let targets: Vec<_> = p.edit_ids(video).into_iter().collect();
        let right = p.split_segments(&targets, 3000);
        assert_eq!(right.len(), 2);
        assert_eq!(p.timeline_view().1.len(), 2);
        let right_video = *right.iter().find(|id| p.is_video(**id)).unwrap();
        assert_ne!(
            p.group_for(video).unwrap().0,
            p.group_for(right_video).unwrap().0
        );
        p.selected_clips = [right_video].into_iter().collect();
        p.detach_selection();
        assert_eq!(p.timeline_view().1.len(), 3);
        p.attach_selection();
        assert_eq!(p.timeline_view().1.len(), 2);
    }
}
