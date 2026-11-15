//! Painted controls shared across panels: vector icons, icon buttons, the
//! top-bar drop-down menus and the segmented mode switch.
//!
//! Everything draws with the painter instead of glyphs, so icon weight and
//! alignment never depend on which fallback font happens to carry "🔊".

use crate::theme;
use egui::{
    epaint::PathStroke, pos2, vec2, Align2, Color32, Id, Pos2, Rect, Response, Rounding, Sense,
    Shape, Stroke, Ui,
};
use std::f32::consts::FRAC_PI_4;

/// Click sense that never takes keyboard focus. Painted controls use it so
/// pressing one (play, split, a menu row) leaves Space/S/arrow shortcuts
/// working instead of routing keys to the button.
pub const CLICK: Sense = Sense {
    click: true,
    drag: false,
    focusable: false,
};

/// Vector icons. Each is drawn inside a square and scales with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipStart,
    Speaker,
    SpeakerMuted,
    ChevronDown,
    Check,
    Close,
}

/// Points along a circular arc, for the speaker's sound waves.
fn arc(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    const STEPS: usize = 10;
    (0..=STEPS)
        .map(|i| {
            let a = from + (to - from) * i as f32 / STEPS as f32;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// Draws `icon` centered in `rect` (only the shorter side is used).
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let stroke = Stroke::new((s * 0.09).clamp(1.2, 2.0), color);
    match icon {
        Icon::Play => {
            // Optical centering: a triangle's mass sits left of its box.
            let o = s * 0.05;
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - s * 0.26 + o, c.y - s * 0.32),
                    pos2(c.x + s * 0.32 + o, c.y),
                    pos2(c.x - s * 0.26 + o, c.y + s * 0.32),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for dx in [-0.17, 0.17] {
                painter.rect_filled(
                    Rect::from_center_size(pos2(c.x + s * dx, c.y), vec2(s * 0.17, s * 0.6)),
                    s * 0.04,
                    color,
                );
            }
        }
        Icon::SkipStart => {
            painter.rect_filled(
                Rect::from_center_size(pos2(c.x - s * 0.27, c.y), vec2(s * 0.1, s * 0.52)),
                1.0,
                color,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x + s * 0.3, c.y - s * 0.28),
                    pos2(c.x + s * 0.3, c.y + s * 0.28),
                    pos2(c.x - s * 0.18, c.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Speaker | Icon::SpeakerMuted => {
            let body = Rect::from_min_max(
                pos2(c.x - s * 0.40, c.y - s * 0.13),
                pos2(c.x - s * 0.20, c.y + s * 0.13),
            );
            painter.rect_filled(body, 1.0, color);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - s * 0.22, c.y - s * 0.13),
                    pos2(c.x + s * 0.02, c.y - s * 0.34),
                    pos2(c.x + s * 0.02, c.y + s * 0.34),
                    pos2(c.x - s * 0.22, c.y + s * 0.13),
                ],
                color,
                Stroke::NONE,
            ));
            if icon == Icon::Speaker {
                let origin = pos2(c.x + s * 0.02, c.y);
                for radius in [0.17, 0.31] {
                    painter.add(Shape::line(
                        arc(origin, s * radius, -FRAC_PI_4, FRAC_PI_4),
                        PathStroke::new(stroke.width, color),
                    ));
                }
            } else {
                let x = pos2(c.x + s * 0.27, c.y);
                let r = s * 0.12;
                painter.line_segment([x + vec2(-r, -r), x + vec2(r, r)], stroke);
                painter.line_segment([x + vec2(-r, r), x + vec2(r, -r)], stroke);
            }
        }
        Icon::ChevronDown => {
            let r = s * 0.22;
            painter.add(Shape::line(
                vec![
                    pos2(c.x - r, c.y - r * 0.45),
                    pos2(c.x, c.y + r * 0.55),
                    pos2(c.x + r, c.y - r * 0.45),
                ],
                PathStroke::new(stroke.width, color),
            ));
        }
        Icon::Check => {
            painter.add(Shape::line(
                vec![
                    pos2(c.x - s * 0.28, c.y + s * 0.02),
                    pos2(c.x - s * 0.08, c.y + s * 0.22),
                    pos2(c.x + s * 0.3, c.y - s * 0.2),
                ],
                PathStroke::new(stroke.width, color),
            ));
        }
        Icon::Close => {
            let r = s * 0.22;
            painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        }
    }
}

