//! Text layers: free-floating titles and captions placed anywhere on the
//! canvas, with their own time range on the timeline.
//!
//! This module is the pure model: styling, presets and the animation
//! curves. Rasterizing lives in `textrender`, which both the live preview
//! and the export use, so what you see is exactly what gets burned in.

use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TextId(pub u32);

/// Bundled caption fonts (Latin subsets, OFL / Apache 2.0). Bundled rather
/// than looked up on the system so preview and export, Linux and Windows,
/// always draw the same glyphs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum TextFont {
    Montserrat,
    Poppins,
    Inter,
    BebasNeue,
    Anton,
    LuckiestGuy,
    Bangers,
    PermanentMarker,
    Pacifico,
    CourierPrime,
    Playfair,
}

impl TextFont {
    pub const ALL: [Self; 11] = [
        Self::Montserrat,
        Self::Poppins,
        Self::Inter,
        Self::BebasNeue,
        Self::Anton,
        Self::LuckiestGuy,
        Self::Bangers,
        Self::PermanentMarker,
        Self::Pacifico,
        Self::CourierPrime,
        Self::Playfair,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Montserrat => "Montserrat",
            Self::Poppins => "Poppins",
            Self::Inter => "Inter",
            Self::BebasNeue => "Bebas Neue",
            Self::Anton => "Anton",
            Self::LuckiestGuy => "Luckiest Guy",
            Self::Bangers => "Bangers",
            Self::PermanentMarker => "Permanent Marker",
            Self::Pacifico => "Pacifico",
            Self::CourierPrime => "Courier Prime",
            Self::Playfair => "Playfair",
        }
    }

    pub fn data(self) -> &'static [u8] {
        match self {
            Self::Montserrat => include_bytes!("../assets/fonts/caption/Montserrat-ExtraBold.ttf"),
            Self::Poppins => include_bytes!("../assets/fonts/caption/Poppins-Bold.ttf"),
            Self::Inter => include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
            Self::BebasNeue => include_bytes!("../assets/fonts/caption/BebasNeue-Regular.ttf"),
            Self::Anton => include_bytes!("../assets/fonts/caption/Anton-Regular.ttf"),
            Self::LuckiestGuy => include_bytes!("../assets/fonts/caption/LuckiestGuy-Regular.ttf"),
            Self::Bangers => include_bytes!("../assets/fonts/caption/Bangers-Regular.ttf"),
            Self::PermanentMarker => {
                include_bytes!("../assets/fonts/caption/PermanentMarker-Regular.ttf")
            }
            Self::Pacifico => include_bytes!("../assets/fonts/caption/Pacifico-Regular.ttf"),
            Self::CourierPrime => include_bytes!("../assets/fonts/caption/CourierPrime-Bold.ttf"),
            Self::Playfair => include_bytes!("../assets/fonts/caption/PlayfairDisplay-Bold.ttf"),
        }
    }

    /// egui family name the font is registered under (for the font menu,
    /// which shows each name in its own face).
    pub fn egui_family(self) -> String {
        format!("caption:{}", self.label())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Shadow {
    None,
    /// Soft dark drop shadow below the text.
    Soft,
    /// Blurred halo in the text color ("neon").
    Glow,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Intro {
    None,
    Fade,
    /// Scales up from small with a slight overshoot.
    Pop,
    /// Rises into place while fading in.
    Slide,
    /// Letters appear one by one with human-like rhythm and a caret.
    Typewriter,
    /// Words pop in one at a time (short-form caption style).
    Words,
}

impl Intro {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Fade,
        Self::Pop,
        Self::Slide,
        Self::Typewriter,
        Self::Words,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Fade => "Fade",
            Self::Pop => "Pop",
            Self::Slide => "Rise",
            Self::Typewriter => "Type",
            Self::Words => "Words",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Outro {
    None,
    Fade,
    Pop,
    Slide,
}

impl Outro {
    pub const ALL: [Self; 4] = [Self::None, Self::Fade, Self::Pop, Self::Slide];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Fade => "Fade",
            Self::Pop => "Pop",
            Self::Slide => "Drop",
        }
    }
}

/// Two-color gradient; the starting color is the control's solid color.
#[derive(Clone, Copy, PartialEq, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Gradient {
    pub end: [u8; 3],
    pub angle: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TextStyle {
    pub font: TextFont,
    pub color: [u8; 3],
    pub align: Align,
    pub gradient: Option<Gradient>,
    pub outline_gradient: Option<Gradient>,
    pub box_gradient: Option<Gradient>,
    /// Outline width as a fraction of the font size; 0 = none.
    pub outline: f32,
    pub outline_color: [u8; 3],
    /// Rounded box behind each line: rgb + opacity.
    pub background: Option<([u8; 3], f32)>,
    pub shadow: Shadow,
}

impl Default for TextStyle {
    fn default() -> Self {
        STYLE_PRESETS[1].1
    }
}

/// One-click looks, in the spirit of TikTok / CapCut text styles. They set
/// colors and decoration and leave the font alone.
pub const STYLE_PRESETS: [(&str, TextStyle); 6] = [
    (
        "Clean",
        TextStyle {
            font: TextFont::Montserrat,
            color: [255, 255, 255],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.0,
            outline_color: [0, 0, 0],
            background: None,
            shadow: Shadow::Soft,
        },
    ),
    (
        "Outline",
        TextStyle {
            font: TextFont::Montserrat,
            color: [255, 255, 255],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.075,
            outline_color: [0, 0, 0],
            background: None,
            shadow: Shadow::None,
        },
    ),
    (
        "Pop",
        TextStyle {
            font: TextFont::Montserrat,
            color: [255, 214, 10],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.09,
            outline_color: [0, 0, 0],
            background: None,
            shadow: Shadow::Soft,
        },
    ),
    (
        "Box",
        TextStyle {
            font: TextFont::Montserrat,
            color: [255, 255, 255],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.0,
            outline_color: [0, 0, 0],
            background: Some(([0, 0, 0], 0.72)),
            shadow: Shadow::None,
        },
    ),
    (
        "Label",
        TextStyle {
            font: TextFont::Montserrat,
            color: [16, 16, 16],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.0,
            outline_color: [0, 0, 0],
            background: Some(([255, 255, 255], 1.0)),
            shadow: Shadow::None,
        },
    ),
    (
        "Neon",
        TextStyle {
            font: TextFont::Montserrat,
            color: [120, 232, 255],
            align: Align::Center,
            gradient: None,
            outline_gradient: None,
            box_gradient: None,
            outline: 0.0,
            outline_color: [0, 0, 0],
            background: None,
            shadow: Shadow::Glow,
        },
    ),
];

#[derive(Clone, Debug, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TextItem {
    pub id: TextId,
    /// The text lane this layer lives on; its position among the video
    /// lanes sets what the text draws above and below.
    pub track: crate::track::TrackId,
    /// Cut-time range, half-open.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Center of the text block, as a fraction of the canvas.
    pub x: f32,
    pub y: f32,
    /// Font size as a fraction of the canvas's short side, so a title
    /// keeps its proportion when the canvas switches between 16:9 and 9:16.
    pub size: f32,
    pub style: TextStyle,
    pub intro: Intro,
    pub outro: Outro,
    pub intro_ms: f32,
    pub outro_ms: f32,
}

