//! Closing gaps (G). With a selection, the selection and everything after
//! it slide left until something on one of their lanes touches; with
//! nothing selected, every empty stretch of the timeline is removed. Both
//! are ripple moves, so layers stay in sync with each other.

use super::*;

/// Something with a lane and a time range that a ripple can move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Item {
    Clip(ClipId),
    Text(TextId),
}

/// Stretches of `[0, last end)` that no span covers.
fn empty_stretches(spans: &mut [(i64, i64)]) -> Vec<(i64, i64)> {
    spans.sort_unstable();
    let mut cursor = 0;
    let mut gaps = Vec::new();
    for &(start, end) in spans.iter() {
        if start > cursor {
            gaps.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    gaps
}

/// How far an item starting at `t` moves left once `gaps` are removed.
fn removed_before(gaps: &[(i64, i64)], t: i64) -> i64 {
    gaps.iter()
        .filter(|(_, end)| *end <= t)
        .map(|(start, end)| end - start)
        .sum()
}

impl Project {
    fn ripple_items(&self) -> Vec<(Item, TrackId, i64, i64)> {
        self.clips
            .iter()
            .map(|c| (Item::Clip(c.id), c.track_id, c.cut_start_ms, c.cut_end_ms()))
            .chain(
                self.texts
                    .iter()
                    .map(|t| (Item::Text(t.id), t.track, t.start_ms, t.end_ms)),
            )
            .collect()
    }

    fn shift_item(&mut self, item: Item, by: i64) {
        match item {
            Item::Clip(id) => {
                if let Some(c) = self.clips.iter_mut().find(|c| c.id == id) {
                    c.cut_start_ms = (c.cut_start_ms - by).max(0);
                }
            }
            Item::Text(id) => {
                if let Some(t) = self.text_mut(id) {
                    let len = t.duration_ms();
                    t.start_ms = (t.start_ms - by).max(0);
                    t.end_ms = t.start_ms + len;
                }
            }
        }
    }

    /// Closes gaps as described in the module docs. Returns false when
    /// nothing needed to move.
    pub(super) fn close_gaps(&mut self) -> bool {
        let items = self.ripple_items();
        let selected: HashSet<Item> = self
            .selected_clips
            .iter()
            .flat_map(|id| self.edit_ids(*id))
            .map(Item::Clip)
            .chain(self.selected_text.map(Item::Text))
            .collect();

        if selected.is_empty() {
            let mut spans: Vec<_> = items.iter().map(|i| (i.2, i.3)).collect();
            let gaps = empty_stretches(&mut spans);
            if gaps.is_empty() {
                return false;
            }
            for (item, _, start, _) in items {
                let by = removed_before(&gaps, start);
                if by > 0 {
                    self.shift_item(item, by);
                }
            }
            return true;
        }

        // The selection and everything starting at or after it ripple left.
        let from = items
            .iter()
            .filter(|i| selected.contains(&i.0))
            .map(|i| i.2)
            .min()
            .unwrap_or(0);
        let mut moving: HashSet<Item> = selected;
        for (item, _, start, _) in &items {
            if *start >= from {
                match item {
                    Item::Clip(id) => moving.extend(self.edit_ids(*id).into_iter().map(Item::Clip)),
                    Item::Text(_) => {
                        moving.insert(*item);
                    }
                }
            }
        }
        // The largest shift that keeps every moving item clear of the
        // fixed items left of it on its own lane (and of time zero).
        let shift = items
            .iter()
            .filter(|m| moving.contains(&m.0))
            .map(|m| {
                let wall = items
                    .iter()
                    .filter(|n| !moving.contains(&n.0) && n.1 == m.1 && n.2 < m.2)
                    .map(|n| n.3)
                    .max()
                    .unwrap_or(0);
                m.2 - wall
            })
            .min()
            .unwrap_or(0);
        if shift <= 0 {
            return false;
        }
        for item in moving {
            self.shift_item(item, shift);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(p: &mut Project, kind: TrackKind) -> TrackId {
        p.new_track(kind, usize::MAX)
    }
    fn clip(p: &mut Project, track: TrackId, start: i64, len: i64) -> ClipId {
        let id = p.id_gen.next_clip();
        p.clips.push(Clip {
            id,
            track_id: track,
            cut_start_ms: start,
            src_start_ms: 0,
            src_end_ms: len,
            link: None,
            gain: 1.0,
            speed: 1.0,
            transform: Default::default(),
            animation: Default::default(),
            asset_id: None,
            audio_stream: None,
        });
        id
    }
    fn start(p: &Project, id: ClipId) -> i64 {
        track::clip_by_id(&p.clips, id).unwrap().cut_start_ms
    }

    #[test]
    fn stretches_cover_the_lead_in_and_holes_but_not_overlaps() {
        let mut spans = vec![(2_000, 3_000), (500, 1_000), (900, 1_500)];
        assert_eq!(empty_stretches(&mut spans), vec![(0, 500), (1_500, 2_000)]);
        assert_eq!(removed_before(&[(0, 500), (1_500, 2_000)], 2_000), 1_000);
    }

    #[test]
    fn nothing_selected_closes_every_gap_and_keeps_layers_aligned() {
        let mut p = Project::empty();
        let v1 = lane(&mut p, TrackKind::Video);
        let v2 = lane(&mut p, TrackKind::Video);
        let a = clip(&mut p, v1, 1_000, 2_000);
        let b = clip(&mut p, v1, 5_000, 1_000);
        // Overlay on another lane, covering part of the hole: that part stays.
        let o = clip(&mut p, v2, 3_500, 500);
        assert!(p.close_gaps());
        assert_eq!(
            (start(&p, a), start(&p, o), start(&p, b)),
            (0, 2_000, 2_500)
        );
        assert!(!p.close_gaps());
    }

    #[test]
    fn selection_ripples_left_until_its_lane_neighbour() {
        let mut p = Project::empty();
        let v1 = lane(&mut p, TrackKind::Video);
        let v2 = lane(&mut p, TrackKind::Video);
        let a = clip(&mut p, v1, 0, 2_000);
        let b = clip(&mut p, v1, 4_000, 1_000);
        let later = clip(&mut p, v2, 6_000, 1_000);
        let earlier = clip(&mut p, v2, 0, 500);
        p.selected_clips = [b].into_iter().collect();
        assert!(p.close_gaps());
        assert_eq!(start(&p, a), 0);
        assert_eq!(start(&p, b), 2_000);
        // Everything after the selection keeps its offset; earlier items stay.
        assert_eq!(start(&p, later), 4_000);
        assert_eq!(start(&p, earlier), 0);
        assert!(!p.close_gaps());
    }

    #[test]
    fn text_after_a_gap_moves_with_the_ripple() {
        let mut p = Project::empty();
        let v1 = lane(&mut p, TrackKind::Video);
        clip(&mut p, v1, 0, 1_000);
        clip(&mut p, v1, 3_000, 1_000);
        p.playhead_ms = 3_200;
        let id = p.add_text(None);
        p.selected_text = None;
        assert!(p.close_gaps());
        let t = p.text(id).unwrap();
        assert_eq!((t.start_ms, t.end_ms), (1_200, 4_200));
    }
}