/// Square icon button: transparent at rest, soft fill on hover. `on`
/// tints the icon with `on_color` (e.g. a muted track).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, on: bool, on_color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), CLICK);
    paint_icon_button(ui, rect, &response, icon, on, on_color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Destination highlight for files dragged in from outside: a faint tint,
/// a hairline accent border and, optionally, a centered label with a
/// painted "drop" glyph. `fade` (0..1) animates it in and out.
pub fn paint_drop_zone(painter: &egui::Painter, rect: egui::Rect, fade: f32, label: Option<&str>) {
    if fade <= 0.0 {
        return;
    }
    painter.rect_filled(rect, 8.0, theme::ACCENT.gamma_multiply(0.07 * fade));
    painter.rect_stroke(
        rect.shrink(0.5),
        8.0,
        Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.55 * fade)),
    );
    let Some(label) = label else { return };
    let galley = painter.layout_no_wrap(label.to_owned(), theme::medium(12.5), theme::TEXT_PRIMARY);
    // A pill when it fits; in a narrow zone (the Media column) a round
    // badge with the label centered under it, so nothing pokes out.
    let stacked = galley.size().x + 52.0 > rect.width() - 20.0;
    let (c, text_pos) = if stacked {
        let badge = rect.center() - egui::vec2(0.0, 14.0);
        painter.circle_filled(badge, 22.0, theme::BG_MODAL.gamma_multiply(fade));
        painter.circle_stroke(
            badge,
            22.0,
            Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.35 * fade)),
        );
        (
            badge,
            egui::pos2(
                rect.center().x - galley.size().x * 0.5,
                badge.y + 22.0 + 10.0,
            ),
        )
    } else {
        let pill =
            egui::Rect::from_center_size(rect.center(), egui::vec2(galley.size().x + 52.0, 36.0));
        painter.rect_filled(pill, 18.0, theme::BG_MODAL.gamma_multiply(fade));
        painter.rect_stroke(
            pill,
            18.0,
            Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.35 * fade)),
        );
        (
            egui::pos2(pill.left() + 22.0, pill.center().y),
            egui::pos2(pill.left() + 38.0, pill.center().y - galley.size().y * 0.5),
        )
    };
    // Arrow into a tray.
    let stroke = Stroke::new(1.6_f32, theme::ACCENT.gamma_multiply(fade));
    painter.line_segment(
        [c + egui::vec2(0.0, -7.0), c + egui::vec2(0.0, 2.0)],
        stroke,
    );
    painter.line_segment(
        [c + egui::vec2(-3.5, -1.5), c + egui::vec2(0.0, 2.0)],
        stroke,
    );
    painter.line_segment(
        [c + egui::vec2(3.5, -1.5), c + egui::vec2(0.0, 2.0)],
        stroke,
    );
    painter.add(egui::Shape::line(
        vec![
            c + egui::vec2(-6.5, 2.5),
            c + egui::vec2(-6.5, 6.5),
            c + egui::vec2(6.5, 6.5),
            c + egui::vec2(6.5, 2.5),
        ],
        stroke,
    ));
    painter.galley(text_pos, galley, theme::TEXT_PRIMARY.gamma_multiply(fade));
}