pub const MIN_SIZE: f32 = 0.015;
pub const MAX_SIZE: f32 = 0.5;
pub const MIN_DURATION_MS: i64 = 200;

impl TextItem {
    pub fn new(id: TextId, track: crate::track::TrackId, start_ms: i64, end_ms: i64) -> Self {
        Self {
            id,
            track,
            start_ms,
            end_ms: end_ms.max(start_ms + MIN_DURATION_MS),
            text: String::new(),
            x: 0.5,
            y: 0.72,
            size: 0.07,
            style: TextStyle::default(),
            intro: Intro::Pop,
            outro: Outro::Fade,
            intro_ms: 320.0,
            outro_ms: 320.0,
        }
    }

    pub fn contains(&self, t_ms: i64) -> bool {
        t_ms >= self.start_ms && t_ms < self.end_ms
    }

    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }

    /// The text that is drawn: placeholder when empty so a fresh layer is
    /// visible and grabbable.
    pub fn shown_text(&self) -> &str {
        if self.text.trim().is_empty() {
            "Your text"
        } else {
            &self.text
        }
    }

    /// Hash of everything that changes the rendered pixels of the block
    /// (not its position or time), for texture caching.
    pub fn render_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.shown_text().hash(&mut h);
        self.size.to_bits().hash(&mut h);
        let s = &self.style;
        (
            s.font,
            s.color,
            s.align,
            s.outline.to_bits(),
            s.outline_color,
            s.shadow,
        )
            .hash(&mut h);
        for gradient in [s.gradient, s.outline_gradient, s.box_gradient] {
            gradient.map(|g| (g.end, g.angle.to_bits())).hash(&mut h);
        }
        s.background.map(|(c, a)| (c, a.to_bits())).hash(&mut h);
        h.finish()
    }
}

