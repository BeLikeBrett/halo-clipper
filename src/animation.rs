//! Small, per-segment camera moves. Preview and export share timing and
//! amplitudes; they never alter the clip's source time or attached audio.
use crate::track::Transform;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Effect {
    #[default]
    None,
    Fade,
    SnapZoom,
    PullBack,
    Whip,
    Flash,
}
impl Effect {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Fade,
        Self::SnapZoom,
        Self::PullBack,
        Self::Whip,
        Self::Flash,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Fade => "Fade",
            Self::SnapZoom => "Snap zoom",
            Self::PullBack => "Pull back",
            Self::Whip => "Whip",
            Self::Flash => "Flash",
        }
    }
    fn zoom(self) -> f32 {
        match self {
            Self::SnapZoom => 0.45,
            Self::PullBack => -0.2,
            Self::Whip => 0.5,
            _ => 0.0,
        }
    }
    fn fades(self) -> bool {
        matches!(self, Self::Fade | Self::PullBack | Self::Whip)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Animation {
    pub intro: Effect,
    pub outro: Effect,
    pub in_ms: f32,
    pub out_ms: f32,
    pub amount: f32,
}
impl Default for Animation {
    fn default() -> Self {
        Self {
            intro: Effect::None,
            outro: Effect::None,
            in_ms: 220.0,
            out_ms: 220.0,
            amount: 0.7,
        }
    }
}
impl Animation {
    pub fn active(self) -> bool {
        self.intro != Effect::None || self.outro != Effect::None
    }
    pub fn durations(self, duration_ms: i64) -> (f32, f32) {
        let half = (duration_ms.max(1) as f32 / 2.0).max(1.0);
        (
            self.in_ms.clamp(50.0, 5000.0).min(half),
            self.out_ms.clamp(50.0, 5000.0).min(half),
        )
    }
    /// Returns the animated transform and a single white-flash blend.
    pub fn sample(
        self,
        mut transform: Transform,
        local_ms: i64,
        duration_ms: i64,
    ) -> (Transform, f32) {
        let (a, b) = self.durations(duration_ms);
        let mut flash: f32 = 0.0;
        for (effect, progress, direction) in [
            (self.intro, (local_ms as f32 / a).clamp(0.0, 1.0), 1.0),
            (
                self.outro,
                ((duration_ms - local_ms) as f32 / b).clamp(0.0, 1.0),
                -1.0,
            ),
        ] {
            let remaining = 1.0 - progress;
            let weight = remaining.powi(3) * self.amount.clamp(0.0, 1.0);
            transform.scale *= 1.0 + effect.zoom() * weight;
            if effect == Effect::Whip {
                transform.x += direction * 0.12 * weight;
            }
            if effect.fades() {
                transform.opacity *= progress;
            }
            if effect == Effect::Flash {
                flash = flash.max(remaining * self.amount.clamp(0.0, 1.0));
            }
        }
        (transform, flash)
    }
    /// Filters run on clip-local timestamps, after speed and before timeline offset.
    pub fn filters(self, duration_ms: i64) -> String {
        if !self.active() {
            return String::new();
        }
        let (a, b) = self.durations(duration_ms);
        let duration = duration_ms as f32 / 1000.0;
        let mut filters = String::new();
        for (effect, start, length, way) in [
            (self.intro, 0.0, a / 1000.0, "in"),
            (self.outro, duration - b / 1000.0, b / 1000.0, "out"),
        ] {
            if effect.fades() {
                filters.push_str(&format!(
                    ",fade=t={way}:st={start:.6}:d={length:.6}:alpha=1"
                ));
            }
            if effect == Effect::Flash {
                // Mix a white fade with the original to expose the same Amount
                // control as preview, without bleaching the settled footage.
                let amount = self.amount.clamp(0.0, 1.0);
                let enable = if way == "in" {
                    format!("lt(t,{length:.6})")
                } else {
                    format!("gte(t,{start:.6})")
                };
                filters.push_str(&format!(",geq=r='r(X,Y)+(255-r(X,Y))*{amount:.6}*{}':g='g(X,Y)+(255-g(X,Y))*{amount:.6}*{}':b='b(X,Y)+(255-b(X,Y))*{amount:.6}*{}':a='alpha(X,Y)':enable='{enable}'", self.flash_expression(way, duration, length), self.flash_expression(way, duration, length), self.flash_expression(way, duration, length)));
            }
        }
        // geq negotiates planar RGB; restore packed RGBA while the size
        // is still fixed, before any animated scale. This also covers
        // mixed Flash + zoom edges without a downstream autoscaler.
        filters.push_str(",format=rgba");
        let zoom = self.zoom_expression("t", duration_ms);
        if self.intro.zoom() != 0.0 || self.outro.zoom() != 0.0 {
            filters.push_str(&format!(",scale=w='max(2,trunc(iw*({zoom})/2)*2)':h='max(2,trunc(ih*({zoom})/2)*2)':eval=frame"));
        }
        filters
    }
    fn flash_expression(self, way: &str, duration: f32, length: f32) -> String {
        if way == "in" {
            format!("(1-clip(T/{length:.6},0,1))")
        } else {
            format!("(1-clip(({duration:.6}-T)/{length:.6},0,1))")
        }
    }
    fn weights(self, time: &str, duration_ms: i64) -> (String, String) {
        let (a, b) = self.durations(duration_ms);
        let duration = duration_ms as f32 / 1000.0;
        (
            format!("pow(1-clip(({time})/{:.6},0,1),3)", a / 1000.0),
            format!(
                "pow(1-clip(({duration:.6}-({time}))/{:.6},0,1),3)",
                b / 1000.0
            ),
        )
    }
    fn zoom_expression(self, time: &str, duration_ms: i64) -> String {
        let (a, b) = self.weights(time, duration_ms);
        let amount = self.amount.clamp(0.0, 1.0);
        format!(
            "(1+{:.6}*{a})*(1+{:.6}*{b})",
            self.intro.zoom() * amount,
            self.outro.zoom() * amount
        )
    }
    /// Offset in canvas fractions. Overlay's t is on the whole timeline.
    pub fn x_expression(self, start_ms: i64, duration_ms: i64) -> String {
        if self.intro != Effect::Whip && self.outro != Effect::Whip {
            return "0".into();
        }
        let time = format!("t-{:.6}", start_ms as f32 / 1000.0);
        let (a, b) = self.weights(&time, duration_ms);
        let amount = self.amount.clamp(0.0, 1.0) * 0.12;
        format!(
            "{:.6}*{a}-{:.6}*{b}",
            if self.intro == Effect::Whip {
                amount
            } else {
                0.0
            },
            if self.outro == Effect::Whip {
                amount
            } else {
                0.0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_timing_and_short_segments_settle_without_overlap() {
        for effect in Effect::ALL {
            let a = Animation {
                intro: effect,
                outro: effect,
                in_ms: 900.0,
                out_ms: 100.0,
                ..Default::default()
            };
            assert_eq!(
                a.sample(Transform::default(), 1000, 2000),
                (Transform::default(), 0.0)
            );
            assert_eq!(
                a.sample(Transform::default(), 50, 100),
                (Transform::default(), 0.0)
            );
        }
    }
    #[test]
    fn effects_leave_base_transform_and_audio_timing_untouched() {
        let base = Transform {
            scale: 0.5,
            x: 0.3,
            opacity: 0.6,
            ..Default::default()
        };
        for effect in Effect::ALL {
            let a = Animation {
                intro: effect,
                ..Default::default()
            };
            assert_eq!(a.sample(base, 500, 1000), (base, 0.0));
        }
        let a = Animation {
            intro: Effect::Flash,
            amount: 0.4,
            ..Default::default()
        };
        assert_eq!(a.sample(base, 0, 1000), (base, 0.4));
    }
}