/// Paints an icon button over an already-allocated rect (for buttons that
/// live inside custom-painted surfaces such as track headers).
pub fn paint_icon_button(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    icon: Icon,
    on: bool,
    on_color: Color32,
) {
    let hovered = response.hovered();
    let fill = if response.is_pointer_button_down_on() {
        theme::BG_BUTTON_ACTIVE
    } else if hovered {
        theme::BG_BUTTON_HOVER
    } else if on {
        on_color.gamma_multiply(0.14)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 5.0, fill);
    let color = if on {
        on_color
    } else if hovered {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    paint_icon(ui.painter(), rect.shrink(rect.width() * 0.2), icon, color);
}

/// Small "▶ Preview" pill for playing back an effect in place.
pub fn preview_button(ui: &mut Ui) -> Response {
    let font = theme::medium(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap("Preview".to_owned(), font, theme::TEXT_PRIMARY);
    let size = vec2(galley.size().x + 34.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, CLICK);
    let hot = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let fill = if pressed {
        theme::BG_BUTTON_ACTIVE
    } else if hot {
        theme::BG_BUTTON_HOVER
    } else {
        theme::BG_BUTTON
    };
    ui.painter().rect_filled(rect, 12.0, fill);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        12.0,
        Stroke::new(
            1.0_f32,
            if hot {
                theme::ACCENT.gamma_multiply(0.7)
            } else {
                theme::BORDER
            },
        ),
    );
    let icon = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(10.0, 10.0));
    paint_icon(
        ui.painter(),
        icon,
        Icon::Play,
        if hot {
            theme::ACCENT
        } else {
            theme::TEXT_SECONDARY
        },
    );
    ui.painter().galley(
        pos2(rect.left() + 24.0, rect.center().y - galley.size().y * 0.5),
        galley,
        if hot {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The circular transport button: filled, the one strong shape in the bar.
pub fn play_button(ui: &mut Ui, playing: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), CLICK);
    let fill = if response.is_pointer_button_down_on() {
        Color32::from_rgb(205, 210, 220)
    } else if response.hovered() {
        Color32::WHITE
    } else {
        theme::TEXT_PRIMARY
    };
    ui.painter().circle_filled(rect.center(), 16.0, fill);
    paint_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(15.0, 15.0)),
        if playing { Icon::Pause } else { Icon::Play },
        theme::BG_PREVIEW,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ── Drop-down panels ────────────────────────────────────────────────────

/// Which top-bar menu is open, shared so hovering a neighbour while one is
/// open slides straight across like a native menu bar.
fn open_menu_id() -> Id {
    Id::new("halo_menu_bar_open")
}

/// Cubic ease-out: fast start, gentle landing.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Open/close progress of the panel owned by `id`, eased (0 closed, 1 open).
pub fn reveal(ctx: &egui::Context, id: Id, open: bool) -> f32 {
    ease_out(ctx.animate_bool_with_time(id.with("reveal"), open, 0.16))
}

/// Fill and rounding for a button that owns a drop panel. While the panel
/// is out, the button takes the panel's colour and loses its lower
/// corners, so button and panel read as one surface.
pub fn trigger_style(reveal: f32, hovered: bool, rest: Color32) -> (Color32, Rounding) {
    let rounding = Rounding {
        nw: 6.0,
        ne: 6.0,
        sw: 6.0 * (1.0 - reveal),
        se: 6.0 * (1.0 - reveal),
    };
    let fill = if reveal > 0.0 {
        lerp_color(theme::BG_BUTTON_HOVER, theme::BG_MODAL, reveal)
    } else if hovered {
        theme::BG_BUTTON_HOVER
    } else {
        rest
    };
    (fill, rounding)
}

/// Shows the panel that unrolls from under `trigger` while `reveal > 0`.
/// The panel aligns to the trigger's left edge, or to its right edge when
/// it would run off-screen; the corner touching the trigger stays square.
///
/// `add_contents` returns `true` when a choice was made. The function
/// returns `true` when the panel should close: a choice, a click outside
/// both panel and trigger, or Escape.
pub fn drop_panel(
    ctx: &egui::Context,
    id: Id,
    trigger: Rect,
    open: bool,
    reveal: f32,
    width: f32,
    add_contents: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    if reveal <= 0.0 {
        return false;
    }
    let width = width.max(trigger.width());
    let height_id = id.with("height");
    let full_height = ctx.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(120.0);
    let flip = trigger.left() + width > ctx.screen_rect().right() - 8.0;
    let top = if flip {
        pos2(trigger.right() - width, trigger.bottom())
    } else {
        trigger.left_bottom()
    };
    let mut chosen = false;
    let area = egui::Area::new(id.with("panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(top)
        .constrain(false)
        .interactable(open)
        .show(ctx, |ui| {
            // Unroll from the trigger edge: the clip grows downward while
            // the content fades in.
            ui.set_clip_rect(Rect::from_min_max(
                top - vec2(40.0, 0.0),
                pos2(
                    top.x + width + 40.0,
                    top.y + full_height * reveal + 40.0 * reveal,
                ),
            ));
            ui.multiply_opacity(0.35 + 0.65 * reveal);
            let corner = |square: bool| if square { 0.0 } else { 8.0 };
            let frame = egui::Frame::none()
                .fill(theme::BG_MODAL)
                .rounding(Rounding {
                    nw: corner(!flip),
                    ne: corner(flip),
                    sw: 8.0,
                    se: 8.0,
                })
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::same(6.0));
            let inner = frame.show(ui, |ui| {
                ui.set_width(width - 12.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                add_contents(ui)
            });
            chosen = inner.inner;
            inner.response.rect
        });
    let panel = area.inner;
    if (panel.height() - full_height).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(height_id, panel.height()));
        ctx.request_repaint();
    }
    let clicked_elsewhere = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !panel.contains(p) && !trigger.contains(p))
    });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    open && (chosen || clicked_elsewhere || escape)
}

