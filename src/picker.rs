//! Compact color picker for drop panels: saturation/brightness square, hue
//! strip, preset swatches and a hex field. Replaces egui's stock picker,
//! whose U8/RGB button row and fixed layout don't fit a toolbar popover.

use crate::theme;
use egui::{ecolor::HsvaGamma, pos2, vec2, Color32, Mesh, Rect, Sense, Stroke, Ui};

/// Swatches covering the usual canvas backgrounds: neutrals first, then a
/// few muted colors that sit well behind footage.
const PRESETS: [[u8; 3]; 9] = [
    [0, 0, 0],
    [24, 26, 31],
    [58, 62, 72],
    [245, 245, 245],
    [46, 74, 112],
    [30, 82, 88],
    [112, 62, 140],
    [168, 52, 60],
    [214, 150, 64],
];

/// Grid resolution of the saturation/value mesh. The HSV→RGB map is
/// bilinear per hue, so a coarse grid is already visually exact.
const GRID: usize = 8;

/// Non-focusable drag sense: clicking the picker must not steal keyboard
/// focus from the editor's shortcuts.
const PICK: Sense = Sense {
    click: true,
    drag: true,
    focusable: false,
};

fn rgb_of(c: HsvaGamma) -> [u8; 3] {
    let c = Color32::from(c);
    [c.r(), c.g(), c.b()]
}

/// Parses `#rrggbb` / `rrggbb` (case-insensitive).
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

pub fn format_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Persistent picker state: the HSV value (kept so hue survives dragging
/// through black or grey) and the hex field's in-progress text.
#[derive(Clone, Debug)]
pub struct PickerState {
    pub color: HsvaGamma,
    pub hex: String,
}

impl PickerState {
    pub fn new(rgb: [u8; 3]) -> Self {
        let mut state = Self {
            color: HsvaGamma::default(),
            hex: String::new(),
        };
        state.set_rgb(rgb);
        state
    }

    pub fn rgb(&self) -> [u8; 3] {
        rgb_of(self.color)
    }

    /// Adopts an external color unless it is what we already show (so a
    /// round-trip through RGB doesn't snap the hue of a grey to red).
    pub fn set_rgb(&mut self, rgb: [u8; 3]) {
        if self.rgb() != rgb {
            self.color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]).into();
        }
        self.hex = format_hex(rgb);
    }
}