// ── Animation ───────────────────────────────────────────────────────────

/// How much of the text is revealed at a moment.
#[derive(Clone, Debug, PartialEq)]
pub enum Reveal {
    All,
    /// Typewriter: the first `shown` characters, plus an optional caret.
    Chars {
        shown: usize,
        caret: bool,
    },
    /// Words intro: per-word progress, 0 = hidden, 1 = settled.
    Words(Vec<f32>),
}

/// The animated state of a text block at one moment. Quantized, so equal
/// looks compare equal and the export can reuse frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub alpha: f32,
    pub scale: f32,
    /// Vertical offset in font-size units (positive = down).
    pub dy: f32,
    pub reveal: Reveal,
}

impl Look {
    pub const STILL: Look = Look {
        alpha: 1.0,
        scale: 1.0,
        dy: 0.0,
        reveal: Reveal::All,
    };

    /// True when nothing of the text would be drawn, e.g. the first frame
    /// of a Fade, Pop, Type or Words intro.
    pub fn is_blank(&self) -> bool {
        self.alpha < 0.02
            || match &self.reveal {
                Reveal::All => false,
                Reveal::Chars { shown, .. } => *shown == 0,
                Reveal::Words(progress) => progress.iter().all(|p| *p < 0.02),
            }
    }
    pub fn is_visible(&self) -> bool {
        self.alpha > 0.0 && self.scale > 0.0
    }
}

fn q(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Overshoots past 1 and settles — the "pop".
fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

/// Seeded 0..1 noise so typing rhythm is irregular yet identical on every
/// render (preview and export must agree).
fn jitter(i: usize) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x % 10_000) as f32 / 10_000.0
}

/// Moments (ms from the start) at which each character appears. Pauses a
/// little after spaces and longer after punctuation, like someone typing.
pub fn typing_schedule(text: &str, budget_ms: f32) -> Vec<f32> {
    let base = 1000.0 / 16.0;
    let mut times = Vec::new();
    let mut t = 0.0;
    let mut previous = ' ';
    for (i, ch) in text.chars().enumerate() {
        let mut step = base * (0.6 + 0.8 * jitter(i));
        if matches!(previous, '.' | '!' | '?' | ',' | ';' | ':' | '\n') {
            step += base * 2.5;
        } else if previous == ' ' {
            step += base * 0.4;
        }
        if i > 0 {
            t += step;
        }
        times.push(t);
        previous = ch;
    }
    // Long text in a short layer types faster rather than running over.
    let total = times.last().copied().unwrap_or(0.0);
    if total > budget_ms && total > 0.0 {
        let k = budget_ms / total;
        times.iter_mut().for_each(|t| *t *= k);
    }
    times
}

/// The moment (timeline ms) the intro has fully played: where a click
/// on the layer's timeline bar parks the playhead so the whole text shows.
pub fn settled_at(item: &TextItem) -> i64 {
    let chars = item.shown_text().chars().count();
    let settled = |look: &Look| {
        look.alpha >= 1.0
            && (look.scale - 1.0).abs() < 1e-3
            && look.dy.abs() < 1e-3
            && match &look.reveal {
                Reveal::All => true,
                Reveal::Chars { shown, .. } => *shown >= chars,
                Reveal::Words(progress) => progress.iter().all(|p| *p >= 1.0),
            }
    };
    let middle = item.start_ms + item.duration_ms() / 2;
    (item.start_ms..middle)
        .step_by(20)
        .find(|t| settled(&look_at(item, *t)))
        .unwrap_or(middle)
}