/// The app menu in the top-left corner: the Halo ring and the app name as
/// one button. `badge` adds a small accent dot (an update is ready).
pub fn app_menu_button(
    ui: &mut Ui,
    badge: bool,
    min_popup_width: f32,
    add_contents: impl FnOnce(&mut Ui) -> bool,
) {
    let galley = ui.painter().layout_no_wrap(
        "Halo Clipper".to_owned(),
        theme::semibold(13.5),
        theme::TEXT_PRIMARY,
    );
    let size = vec2(10.0 + 18.0 + 9.0 + galley.size().x + 22.0, 34.0);
    menu_trigger(
        ui,
        "app_menu",
        size,
        move |painter, rect, color| {
            let ring = pos2(rect.left() + 10.0 + 9.0, rect.center().y);
            paint_logo(painter, ring);
            if badge {
                let dot = ring + vec2(7.0, -7.0);
                painter.circle_filled(dot, 3.5, theme::ACCENT);
                painter.circle_stroke(dot, 3.5, Stroke::new(1.5_f32, theme::BG_HEADER));
            }
            let text_left = ring.x + 9.0 + 9.0;
            let text_right = text_left + galley.size().x;
            painter.galley(
                pos2(text_left, rect.center().y - galley.size().y * 0.5),
                galley,
                color,
            );
            // Small chevron: this opens a menu.
            let c = pos2(text_right + 10.0, rect.center().y + 1.5);
            let stroke = Stroke::new(1.4_f32, color.gamma_multiply(0.7));
            painter.line_segment([c + vec2(-3.5, -2.5), c], stroke);
            painter.line_segment([c, c + vec2(3.5, -2.5)], stroke);
        },
        min_popup_width,
        add_contents,
    );
}

/// The Halo ring: a faint circle with a bright arc.
pub fn paint_logo(painter: &egui::Painter, c: Pos2) {
    painter.circle_stroke(
        c,
        7.5,
        Stroke::new(2.0_f32, theme::ACCENT.gamma_multiply(0.35)),
    );
    let arc: Vec<_> = (0..=14)
        .map(|i| {
            let a = -2.4 + 1.9 * i as f32 / 14.0;
            c + vec2(a.cos(), a.sin()) * 7.5
        })
        .collect();
    painter.add(egui::Shape::line(
        arc,
        PathStroke::new(2.0_f32, theme::ACCENT),
    ));
}

