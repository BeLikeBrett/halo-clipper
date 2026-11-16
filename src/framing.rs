//! Canvas shape is independent of source quality. Preview and export share it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Aspect {
    #[default]
    Original,
    Wide,
    Vertical,
    Square,
    Feed,
}
impl Aspect {
    pub const ALL: [Self; 5] = [
        Self::Original,
        Self::Wide,
        Self::Vertical,
        Self::Square,
        Self::Feed,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::Wide => "Widescreen · 16:9",
            Self::Vertical => "Reels / TikTok · 9:16",
            Self::Square => "Square · 1:1",
            Self::Feed => "Portrait feed · 4:5",
        }
    }
    pub fn dimensions(self, w: u32, h: u32) -> (u32, u32) {
        let edge = w.min(h).max(2) as f64;
        let ratio = match self {
            Self::Original => return (w.max(2), h.max(2)),
            Self::Wide => (16.0, 9.0),
            Self::Vertical => (9.0, 16.0),
            Self::Square => (1.0, 1.0),
            Self::Feed => (4.0, 5.0),
        };
        let unit = edge / f64::min(ratio.0, ratio.1);
        let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
        (even(unit * ratio.0), even(unit * ratio.1))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Background {
    #[default]
    Black,
    Color([u8; 3]),
    Blur,
}
impl Background {
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Color(rgb) => rgb,
            _ => [0, 0, 0],
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CanvasSettings {
    pub aspect: Aspect,
    pub background: Background,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fit {
    #[default]
    Fit,
    Fill,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_keep_native_source_quality_and_even_dimensions() {
        assert_eq!(Aspect::Original.dimensions(3840, 2160), (3840, 2160));
        assert_eq!(Aspect::Vertical.dimensions(3840, 2160), (2160, 3840));
        assert_eq!(Aspect::Feed.dimensions(1920, 1080), (1080, 1350));
        assert_eq!(Aspect::Square.dimensions(1920, 1080), (1080, 1080));
    }
}