/// Number of whitespace-separated words.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Animated state of `item` at cut time `t_ms`.
pub fn look_at(item: &TextItem, t_ms: i64) -> Look {
    let dur = item.duration_ms() as f32;
    let t = (t_ms - item.start_ms) as f32;
    let left = (item.end_ms - t_ms) as f32;
    let transition = item.intro_ms.clamp(50.0, 5000.0).min(dur / 2.0).max(1.0);
    let mut look = Look::STILL;

    match item.intro {
        Intro::None => {}
        Intro::Fade => look.alpha *= ease_out_cubic((t / transition).clamp(0.0, 1.0)),
        Intro::Pop => {
            let p = (t / transition).clamp(0.0, 1.0);
            look.scale *= 0.5 + 0.5 * ease_out_back(p);
            look.alpha *= (p * 2.5).min(1.0);
        }
        Intro::Slide => {
            let p = (t / transition).clamp(0.0, 1.0);
            look.dy += 0.6 * (1.0 - ease_out_cubic(p));
            look.alpha *= ease_out_cubic(p);
        }
        Intro::Typewriter => {
            let text = item.shown_text();
            let mut schedule = typing_schedule(text, f32::MAX);
            let total = schedule.last().copied().unwrap_or(0.0);
            if total > 0.0 {
                schedule.iter_mut().for_each(|at| *at *= transition / total);
            }
            let shown = schedule.iter().take_while(|at| **at <= t).count();
            let done_at = schedule.last().copied().unwrap_or(0.0);
            // Solid caret while typing, two blinks after, then gone.
            let after = t - done_at;
            let caret =
                shown < schedule.len() || (after < 1060.0 && (after / 530.0) as i32 % 2 == 0);
            look.reveal = Reveal::Chars { shown, caret };
        }
        Intro::Words => {
            let words = word_count(item.shown_text()).max(1);
            let gap = (transition / words as f32).max(1.0);
            let pop = gap;
            let progress = (0..words)
                .map(|i| q(((t - i as f32 * gap) / pop).clamp(0.0, 1.0)))
                .collect::<Vec<_>>();
            if progress.iter().any(|p| *p < 1.0) {
                look.reveal = Reveal::Words(progress);
            }
        }
    }

    let transition = item.outro_ms.clamp(50.0, 5000.0).min(dur / 2.0).max(1.0);
    match item.outro {
        Outro::None => {}
        Outro::Fade => look.alpha *= ease_out_cubic((left / transition).clamp(0.0, 1.0)),
        Outro::Pop => {
            let p = (left / transition).clamp(0.0, 1.0);
            look.scale *= 0.5 + 0.5 * ease_out_back(p);
            look.alpha *= (p * 2.5).min(1.0);
        }
        Outro::Slide => {
            let p = (left / transition).clamp(0.0, 1.0);
            look.dy += 0.6 * (1.0 - ease_out_cubic(p));
            look.alpha *= ease_out_cubic(p);
        }
    }

    look.alpha = q(look.alpha.clamp(0.0, 1.0));
    look.scale = q(look.scale.max(0.0));
    look.dy = q(look.dy);
    look
}