/// A menu-bar trigger of `size` whose face `paint` draws; opens a drop
/// panel with `add_contents` (true = an item was chosen, close it).
fn menu_trigger(
    ui: &mut Ui,
    key: &str,
    size: egui::Vec2,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
    min_popup_width: f32,
    add_contents: impl FnOnce(&mut Ui) -> bool,
) {
    let id = ui.id().with(("menu_bar", key));
    let ctx = ui.ctx().clone();
    let (rect, response) = ui.allocate_exact_size(size, CLICK);
    let open_now = ctx.data(|d| d.get_temp::<Id>(open_menu_id())) == Some(id);

    let mut open = open_now;
    if response.clicked() {
        open = !open_now;
    } else if response.hovered() && !open_now {
        // Menu-bar convention: once any menu is open, hovering a sibling
        // switches to it.
        open = ctx.data(|d| d.get_temp::<Id>(open_menu_id())).is_some();
    }
    if open != open_now {
        ctx.data_mut(|d| {
            if open {
                d.insert_temp(open_menu_id(), id);
            } else {
                d.remove::<Id>(open_menu_id());
            }
        });
    }

    let t = reveal(&ctx, id, open);
    let (fill, rounding) = trigger_style(t, response.hovered(), Color32::TRANSPARENT);
    ui.painter().rect_filled(rect, rounding, fill);
    paint(
        ui.painter(),
        rect,
        if open || response.hovered() {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if drop_panel(&ctx, id, rect, open, t, min_popup_width, add_contents) {
        ctx.data_mut(|d| d.remove::<Id>(open_menu_id()));
        ctx.request_repaint();
    }
}

/// Toolbar button that owns a drop panel. `paint_face` draws the button's
/// content (label, swatch…) into the given rect in the given text colour.
/// Returns the trigger response; `open` is toggled on click and cleared
/// when the panel closes.
#[allow(clippy::too_many_arguments)]
pub fn dropdown_button(
    ui: &mut Ui,
    id_salt: &str,
    open: &mut bool,
    size: egui::Vec2,
    panel_width: f32,
    paint_face: impl FnOnce(&egui::Painter, Rect, Color32),
    add_contents: impl FnOnce(&mut Ui) -> bool,
) -> Response {
    let id = ui.id().with(("dropdown", id_salt));
    let (rect, response) = ui.allocate_exact_size(size, CLICK);
    if response.clicked() {
        *open = !*open;
    }
    let t = reveal(ui.ctx(), id, *open);
    let (fill, rounding) = trigger_style(t, response.hovered(), theme::BG_BUTTON);
    ui.painter().rect_filled(rect, rounding, fill);
    if t <= 0.0 {
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            rounding,
            Stroke::new(1.0_f32, Color32::from_rgb(46, 50, 59)),
        );
    }
    let text = if *open || response.hovered() {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let chevron = Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), vec2(14.0, 14.0));
    paint_icon(ui.painter(), chevron, Icon::ChevronDown, theme::TEXT_MUTED);
    paint_face(
        ui.painter(),
        Rect::from_min_max(
            rect.min + vec2(10.0, 0.0),
            pos2(chevron.left() - 4.0, rect.bottom()),
        ),
        text,
    );
    if drop_panel(ui.ctx(), id, rect, *open, t, panel_width, add_contents) {
        *open = false;
        ui.ctx().request_repaint();
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// One row in a drop-down: label on the left, optional key hint on the
/// right, full-width hover highlight.
pub fn menu_item(ui: &mut Ui, label: &str, shortcut: Option<&str>) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), CLICK);
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, theme::BG_BUTTON_HOVER);
    }
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        theme::medium(12.5),
        theme::TEXT_PRIMARY,
    );
    if let Some(keys) = shortcut {
        ui.painter().text(
            rect.right_center() - vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            keys,
            theme::regular(11.5),
            theme::TEXT_MUTED,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A drop-down row for picking one of several values; the current one
/// carries a check mark.
pub fn menu_choice(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), CLICK);
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, theme::BG_BUTTON_HOVER);
    }
    if selected {
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.left_center() + vec2(15.0, 0.0), vec2(14.0, 14.0)),
            Icon::Check,
            theme::ACCENT,
        );
    }
    ui.painter().text(
        rect.left_center() + vec2(30.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        theme::medium(12.5),
        if selected {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Hairline between drop-down groups.
pub fn menu_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment(
        [
            pos2(rect.left() + 6.0, rect.center().y),
            pos2(rect.right() - 6.0, rect.center().y),
        ],
        Stroke::new(1.0_f32, theme::BORDER),
    );
}

// ── Segmented control ───────────────────────────────────────────────────

/// Two-or-more option switch in one recessed track; the highlight slides
/// between options. Returns the index clicked this frame, if any.
/// Tabs across a panel: equal-width labels over a hairline, the active one
/// bright with an accent underline that slides between tabs.
pub fn tab_bar(
    ui: &mut Ui,
    id: &str,
    labels: &[&str],
    selected: usize,
    width: f32,
) -> Option<usize> {
    let count = labels.len().max(1);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    let cell = width / count as f32;
    let selected = selected.min(count - 1);
    let t = ui
        .ctx()
        .animate_value_with_time(ui.id().with(("tab_bar", id)), selected as f32, 0.14);
    let painter = ui.painter();
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER),
    );
    let under = Rect::from_min_size(
        pos2(rect.left() + t * cell + 10.0, rect.bottom() - 2.0),
        vec2(cell - 20.0, 2.0),
    );
    painter.rect_filled(under, 1.0, theme::ACCENT);
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let tab = Rect::from_min_size(
            pos2(rect.left() + i as f32 * cell, rect.top()),
            vec2(cell, rect.height()),
        );
        let response = ui
            .interact(tab, ui.id().with(("tab", id, i)), CLICK)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let lit = 1.0 - (t - i as f32).abs().min(1.0);
        let base = if response.hovered() {
            theme::TEXT_SECONDARY
        } else {
            theme::TEXT_MUTED
        };
        ui.painter().text(
            tab.center() - vec2(0.0, 1.0),
            Align2::CENTER_CENTER,
            *label,
            theme::semibold(12.5),
            lerp_color(base, theme::TEXT_PRIMARY, lit),
        );
        if response.clicked() && i != selected {
            clicked = Some(i);
        }
    }
    clicked
}

/// A slim slider: thin rail, accent fill up to a small white knob, and an
/// editable value box. Drop-in for the subset of `egui::Slider` the app
/// uses (egui sizes its knob from the row height, which made it huge).
pub struct Slim<'a> {
    get_set: Box<dyn 'a + FnMut(Option<f64>) -> f64>,
    range: std::ops::RangeInclusive<f64>,
    integer: bool,
    logarithmic: bool,
    step: Option<f64>,
    decimals: Option<usize>,
    suffix: String,
    text: String,
    show_value: bool,
}

impl<'a> Slim<'a> {
    pub fn new<Num: egui::emath::Numeric>(
        value: &'a mut Num,
        range: std::ops::RangeInclusive<Num>,
    ) -> Self {
        Self {
            get_set: Box::new(move |v| {
                if let Some(v) = v {
                    *value = Num::from_f64(v);
                }
                value.to_f64()
            }),
            range: range.start().to_f64()..=range.end().to_f64(),
            integer: Num::INTEGRAL,
            logarithmic: false,
            step: None,
            decimals: None,
            suffix: String::new(),
            text: String::new(),
            show_value: true,
        }
    }
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.logarithmic = on;
        self
    }
    pub fn step_by(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }
    pub fn fixed_decimals(mut self, decimals: usize) -> Self {
        self.decimals = Some(decimals);
        self
    }
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }
    pub fn show_value(mut self, on: bool) -> Self {
        self.show_value = on;
        self
    }

    /// Value at rail fraction `t` (0..1).
    fn value_at(&self, t: f64) -> f64 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let v = if self.logarithmic && lo > 0.0 && hi > lo {
            (lo.ln() + (hi.ln() - lo.ln()) * t).exp()
        } else {
            lo + (hi - lo) * t
        };
        let v = match self.step {
            Some(step) if step > 0.0 => lo + ((v - lo) / step).round() * step,
            _ => v,
        };
        let v = if self.integer { v.round() } else { v };
        v.clamp(lo.min(hi), hi.max(lo))
    }
    /// Rail fraction (0..1) of value `v`.
    fn fraction(&self, v: f64) -> f64 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let t = if self.logarithmic && lo > 0.0 && hi > lo {
            (v.max(lo).ln() - lo.ln()) / (hi.ln() - lo.ln())
        } else if hi != lo {
            (v - lo) / (hi - lo)
        } else {
            0.0
        };
        t.clamp(0.0, 1.0)
    }
}