/// Draws the picker at the available width. Returns true when the color
/// changed this frame.
pub fn show(ui: &mut Ui, state: &mut PickerState) -> bool {
    let before = state.rgb();
    let width = ui.available_width();

    // Saturation (x) / value (y) square.
    let (sv_rect, sv) = ui.allocate_exact_size(vec2(width, 132.0), PICK);
    let mut mesh = Mesh::default();
    for row in 0..=GRID {
        for col in 0..=GRID {
            let s = col as f32 / GRID as f32;
            let v = 1.0 - row as f32 / GRID as f32;
            mesh.colored_vertex(
                pos2(
                    sv_rect.left() + s * sv_rect.width(),
                    sv_rect.top() + (1.0 - v) * sv_rect.height(),
                ),
                HsvaGamma {
                    s,
                    v,
                    a: 1.0,
                    ..state.color
                }
                .into(),
            );
        }
    }
    let stride = (GRID + 1) as u32;
    for row in 0..GRID as u32 {
        for col in 0..GRID as u32 {
            let i = row * stride + col;
            mesh.add_triangle(i, i + 1, i + stride);
            mesh.add_triangle(i + 1, i + stride + 1, i + stride);
        }
    }
    ui.painter().add(mesh);
    ui.painter()
        .rect_stroke(sv_rect, 4.0, Stroke::new(1.0_f32, theme::BORDER));
    if let Some(p) = sv
        .interact_pointer_pos()
        .filter(|_| sv.is_pointer_button_down_on())
    {
        state.color.s = ((p.x - sv_rect.left()) / sv_rect.width()).clamp(0.0, 1.0);
        state.color.v = 1.0 - ((p.y - sv_rect.top()) / sv_rect.height()).clamp(0.0, 1.0);
    }
    let knob = pos2(
        sv_rect.left() + state.color.s * sv_rect.width(),
        sv_rect.top() + (1.0 - state.color.v) * sv_rect.height(),
    );
    ui.painter()
        .circle_stroke(knob, 6.0, Stroke::new(2.0_f32, Color32::WHITE));
    ui.painter().circle_stroke(
        knob,
        7.5,
        Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
    );
    sv.on_hover_cursor(egui::CursorIcon::Crosshair);

    ui.add_space(8.0);

    // Hue strip.
    let (hue_rect, hue) = ui.allocate_exact_size(vec2(width, 12.0), PICK);
    let mut strip = Mesh::default();
    const STEPS: usize = 24;
    for i in 0..=STEPS {
        let h = i as f32 / STEPS as f32;
        let x = hue_rect.left() + h * hue_rect.width();
        let color: Color32 = HsvaGamma {
            h,
            s: 1.0,
            v: 1.0,
            a: 1.0,
        }
        .into();
        strip.colored_vertex(pos2(x, hue_rect.top()), color);
        strip.colored_vertex(pos2(x, hue_rect.bottom()), color);
    }
    for i in 0..STEPS as u32 {
        let k = i * 2;
        strip.add_triangle(k, k + 1, k + 2);
        strip.add_triangle(k + 1, k + 3, k + 2);
    }
    ui.painter().add(strip);
    if let Some(p) = hue
        .interact_pointer_pos()
        .filter(|_| hue.is_pointer_button_down_on())
    {
        state.color.h = ((p.x - hue_rect.left()) / hue_rect.width()).clamp(0.0, 0.9999);
    }
    let hx = hue_rect.left() + state.color.h * hue_rect.width();
    let thumb = Rect::from_center_size(pos2(hx, hue_rect.center().y), vec2(6.0, 18.0));
    ui.painter().rect_filled(thumb, 3.0, Color32::WHITE);
    ui.painter().rect_stroke(
        thumb,
        3.0,
        Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
    );
    hue.on_hover_cursor(egui::CursorIcon::PointingHand);

    ui.add_space(10.0);

    // Presets.
    let current = state.rgb();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let size = width / PRESETS.len() as f32;
        for rgb in PRESETS {
            let (cell, response) = ui.allocate_exact_size(vec2(size, 24.0), PICK);
            let fill = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            let r = 9.0;
            ui.painter().circle_filled(cell.center(), r, fill);
            ui.painter().circle_stroke(
                cell.center(),
                r,
                Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
            );
            if rgb == current {
                ui.painter().circle_stroke(
                    cell.center(),
                    r + 3.0,
                    Stroke::new(1.5_f32, theme::ACCENT),
                );
            } else if response.hovered() {
                ui.painter().circle_stroke(
                    cell.center(),
                    r + 3.0,
                    Stroke::new(1.0_f32, theme::TEXT_MUTED),
                );
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.set_rgb(rgb);
            }
        }
    });

    ui.add_space(8.0);

    // Hex field with a live chip.
    ui.horizontal(|ui| {
        let (chip, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
        let rgb = state.rgb();
        ui.painter()
            .rect_filled(chip, 5.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        ui.painter()
            .rect_stroke(chip, 5.0, Stroke::new(1.0_f32, theme::BORDER));
        let edit = ui.add(
            egui::TextEdit::singleline(&mut state.hex)
                .font(theme::mono(12.0))
                .desired_width(84.0)
                .char_limit(7),
        );
        if edit.changed() {
            if let Some(rgb) = parse_hex(&state.hex) {
                let text = std::mem::take(&mut state.hex);
                state.set_rgb(rgb);
                state.hex = text;
            }
        }
        // While typing, keep the user's partial text; otherwise mirror the
        // color so drags and swatches show up in the field.
        if !edit.has_focus() {
            state.hex = format_hex(state.rgb());
        }
    });

    state.rgb() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(parse_hex("#1A2b3C"), Some([0x1A, 0x2B, 0x3C]));
        assert_eq!(parse_hex("ffffff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#12345g"), None);
        assert_eq!(parse_hex("#éé1234"), None);
        assert_eq!(format_hex([0x1A, 0x2B, 0x3C]), "#1A2B3C");
    }

    #[test]
    fn adopting_the_shown_color_keeps_the_hue() {
        let mut state = PickerState::new([0, 0, 0]);
        state.color.h = 0.6;
        state.set_rgb([0, 0, 0]);
        assert!((state.color.h - 0.6).abs() < f32::EPSILON);
        state.set_rgb([255, 0, 0]);
        assert_eq!(state.rgb(), [255, 0, 0]);
    }
}