/// Per-word (alpha, scale) for a Words-intro progress value.
pub fn word_pop(progress: f32) -> (f32, f32) {
    if progress <= 0.0 {
        return (0.0, 0.0);
    }
    (
        (progress * 2.0).min(1.0),
        0.55 + 0.45 * ease_out_back(progress),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Type shows its first letter on frame one, so it is never blank.
    #[test]
    fn intros_start_blank_and_settle() {
        for intro in [Intro::Fade, Intro::Pop, Intro::Slide, Intro::Words] {
            let t = item(intro, Outro::None);
            assert!(look_at(&t, t.start_ms).is_blank(), "{intro:?}");
            assert!(
                !look_at(&t, (t.start_ms + t.end_ms) / 2).is_blank(),
                "{intro:?}"
            );
        }
        let t = item(Intro::None, Outro::None);
        assert!(!look_at(&t, t.start_ms).is_blank());
    }

    fn item(intro: Intro, outro: Outro) -> TextItem {
        let mut t = TextItem::new(TextId(1), crate::track::TrackId(0), 1_000, 5_000);
        t.text = "Hello there, world".into();
        t.intro = intro;
        t.outro = outro;
        t
    }

    #[test]
    fn intro_and_outro_timing_are_independent_and_reveals_follow_duration() {
        let mut t = item(Intro::Fade, Outro::Fade);
        t.intro_ms = 1000.0;
        t.outro_ms = 100.0;
        assert!(look_at(&t, 1500).alpha < 1.0);
        assert_eq!(look_at(&t, 4800).alpha, 1.0);
        assert!(look_at(&t, 4950).alpha < 1.0);
        t.intro = Intro::Typewriter;
        t.intro_ms = 1500.0;
        assert!(
            matches!(look_at(&t,2000).reveal, Reveal::Chars { shown, .. } if shown < t.text.chars().count())
        );
        assert!(
            matches!(look_at(&t,2500).reveal, Reveal::Chars { shown, .. } if shown == t.text.chars().count())
        );
    }

    #[test]
    fn settled_at_is_where_the_intro_has_finished() {
        for intro in Intro::ALL {
            let t = item(intro, Outro::Fade);
            let at = settled_at(&t);
            assert!(
                (t.start_ms..t.start_ms + t.duration_ms() / 2).contains(&at),
                "{intro:?}"
            );
            let look = look_at(&t, at);
            assert_eq!(look.alpha, 1.0, "{intro:?}");
            assert!(
                !matches!(look.reveal, Reveal::Chars { shown, .. } if shown < t.text.chars().count())
            );
        }
        assert_eq!(settled_at(&item(Intro::None, Outro::None)), 1_000);
    }

    #[test]
    fn transitions_start_hidden_and_settle() {
        for intro in [Intro::Fade, Intro::Pop, Intro::Slide] {
            let t = item(intro, Outro::None);
            assert!(look_at(&t, 1_000).alpha < 0.01, "{intro:?}");
            assert_eq!(look_at(&t, 2_000), Look::STILL, "{intro:?}");
        }
        for outro in [Outro::Fade, Outro::Pop, Outro::Slide] {
            let t = item(Intro::None, outro);
            assert_eq!(look_at(&t, 3_000), Look::STILL, "{outro:?}");
            assert!(look_at(&t, 4_999).alpha < 0.02, "{outro:?}");
        }
    }

    #[test]
    fn pop_overshoots_before_settling() {
        let t = item(Intro::Pop, Outro::None);
        let peak = (1_000..1_400)
            .step_by(10)
            .map(|ms| look_at(&t, ms).scale)
            .fold(0.0, f32::max);
        assert!(peak > 1.02, "peak {peak}");
    }

    #[test]
    fn typewriter_reveals_in_order_within_budget_and_drops_caret() {
        let t = item(Intro::Typewriter, Outro::None);
        let shown = |ms| match look_at(&t, ms).reveal {
            Reveal::Chars { shown, .. } => shown,
            other => panic!("{other:?}"),
        };
        let counts: Vec<_> = (1_000..5_000).step_by(50).map(shown).collect();
        assert!(counts.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(shown(1_000), 1);
        // Everything typed by 75 % of the layer.
        assert_eq!(shown(4_000), t.text.chars().count());
        assert!(matches!(
            look_at(&t, 4_990).reveal,
            Reveal::Chars { caret: false, .. }
        ));
    }

    #[test]
    fn typing_pauses_after_punctuation() {
        let times = typing_schedule("ab, cd", f32::MAX);
        let gap_after_comma = times[3] - times[2];
        let gap_plain = times[1] - times[0];
        assert!(gap_after_comma > gap_plain * 1.5);
    }

    #[test]
    fn words_appear_one_at_a_time_then_settle() {
        let t = item(Intro::Words, Outro::None);
        let Reveal::Words(early) = look_at(&t, 1_100).reveal else {
            panic!()
        };
        assert_eq!(early.len(), 3);
        assert!(early[0] > 0.0 && early[2] == 0.0);
        assert_eq!(look_at(&t, 4_000).reveal, Reveal::All);
    }

    #[test]
    fn short_layers_scale_transitions_down() {
        let mut t = item(Intro::Fade, Outro::Fade);
        t.end_ms = t.start_ms + 300;
        // Mid-point is fully visible even though Normal pace is 320 ms.
        assert_eq!(look_at(&t, t.start_ms + 150).alpha, 1.0);
    }
}