impl egui::Widget for Slim<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        // One row, like egui's slider (it follows the parent's direction).
        ui.horizontal(|ui| self.contents(ui)).inner
    }
}

impl Slim<'_> {
    fn contents(mut self, ui: &mut Ui) -> Response {
        let size = vec2(ui.spacing().slider_width, ui.spacing().interact_size.y);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let knob_r = 7.0;
        let left = rect.left() + knob_r;
        let span = (rect.width() - knob_r * 2.0).max(1.0);
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - left) / span).clamp(0.0, 1.0) as f64;
            let next = self.value_at(t);
            if (next - (self.get_set)(None)).abs() > f64::EPSILON {
                (self.get_set)(Some(next));
                response.mark_changed();
            }
        }
        let value = (self.get_set)(None);
        let x = left + span * self.fraction(value) as f32;
        let y = rect.center().y;
        let hot = response.hovered() || response.dragged();
        let painter = ui.painter();
        let rail = Rect::from_min_max(pos2(left, y - 2.0), pos2(left + span, y + 2.0));
        painter.rect_filled(rail, 2.0, Color32::from_rgb(46, 50, 60));
        painter.rect_filled(
            Rect::from_min_max(rail.min, pos2(x, rail.max.y)),
            2.0,
            theme::ACCENT,
        );
        let knob = pos2(x, y);
        let r = if hot { knob_r + 0.5 } else { knob_r - 0.5 };
        painter.circle_filled(
            knob + vec2(0.0, 1.0),
            r + 1.0,
            Color32::from_black_alpha(90),
        );
        painter.circle_filled(knob, r, theme::PLAYHEAD);
        if response.dragged() {
            painter.circle_stroke(
                knob,
                r + 3.0,
                Stroke::new(3.0_f32, theme::ACCENT.gamma_multiply(0.25)),
            );
        }
        let mut response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if self.show_value {
            let range = self.range.clone();
            let speed = (range.end() - range.start()).abs() / 300.0;
            let decimals = self.decimals.or(self.integer.then_some(0));
            let suffix = std::mem::take(&mut self.suffix);
            let mut drag = egui::DragValue::from_get_set(|v| (self.get_set)(v))
                .range(range)
                .speed(speed)
                .suffix(suffix);
            if let Some(d) = decimals {
                drag = drag.fixed_decimals(d);
            }
            response = response.union(ui.add(drag));
        }
        if !self.text.is_empty() {
            ui.label(egui::RichText::new(&self.text).color(theme::TEXT_SECONDARY));
        }
        response
    }
}

