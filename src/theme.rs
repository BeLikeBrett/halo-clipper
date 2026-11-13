// Color palette + global style setup. Centralizes the look so we can
// re-skin without grepping for hex values.

use egui::{Color32, Context, Rounding, Stroke, Visuals};

pub const BG_CANVAS: Color32 = Color32::from_rgb(14, 15, 18);
pub const BG_PANEL: Color32 = Color32::from_rgb(22, 24, 28);
pub const BG_HEADER: Color32 = Color32::from_rgb(18, 20, 24);
pub const BG_PREVIEW: Color32 = Color32::from_rgb(8, 9, 11);
pub const BG_STATUS: Color32 = Color32::from_rgb(28, 31, 36);
pub const BG_MODAL: Color32 = Color32::from_rgb(28, 31, 38);
pub const BG_BUTTON: Color32 = Color32::from_rgb(46, 50, 58);
pub const BG_BUTTON_HOVER: Color32 = Color32::from_rgb(60, 66, 78);
pub const BG_BUTTON_ACTIVE: Color32 = Color32::from_rgb(78, 86, 102);
pub const BG_BUTTON_DISABLED: Color32 = Color32::from_rgb(36, 38, 44);

pub const BORDER: Color32 = Color32::from_rgb(50, 54, 62);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 232, 238);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(140, 146, 158);

pub const ACCENT: Color32 = Color32::from_rgb(96, 168, 255);
pub const ACCENT_IN: Color32 = Color32::from_rgb(110, 220, 150);
pub const ACCENT_OUT: Color32 = Color32::from_rgb(240, 140, 140);
pub const ACCENT_OK: Color32 = Color32::from_rgb(110, 220, 150);
pub const ACCENT_ERR: Color32 = Color32::from_rgb(240, 120, 120);

/// Primary action buttons (Export Clip, Begin Export). Toned-down vs.
/// ACCENT so they don't glare against the dark UI — same hue family,
/// lower saturation, paired with a slightly off-white text tone.
pub const BTN_PRIMARY: Color32 = Color32::from_rgb(64, 112, 168);
pub const BTN_PRIMARY_HOVER: Color32 = Color32::from_rgb(80, 132, 196);
pub const BTN_PRIMARY_TEXT: Color32 = Color32::from_rgb(228, 236, 248);

pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_MODAL;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_rounding = Rounding::same(10.0);
    visuals.extreme_bg_color = Color32::from_rgb(10, 11, 14);
    visuals.faint_bg_color = Color32::from_rgb(30, 33, 38);
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);

    visuals.widgets.inactive.bg_fill = BG_BUTTON;
    visuals.widgets.inactive.weak_bg_fill = BG_BUTTON;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(6.0);

    visuals.widgets.hovered.bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.linear_multiply(0.7));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(6.0);

    visuals.widgets.active.bg_fill = BG_BUTTON_ACTIVE;
    visuals.widgets.active.weak_bg_fill = BG_BUTTON_ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.active.rounding = Rounding::same(6.0);

    visuals.widgets.open.bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.open.weak_bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.open.rounding = Rounding::same(6.0);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    // Default-disable text-cursor over labels. Selectable labels make
    // every text widget show an I-beam on hover, which competes with
    // the pointing-hand cursor on the button/card under it and feels
    // janky. Users can still copy text we explicitly opt-in (none
    // today). One global flip is cleaner than per-label opt-outs.
    style.interaction.selectable_labels = false;
    ctx.set_style(style);
}
