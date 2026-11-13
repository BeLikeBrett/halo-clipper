//! Color palette, typography and global style. Centralizes the look so the
//! app can be re-skinned without grepping for hex values.
//!
//! The palette is a cool graphite ramp: each surface step is a few points
//! lighter than the one beneath it, so depth reads from value alone and
//! borders can stay faint.

use egui::{Color32, Context, FontFamily, FontId, Rounding, Shadow, Stroke, Visuals};

// ── Surfaces (darkest → lightest) ───────────────────────────────────────
pub const BG_PREVIEW: Color32 = Color32::from_rgb(9, 10, 12);
pub const BG_CANVAS: Color32 = Color32::from_rgb(13, 14, 17);
pub const BG_HEADER: Color32 = Color32::from_rgb(16, 17, 20);
pub const BG_PANEL: Color32 = Color32::from_rgb(19, 20, 24);
pub const BG_STATUS: Color32 = Color32::from_rgb(25, 27, 32);
pub const BG_MODAL: Color32 = Color32::from_rgb(26, 28, 33);
pub const BG_BUTTON: Color32 = Color32::from_rgb(31, 34, 40);
pub const BG_BUTTON_HOVER: Color32 = Color32::from_rgb(40, 44, 52);
pub const BG_BUTTON_ACTIVE: Color32 = Color32::from_rgb(48, 54, 65);
pub const BG_BUTTON_DISABLED: Color32 = Color32::from_rgb(27, 29, 34);

pub const BORDER: Color32 = Color32::from_rgb(42, 45, 53);
/// Hairlines inside a surface (row separators, ruler ticks).
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(30, 32, 38);

// ── Text ────────────────────────────────────────────────────────────────
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 232, 237);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(170, 175, 186);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(122, 128, 140);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(82, 87, 98);

// ── Accents ─────────────────────────────────────────────────────────────
pub const ACCENT: Color32 = Color32::from_rgb(98, 160, 255);
pub const ACCENT_IN: Color32 = Color32::from_rgb(110, 214, 150);
pub const ACCENT_OUT: Color32 = Color32::from_rgb(238, 132, 132);
pub const ACCENT_OK: Color32 = Color32::from_rgb(110, 214, 150);
pub const ACCENT_ERR: Color32 = Color32::from_rgb(238, 116, 116);
/// Timeline playhead. A soft white: the brightest mark on the timeline
/// without adding a fourth hue next to the blue accent and the clip fills.
pub const PLAYHEAD: Color32 = Color32::from_rgb(236, 239, 245);

// ── Timeline clip fills ─────────────────────────────────────────────────
pub const CLIP_VIDEO: Color32 = Color32::from_rgb(46, 74, 112);
pub const CLIP_VIDEO_EDGE: Color32 = Color32::from_rgb(78, 116, 166);
/// Teal sits next to the video blue on the colour wheel, so the two lanes
/// read as one family while staying distinguishable at a glance.
pub const CLIP_AUDIO: Color32 = Color32::from_rgb(30, 80, 88);
pub const CLIP_AUDIO_EDGE: Color32 = Color32::from_rgb(58, 132, 140);

/// Text layer bars on the timeline.
pub const CLIP_TEXT: Color32 = Color32::from_rgb(70, 58, 128);
pub const CLIP_TEXT_EDGE: Color32 = Color32::from_rgb(118, 100, 200);

/// Primary action buttons (Export, Begin Export). Same hue family as
/// ACCENT, lower saturation, so the call to action doesn't glare.
pub const BTN_PRIMARY: Color32 = Color32::from_rgb(58, 108, 176);
pub const BTN_PRIMARY_HOVER: Color32 = Color32::from_rgb(72, 126, 200);
pub const BTN_PRIMARY_TEXT: Color32 = Color32::from_rgb(240, 245, 252);

// ── Typography ──────────────────────────────────────────────────────────
// Font-data keys registered in `main.rs`. egui has no synthetic bold, so
// each weight is its own family.
pub const FONT_REGULAR: &str = "Inter";
pub const FONT_MEDIUM: &str = "Inter Medium";
pub const FONT_SEMIBOLD: &str = "Inter SemiBold";
pub const FONT_MONO: &str = "JetBrains Mono";

/// Inter Medium — buttons, labels that need presence without shouting.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_MEDIUM.into()))
}

/// Inter SemiBold — section titles and the active state of controls.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_SEMIBOLD.into()))
}

/// Inter Regular.
pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// JetBrains Mono — timecodes and key caps.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn apply(ctx: &Context) {
    // The app is dark-only. egui otherwise follows the OS preference, and
    // on a light-mode Windows (or GNOME) desktop the stock widgets would
    // switch to the light style while the custom surfaces stay dark.
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_MODAL;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_rounding = Rounding::same(10.0);
    visuals.menu_rounding = Rounding::same(8.0);
    visuals.window_shadow = Shadow {
        offset: egui::vec2(0.0, 12.0),
        blur: 32.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };
    visuals.popup_shadow = Shadow {
        offset: egui::vec2(0.0, 8.0),
        blur: 20.0,
        spread: 0.0,
        color: Color32::from_black_alpha(110),
    };
    visuals.extreme_bg_color = Color32::from_rgb(11, 12, 15);
    visuals.faint_bg_color = Color32::from_rgb(28, 30, 36);
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.32);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;

    let rounding = Rounding::same(6.0);
    let edge = Color32::from_rgb(46, 50, 59);

    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = rounding;

    visuals.widgets.inactive.bg_fill = BG_BUTTON;
    visuals.widgets.inactive.weak_bg_fill = BG_BUTTON;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, edge);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_SECONDARY);
    visuals.widgets.inactive.rounding = rounding;

    visuals.widgets.hovered.bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(64, 70, 82));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = BG_BUTTON_ACTIVE;
    visuals.widgets.active.weak_bg_fill = BG_BUTTON_ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.8));
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.active.rounding = rounding;
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.open.weak_bg_fill = BG_BUTTON_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(64, 70, 82));
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.open.rounding = rounding;

    ctx.set_visuals_of(egui::Theme::Dark, visuals);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(11.0, 5.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.slider_rail_height = 4.0;
    style.text_styles = [
        (egui::TextStyle::Body, regular(13.0)),
        (egui::TextStyle::Button, medium(12.5)),
        (egui::TextStyle::Small, regular(11.0)),
        (egui::TextStyle::Heading, semibold(15.0)),
        (egui::TextStyle::Monospace, mono(12.0)),
    ]
    .into();
    // Labels are not selectable: an I-beam over every caption competes with
    // the pointing hand of the control underneath it.
    style.interaction.selectable_labels = false;
    ctx.set_style_of(egui::Theme::Dark, style);
}