/// "Fit to screen": lit (accent) when the selected video has been moved
/// or resized off its fitted frame, dimmed and inert when it already fits.
/// `width` stretches it (the clip panel); `None` sizes it to its label.
pub fn fit_button(ui: &mut Ui, moved: bool, width: Option<f32>) -> egui::Response {
    let font = theme::medium(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap("Fit to screen".to_owned(), font, Color32::WHITE);
    let icon = 14.0;
    let content = icon + 7.0 + galley.size().x;
    let size = vec2(width.unwrap_or(content + 24.0), 30.0);
    let sense = if moved { CLICK } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let lit =
        ui.ctx()
            .animate_bool_with_time(ui.id().with(("fit_lit", rect.min.x as i32)), moved, 0.18);
    let hovered = moved && response.hovered();
    let fill = lerp_color(
        theme::BG_BUTTON_DISABLED,
        if hovered {
            theme::BTN_PRIMARY_HOVER
        } else {
            theme::BTN_PRIMARY
        },
        lit,
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 7.0, fill);
    painter.rect_stroke(
        rect.shrink(0.5),
        7.0,
        Stroke::new(
            1.0_f32,
            lerp_color(theme::BORDER_SUBTLE, theme::ACCENT, 0.6 * lit),
        ),
    );
    let color = lerp_color(theme::TEXT_FAINT, theme::BTN_PRIMARY_TEXT, lit);
    let left = rect.center().x - content * 0.5;
    // Four corner brackets: "fit the frame".
    let frame = egui::Rect::from_center_size(
        egui::pos2(left + icon * 0.5, rect.center().y),
        vec2(icon, icon - 3.0),
    );
    let stroke = Stroke::new(1.5_f32, color);
    for (corner, dx, dy) in [
        (frame.left_top(), 1.0, 1.0),
        (frame.right_top(), -1.0, 1.0),
        (frame.left_bottom(), 1.0, -1.0),
        (frame.right_bottom(), -1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + vec2(4.0 * dx, 0.0)], stroke);
        painter.line_segment([corner, corner + vec2(0.0, 4.0 * dy)], stroke);
    }
    painter.galley(
        egui::pos2(left + icon + 7.0, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    if lit > 0.0 && lit < 1.0 {
        ui.ctx().request_repaint();
    }
    if moved {
        response
            .on_hover_text("Put the video back: centered, filling the canvas (or whole with bars when the shapes differ). Ctrl+Z undoes it.")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text(
            "The video already fits the screen. Drag or resize it on the preview to reframe.",
        )
    }
}

/// The Simple Clip / Advanced switch: a large pill whose lit half slides
/// to the active mode.
pub fn mode_switch(ui: &mut Ui, options: [(&str, &str); 2], selected: usize) -> Option<usize> {
    let half = vec2(148.0, 36.0);
    let pad = 3.0;
    let (outer, _) = ui.allocate_exact_size(
        vec2(half.x * 2.0 + pad * 2.0, half.y + pad * 2.0),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(outer, 12.0, theme::BG_PREVIEW);
    painter.rect_stroke(outer, 12.0, Stroke::new(1.0_f32, theme::BORDER));
    let selected = selected.min(1);
    let t =
        ui.ctx()
            .animate_value_with_time(ui.id().with("mode_switch_knob"), selected as f32, 0.16);
    let knob = egui::Rect::from_min_size(outer.min + vec2(pad + t * half.x, pad), half);
    painter.rect_filled(knob, 9.0, theme::BTN_PRIMARY);
    painter.rect_stroke(
        knob,
        9.0,
        Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.55)),
    );
    let mut clicked = None;
    for (i, (label, tip)) in options.iter().enumerate() {
        let rect = egui::Rect::from_min_size(outer.min + vec2(pad + i as f32 * half.x, pad), half);
        let response = ui
            .interact(rect, ui.id().with(("mode_switch", i)), CLICK)
            .on_hover_text(*tip)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        // How lit this half is (follows the sliding knob).
        let lit = 1.0 - (t - i as f32).abs().min(1.0);
        if response.hovered() && i != selected {
            ui.painter().rect_filled(
                rect.shrink(1.0),
                9.0,
                theme::BG_BUTTON_HOVER.gamma_multiply(0.6),
            );
        }
        let color = lerp_color(theme::TEXT_SECONDARY, theme::BTN_PRIMARY_TEXT, lit);
        let galley = ui
            .painter()
            .layout_no_wrap((*label).to_owned(), theme::semibold(14.0), color);
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, color);
        if response.clicked() && i != selected {
            clicked = Some(i);
        }
    }
    clicked
}

pub fn segmented(
    ui: &mut Ui,
    id: &str,
    options: &[(&str, &str)],
    selected: usize,
) -> Option<usize> {
    segmented_sized(ui, id, options, selected, None)
}

/// `segmented` stretched to `width`, options sharing it equally.
pub fn segmented_fill(
    ui: &mut Ui,
    id: &str,
    options: &[(&str, &str)],
    selected: usize,
    width: f32,
) -> Option<usize> {
    segmented_sized(ui, id, options, selected, Some(width))
}

fn segmented_sized(
    ui: &mut Ui,
    id: &str,
    options: &[(&str, &str)],
    selected: usize,
    fill: Option<f32>,
) -> Option<usize> {
    let font = theme::medium(12.5);
    let pad = 3.0;
    let widths: Vec<f32> = match fill {
        Some(width) => {
            vec![((width - pad * 2.0) / options.len().max(1) as f32).floor(); options.len()]
        }
        None => options
            .iter()
            .map(|(label, _)| {
                ui.painter()
                    .layout_no_wrap((*label).to_owned(), font.clone(), theme::TEXT_PRIMARY)
                    .size()
                    .x
                    + 24.0
            })
            .collect(),
    };
    let total = vec2(widths.iter().sum::<f32>() + pad * 2.0, 30.0);
    let (outer, _) = ui.allocate_exact_size(total, Sense::hover());
    ui.painter().rect_filled(outer, 8.0, theme::BG_PREVIEW);
    ui.painter()
        .rect_stroke(outer, 8.0, Stroke::new(1.0_f32, theme::BORDER_SUBTLE));

    let lefts: Vec<f32> = widths
        .iter()
        .scan(outer.left() + pad, |x, w| {
            let left = *x;
            *x += w;
            Some(left)
        })
        .collect();
    let target = selected.min(options.len().saturating_sub(1));
    let anim_id = ui.id().with(("segmented", id));
    let x = ui
        .ctx()
        .animate_value_with_time(anim_id.with("x"), lefts[target] - outer.left(), 0.18);
    let w = ui
        .ctx()
        .animate_value_with_time(anim_id.with("w"), widths[target], 0.18);
    let pill = Rect::from_min_size(
        pos2(outer.left() + x, outer.top() + pad),
        vec2(w, outer.height() - pad * 2.0),
    );
    ui.painter().rect_filled(pill, 6.0, theme::BG_BUTTON_ACTIVE);
    ui.painter().rect_stroke(
        pill,
        6.0,
        Stroke::new(1.0_f32, Color32::from_white_alpha(14)),
    );

    let mut clicked = None;
    for (i, ((label, tip), (left, width))) in options
        .iter()
        .zip(lefts.iter().zip(widths.iter()))
        .enumerate()
    {
        let rect = Rect::from_min_size(
            pos2(*left, outer.top() + pad),
            vec2(*width, outer.height() - pad * 2.0),
        );
        let response = ui
            .interact(rect, anim_id.with(i), CLICK)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let response = if tip.is_empty() {
            response
        } else {
            response.on_hover_text(*tip)
        };
        let color = if i == target {
            theme::TEXT_PRIMARY
        } else if response.hovered() {
            theme::TEXT_SECONDARY
        } else {
            theme::TEXT_MUTED
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            color,
        );
        if response.clicked() && i != target {
            clicked = Some(i);
        }
    }
    clicked
}

/// Timecode with its leading zeros dimmed, so the significant digits pop
/// (00:0**3.258**). Expects `media::format_timestamp` output.
pub fn timecode_job(text: &str, size: f32, color: Color32) -> egui::text::LayoutJob {
    let split = text
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_digit() && *ch != '0')
        .map(|(i, _)| i)
        // Keep at least "0.xxx": the digit before the decimal point stays bright.
        .map(|i| i.min(text.find('.').map_or(i, |dot| dot.saturating_sub(1))))
        .unwrap_or_else(|| text.find('.').map_or(0, |dot| dot.saturating_sub(1)));
    let mut job = egui::text::LayoutJob::default();
    let format = |c| egui::TextFormat {
        font_id: theme::mono(size),
        color: c,
        ..Default::default()
    };
    job.append(&text[..split], 0.0, format(theme::TEXT_FAINT));
    job.append(&text[split..], 0.0, format(color));
    job
}

/// Selection on a timeline item: a soft-white ring with a dark hairline
/// inside it (reads on blue, teal and purple alike), and a small grip on
/// each end where the item trims. Drawn inside `rect`, so it never spills
/// over a neighbour.
pub fn paint_selection(painter: &egui::Painter, rect: Rect, rounding: impl Into<egui::Rounding>) {
    let rounding = rounding.into();
    let inner = egui::Rounding {
        nw: (rounding.nw - 2.0).max(0.0),
        ne: (rounding.ne - 2.0).max(0.0),
        sw: (rounding.sw - 2.0).max(0.0),
        se: (rounding.se - 2.0).max(0.0),
    };
    painter.rect_stroke(
        rect.shrink(2.5),
        inner,
        Stroke::new(1.0_f32, Color32::from_black_alpha(80)),
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        rounding,
        Stroke::new(2.0_f32, theme::PLAYHEAD),
    );
    if rect.width() < 28.0 || rect.height() < 22.0 {
        return;
    }
    let h = (rect.height() - 16.0).min(18.0);
    for x in [rect.left() + 2.5, rect.right() - 2.5] {
        painter.rect_filled(
            Rect::from_center_size(egui::pos2(x, rect.center().y), vec2(4.0, h)),
            2.0,
            theme::PLAYHEAD,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bright(text: &str) -> String {
        let job = timecode_job(text, 12.0, Color32::WHITE);
        job.sections
            .iter()
            .filter(|s| s.format.color == Color32::WHITE)
            .map(|s| job.text[s.byte_range.clone()].to_owned())
            .collect()
    }

    #[test]
    fn timecode_dims_only_leading_zeros() {
        assert_eq!(bright("00:03.258"), "3.258");
        assert_eq!(bright("00:00.000"), "0.000");
        assert_eq!(bright("01:20.500"), "1:20.500");
        assert_eq!(bright("00:00.250"), "0.250");
    }
}
